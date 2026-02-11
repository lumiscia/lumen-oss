use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    env,
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, mpsc},
    thread,
};

use anyhow::{Context, anyhow};
use image::{ImageEncoder, codecs::png::PngEncoder};
use lumen::{
    compile::{CompiledOperationKind, CompiledTimeline},
    gpu::{FrameImage, FrameProvider, GpuRenderer, ProviderError},
    model::{Source, SourceKind},
    time::Rational,
};

const MEDIA_ROOT_ENV: &str = "LUMEN_MEDIA_ROOT";
const DEFAULT_ENCODE_QUEUE: usize = 8;
const DEFAULT_MAX_DECODED_FRAMES: usize = 120_000;

pub struct FfmpegRenderBackend {
    timeline: Arc<CompiledTimeline>,
}

impl FfmpegRenderBackend {
    pub fn new(timeline: Arc<CompiledTimeline>) -> Self {
        Self { timeline }
    }

    pub fn render_to_mp4(&self, on_progress: &mut dyn FnMut(u64, u64)) -> anyhow::Result<Vec<u8>> {
        let requirements =
            collect_requirements(self.timeline.as_ref(), 0..self.timeline.total_frames())?;
        let mut assets = prepare_assets(self.timeline.as_ref(), &requirements)?;

        let width = self.timeline.canvas.width;
        let height = self.timeline.canvas.height;
        let fps = self.timeline.timeline.fps;
        let total_frames = self.timeline.total_frames();

        let queue_capacity = env::var("LUMEN_ENCODE_QUEUE")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_ENCODE_QUEUE);

        let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(queue_capacity);
        let encoder = choose_video_encoder();

        let encode_handle =
            thread::spawn(move || encode_rgba_stream(width, height, fps, encoder, rx));

        let mut renderer = GpuRenderer::new(width, height)
            .map_err(|err| anyhow!("failed to initialize GPU renderer: {err}"))?;

        for frame in 0..total_frames {
            let rgba = renderer
                .render_frame(self.timeline.as_ref(), frame, &mut assets)
                .map_err(|err| anyhow!("failed to render frame {frame}: {err}"))?;
            tx.send(rgba)
                .map_err(|_| anyhow!("encoder thread stopped unexpectedly"))?;

            on_progress(frame.saturating_add(1), total_frames);
        }

        drop(tx);

        match encode_handle.join() {
            Ok(result) => result,
            Err(_) => Err(anyhow!("encoder thread panicked")),
        }
    }

    pub fn render_frame_png(&self, frame: u64) -> anyhow::Result<Vec<u8>> {
        let requirements = collect_requirements(self.timeline.as_ref(), std::iter::once(frame))?;
        let mut assets = prepare_assets(self.timeline.as_ref(), &requirements)?;

        let mut renderer =
            GpuRenderer::new(self.timeline.canvas.width, self.timeline.canvas.height)
                .map_err(|err| anyhow!("failed to initialize GPU renderer: {err}"))?;
        let rgba = renderer
            .render_frame(self.timeline.as_ref(), frame, &mut assets)
            .map_err(|err| anyhow!("failed to render preview frame {frame}: {err}"))?;

        let mut png = Vec::new();
        let encoder = PngEncoder::new(&mut png);
        encoder
            .write_image(
                &rgba,
                self.timeline.canvas.width,
                self.timeline.canvas.height,
                image::ExtendedColorType::Rgba8,
            )
            .context("failed to encode preview PNG")?;
        Ok(png)
    }
}

#[derive(Default)]
struct FrameRequirements {
    images: HashSet<String>,
    videos: HashMap<String, BTreeSet<u64>>,
}

fn collect_requirements(
    timeline: &CompiledTimeline,
    frames: impl IntoIterator<Item = u64>,
) -> anyhow::Result<FrameRequirements> {
    let mut requirements = FrameRequirements::default();

    for frame in frames {
        let operation_indices = timeline
            .operation_indices_for_frame(frame)
            .map_err(|err| anyhow!(err.to_string()))?;

        for operation_index in operation_indices {
            let Some(operation) = timeline.operation(*operation_index) else {
                return Err(anyhow!("missing operation index {}", operation_index));
            };

            match &operation.kind {
                CompiledOperationKind::Image(image) => {
                    requirements.images.insert(image.source_id.clone());
                }
                CompiledOperationKind::Video(video) => {
                    if let Some(source_frame) = operation
                        .resolve_video_source_frame(frame)
                        .map_err(|err| anyhow!(err.to_string()))?
                    {
                        requirements
                            .videos
                            .entry(video.source_id.clone())
                            .or_default()
                            .insert(source_frame);
                    }
                }
                CompiledOperationKind::Solid { .. }
                | CompiledOperationKind::Shape(_)
                | CompiledOperationKind::Text(_) => {}
            }
        }
    }

    Ok(requirements)
}

#[derive(Default)]
struct PreparedAssets {
    images: HashMap<String, FrameImage>,
    videos: HashMap<String, BTreeMap<u64, FrameImage>>,
}

impl FrameProvider for PreparedAssets {
    fn image(&mut self, source_id: &str) -> Result<Option<FrameImage>, ProviderError> {
        Ok(self.images.get(source_id).cloned())
    }

    fn video_frame(
        &mut self,
        source_id: &str,
        source_frame: u64,
    ) -> Result<Option<FrameImage>, ProviderError> {
        let Some(frames) = self.videos.get(source_id) else {
            return Err(ProviderError::MissingSource(source_id.to_string()));
        };

        if let Some(frame) = frames.get(&source_frame) {
            return Ok(Some(frame.clone()));
        }

        let prev = frames.range(..=source_frame).next_back();
        let next = frames.range(source_frame..).next();

        Ok(prev
            .map(|(_, frame)| frame.clone())
            .or_else(|| next.map(|(_, frame)| frame.clone())))
    }
}

fn prepare_assets(
    timeline: &CompiledTimeline,
    requirements: &FrameRequirements,
) -> anyhow::Result<PreparedAssets> {
    let max_frames = env::var("LUMEN_MAX_DECODED_SOURCE_FRAMES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_MAX_DECODED_FRAMES);

    let total_requested_video_frames: usize = requirements.videos.values().map(BTreeSet::len).sum();

    if total_requested_video_frames > max_frames {
        return Err(anyhow!(
            "requested decoded video frames ({total_requested_video_frames}) exceeds configured bound ({max_frames})"
        ));
    }

    let mut prepared = PreparedAssets::default();

    for source_id in &requirements.images {
        let source = timeline
            .source(source_id)
            .ok_or_else(|| anyhow!("missing source `{source_id}`"))?;
        let image = decode_image_source(source)?;
        prepared.images.insert(source_id.clone(), image);
    }

    let fps = timeline.timeline.fps;
    let mut decode_handles = Vec::new();

    for (source_id, frames) in &requirements.videos {
        let source = timeline
            .source(source_id)
            .ok_or_else(|| anyhow!("missing source `{source_id}`"))?
            .clone();
        let frame_set = frames.clone();
        decode_handles.push(thread::spawn(move || {
            decode_video_source_frames(&source, fps, frame_set)
                .map(|frames| (source.id.clone(), frames))
        }));
    }

    for handle in decode_handles {
        let result = match handle.join() {
            Ok(result) => result,
            Err(_) => return Err(anyhow!("video decode thread panicked")),
        }?;
        prepared.videos.insert(result.0, result.1);
    }

    Ok(prepared)
}

fn decode_image_source(source: &Source) -> anyhow::Result<FrameImage> {
    match &source.kind {
        SourceKind::File { path, .. } => {
            let resolved = resolve_source_file_path(path)?;
            let image = image::ImageReader::open(&resolved)
                .with_context(|| format!("failed to open image `{}`", resolved.display()))?
                .decode()
                .with_context(|| format!("failed to decode image `{}`", resolved.display()))?;
            let rgba = image.into_rgba8();
            FrameImage::new(rgba.width(), rgba.height(), rgba.into_raw())
                .map_err(|err| anyhow!("failed to build image frame: {err}"))
        }
        SourceKind::Generator { .. } => Err(anyhow!(
            "generator sources are not supported for image clips"
        )),
    }
}

fn decode_video_source_frames(
    source: &Source,
    fps: Rational,
    requested_frames: BTreeSet<u64>,
) -> anyhow::Result<BTreeMap<u64, FrameImage>> {
    if requested_frames.is_empty() {
        return Ok(BTreeMap::new());
    }

    let (width, height) = probe_video_dimensions(source)?;
    let frame_size = frame_size(width, height)?;
    let min_requested = requested_frames
        .iter()
        .next()
        .copied()
        .ok_or_else(|| anyhow!("requested frame set unexpectedly empty"))?;
    let max_requested = requested_frames
        .iter()
        .next_back()
        .copied()
        .ok_or_else(|| anyhow!("requested frame set unexpectedly empty"))?;

    let mut command = decode_command(source, fps, min_requested, max_requested)?;
    command.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = command
        .spawn()
        .context("failed to spawn ffmpeg decode process")?;

    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("ffmpeg decode stdout was unavailable"))?;

    let mut decoded = BTreeMap::new();
    let frame_count = max_requested.saturating_sub(min_requested).saturating_add(1);
    for decoded_index in 0..frame_count {
        let frame_index = min_requested.saturating_add(decoded_index);
        let mut buffer = vec![0u8; frame_size];
        match stdout.read_exact(&mut buffer) {
            Ok(()) => {
                if requested_frames.contains(&frame_index) {
                    let frame = FrameImage::new(width, height, buffer)
                        .map_err(|err| anyhow!("decoded frame payload was invalid: {err}"))?;
                    decoded.insert(frame_index, frame);
                }
            }
            Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => {
                break;
            }
            Err(err) => {
                return Err(anyhow!("failed while reading decoded raw video: {err}"));
            }
        }
    }

    drop(stdout);
    let output = child
        .wait_with_output()
        .context("failed to wait for ffmpeg decode process")?;

    if !output.status.success() {
        return Err(anyhow!(
            "ffmpeg decode failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    Ok(decoded)
}

fn decode_command(
    source: &Source,
    fps: Rational,
    min_frame: u64,
    max_frame: u64,
) -> anyhow::Result<Command> {
    let mut command = Command::new("ffmpeg");
    command
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-nostdin");

    match &source.kind {
        SourceKind::File { path, .. } => {
            let resolved = resolve_source_file_path(path)?;
            command.arg("-hwaccel").arg("auto").arg("-i").arg(resolved);
        }
        SourceKind::Generator { filter, .. } => {
            command.arg("-f").arg("lavfi").arg("-i").arg(filter);
        }
    }

    command
        .arg("-an")
        .arg("-vf")
        .arg(format!(
            "fps={}/{},trim=start_frame={}:end_frame={},setpts=PTS-STARTPTS",
            fps.num,
            fps.den,
            min_frame,
            max_frame.saturating_add(1)
        ))
        .arg("-frames:v")
        .arg(max_frame.saturating_sub(min_frame).saturating_add(1).to_string())
        .arg("-f")
        .arg("rawvideo")
        .arg("-pix_fmt")
        .arg("rgba")
        .arg("pipe:1");

    Ok(command)
}

fn probe_video_dimensions(source: &Source) -> anyhow::Result<(u32, u32)> {
    let mut command = Command::new("ffprobe");
    command
        .arg("-v")
        .arg("error")
        .arg("-select_streams")
        .arg("v:0")
        .arg("-show_entries")
        .arg("stream=width,height")
        .arg("-of")
        .arg("csv=p=0:s=x");

    match &source.kind {
        SourceKind::File { path, .. } => {
            command.arg(resolve_source_file_path(path)?);
        }
        SourceKind::Generator { filter, .. } => {
            command.arg("-f").arg("lavfi").arg("-i").arg(filter);
        }
    }

    let output = command
        .output()
        .context("failed to execute ffprobe for dimensions")?;

    if !output.status.success() {
        return Err(anyhow!(
            "ffprobe failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let text = String::from_utf8(output.stdout)
        .context("ffprobe dimensions output was not valid UTF-8")?;
    let line = text.lines().next().unwrap_or_default().trim();
    let (w, h) = line
        .split_once('x')
        .ok_or_else(|| anyhow!("could not parse ffprobe dimensions `{line}`"))?;

    let width = w
        .parse::<u32>()
        .with_context(|| format!("invalid width `{w}`"))?;
    let height = h
        .parse::<u32>()
        .with_context(|| format!("invalid height `{h}`"))?;

    if width == 0 || height == 0 {
        return Err(anyhow!("ffprobe returned zero dimensions"));
    }

    Ok((width, height))
}

fn frame_size(width: u32, height: u32) -> anyhow::Result<usize> {
    let pixels = (width as usize)
        .checked_mul(height as usize)
        .ok_or_else(|| anyhow!("frame size overflow"))?;
    pixels
        .checked_mul(4)
        .ok_or_else(|| anyhow!("frame size overflow"))
}

fn choose_video_encoder() -> String {
    if let Ok(encoder) = env::var("LUMEN_VIDEO_ENCODER") {
        let encoder = encoder.trim();
        if !encoder.is_empty() {
            return encoder.to_string();
        }
    }

    if cfg!(target_os = "macos") {
        return "h264_videotoolbox".to_string();
    }

    "libx264".to_string()
}

fn encode_rgba_stream(
    width: u32,
    height: u32,
    fps: Rational,
    encoder: String,
    rx: mpsc::Receiver<Vec<u8>>,
) -> anyhow::Result<Vec<u8>> {
    let tmp = tempfile::tempdir().context("failed to create temporary encode directory")?;
    let output_path = tmp.path().join("output.mp4");

    let mut command = Command::new("ffmpeg");
    command
        .arg("-y")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-nostdin")
        .arg("-f")
        .arg("rawvideo")
        .arg("-pix_fmt")
        .arg("rgba")
        .arg("-s:v")
        .arg(format!("{width}x{height}"))
        .arg("-r")
        .arg(format!("{}/{}", fps.num, fps.den))
        .arg("-i")
        .arg("pipe:0")
        .arg("-an")
        .arg("-c:v")
        .arg(&encoder)
        .arg("-pix_fmt")
        .arg("yuv420p")
        .arg("-movflags")
        .arg("+faststart")
        .arg(&output_path)
        .stdin(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command.spawn().context("failed to spawn ffmpeg encoder")?;

    {
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| anyhow!("ffmpeg stdin unavailable"))?;

        for frame in rx {
            stdin
                .write_all(&frame)
                .context("failed writing frame to ffmpeg stdin")?;
        }
    }

    let output = child
        .wait_with_output()
        .context("failed waiting for ffmpeg encode")?;

    if !output.status.success() {
        return Err(anyhow!(
            "ffmpeg encode failed with encoder `{encoder}`: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    std::fs::read(&output_path)
        .with_context(|| format!("failed to read encoded output `{}`", output_path.display()))
}

fn resolve_source_file_path(path: &str) -> anyhow::Result<PathBuf> {
    let root = media_root()?;
    resolve_local_path_with_root(path, &root)
}

fn media_root() -> anyhow::Result<PathBuf> {
    let raw = env::var(MEDIA_ROOT_ENV).unwrap_or_else(|_| ".".to_string());
    let path = PathBuf::from(raw);
    path.canonicalize()
        .with_context(|| format!("failed to resolve media root from {MEDIA_ROOT_ENV}"))
}

fn resolve_local_path_with_root(source: &str, root: &Path) -> anyhow::Result<PathBuf> {
    let root = root
        .canonicalize()
        .with_context(|| format!("failed to canonicalize media root `{}`", root.display()))?;

    if source.contains("://") && !source.starts_with("file://") {
        return Err(anyhow!("unsupported asset URI scheme for `{source}`"));
    }

    let raw_path = source.strip_prefix("file://").unwrap_or(source);
    let path = Path::new(raw_path);
    if path.as_os_str().is_empty() {
        return Err(anyhow!("asset path must not be empty"));
    }

    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(anyhow!(
            "parent traversal is not allowed in asset paths: `{source}`"
        ));
    }

    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };

    let candidate = candidate.canonicalize().with_context(|| {
        format!(
            "failed to canonicalize asset path `{}`",
            candidate.display()
        )
    })?;

    if !candidate.starts_with(&root) {
        return Err(anyhow!(
            "asset path escapes allowed media root: `{}`",
            candidate.display()
        ));
    }

    Ok(candidate)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::resolve_local_path_with_root;

    #[test]
    fn rejects_traversal_path() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let result = resolve_local_path_with_root("../secret.txt", tmp.path());
        assert!(result.is_err());
    }

    #[test]
    fn rejects_non_file_uri_scheme() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let result = resolve_local_path_with_root("https://example.com/video.mp4", tmp.path());
        assert!(result.is_err());
    }

    #[test]
    fn resolves_under_media_root() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let file_path = tmp.path().join("clip.png");
        fs::write(&file_path, b"x").expect("write");

        let resolved = resolve_local_path_with_root("clip.png", tmp.path()).expect("resolve");
        assert_eq!(resolved, file_path.canonicalize().expect("canonicalize"));
    }
}
