// Items here are used conditionally by feature-gated backends (libav / subprocess).
#![allow(dead_code)]

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    env,
    io::Write,
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
};

use anyhow::{Context, anyhow};
use lumen::{
    backend::{FrameImage, FrameProvider, ProviderError, RenderBackend},
    compile::{CompiledOperationKind, CompiledTimeline},
    model::{FitMode, LoopMode, Source, SourceKind, SourcePipeline},
    time::Rational,
};

pub const MEDIA_ROOT_ENV: &str = "LUMEN_MEDIA_ROOT";
pub const DEFAULT_ENCODE_QUEUE: usize = 8;
pub const DEFAULT_MAX_DECODED_FRAMES: usize = 120_000;
#[allow(dead_code)]
pub const DEFAULT_STREAM_CACHE_FRAMES: usize = 256;

pub fn create_renderer(width: u32, height: u32) -> anyhow::Result<Box<dyn RenderBackend>> {
    #[cfg(feature = "renderer-skia")]
    {
        let renderer = lumen::backend::skia::SkiaRenderer::new(width, height)
            .map_err(|err| anyhow!("failed to initialize Skia renderer: {err}"))?;
        return Ok(Box::new(renderer));
    }
    #[cfg(not(feature = "renderer-skia"))]
    {
        Err(anyhow!("renderer-skia feature is required"))
    }
}

#[derive(Debug, Clone, Default)]
pub struct RenderBackendOptions {
    pub media_root: Option<PathBuf>,
    pub video_encoder: Option<String>,
    pub encode_queue: Option<usize>,
    pub max_decoded_source_frames: Option<usize>,
    pub stream_cache_frames: Option<usize>,
}

#[derive(Default)]
pub struct FrameRequirements {
    pub images: HashSet<String>,
    pub videos: HashMap<String, BTreeSet<u64>>,
}

pub fn collect_requirements(
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
pub struct PreparedAssets {
    pub images: HashMap<String, FrameImage>,
    pub videos: HashMap<String, BTreeMap<u64, FrameImage>>,
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

pub fn decode_image_source(source: &Source, media_root: &Path) -> anyhow::Result<FrameImage> {
    match &source.kind {
        SourceKind::File { path, .. } => {
            let resolved = resolve_source_file_path(path, media_root)?;
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

#[allow(dead_code)]
pub fn frame_size(width: u32, height: u32) -> anyhow::Result<usize> {
    let pixels = (width as usize)
        .checked_mul(height as usize)
        .ok_or_else(|| anyhow!("frame size overflow"))?;
    pixels
        .checked_mul(4)
        .ok_or_else(|| anyhow!("frame size overflow"))
}

pub fn choose_video_encoder(override_encoder: Option<&str>) -> String {
    if let Some(encoder) = override_encoder {
        let encoder = encoder.trim();
        if !encoder.is_empty() {
            return encoder.to_string();
        }
    }

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

pub fn encode_rgba_stream(
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

fn approx_eq(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-3
}

struct FastPathPlan {
    source: Source,
    pipeline: SourcePipeline,
    fit: FitMode,
}

fn analyze_fast_path_timeline(timeline: &CompiledTimeline) -> anyhow::Result<Option<FastPathPlan>> {
    if timeline.has_compositing_nodes() {
        return Ok(None);
    }

    let total_frames = timeline.total_frames();
    if total_frames == 0 {
        return Ok(None);
    }

    let mut single_operation_index: Option<usize> = None;
    for frame in 0..total_frames {
        let operation_indices = timeline
            .operation_indices_for_frame(frame)
            .map_err(|err| anyhow!(err.to_string()))?;
        if operation_indices.len() != 1 {
            return Ok(None);
        }

        let op_idx = *operation_indices
            .first()
            .ok_or_else(|| anyhow!("missing operation for frame {frame}"))?;
        match single_operation_index {
            Some(expected) if expected != op_idx => return Ok(None),
            None => single_operation_index = Some(op_idx),
            _ => {}
        }
    }

    let op_idx =
        single_operation_index.ok_or_else(|| anyhow!("missing operation index for timeline"))?;
    let operation = timeline
        .operation(op_idx)
        .ok_or_else(|| anyhow!("missing operation index {}", op_idx))?;

    if operation.start_frame != 0 || operation.end_frame != total_frames {
        return Ok(None);
    }
    if !approx_eq(operation.opacity, 1.0) {
        return Ok(None);
    }
    if !approx_eq(operation.transform.x, 0.0)
        || !approx_eq(operation.transform.y, 0.0)
        || !approx_eq(operation.transform.rotation_degrees, 0.0)
    {
        return Ok(None);
    }

    let expected_width = timeline.canvas.width as f32;
    let expected_height = timeline.canvas.height as f32;
    let Some(width) = operation.transform.width else {
        return Ok(None);
    };
    let Some(height) = operation.transform.height else {
        return Ok(None);
    };
    if !approx_eq(width, expected_width) || !approx_eq(height, expected_height) {
        return Ok(None);
    }

    let CompiledOperationKind::Video(video) = &operation.kind else {
        return Ok(None);
    };
    if !approx_eq(video.corner_radius, 0.0) {
        return Ok(None);
    }
    if video.pipeline.looping != LoopMode::None {
        return Ok(None);
    }
    if !video.pipeline.speed.is_finite() || !approx_eq(video.pipeline.speed, 1.0) {
        return Ok(None);
    }

    for frame in 0..total_frames {
        let source_frame = operation
            .resolve_video_source_frame(frame)
            .map_err(|err| anyhow!(err.to_string()))?;
        if source_frame.is_none() {
            return Ok(None);
        }
    }

    let source = timeline
        .source(video.source_id.as_str())
        .ok_or_else(|| anyhow!("missing source `{}`", video.source_id))?
        .clone();

    Ok(Some(FastPathPlan {
        source,
        pipeline: video.pipeline.clone(),
        fit: video.fit,
    }))
}

fn background_hex_rgba(color: lumen::model::ColorRgba) -> String {
    format!(
        "0x{:02x}{:02x}{:02x}{:02x}",
        color.r(),
        color.g(),
        color.b(),
        color.a()
    )
}

pub fn try_render_ffmpeg_fast_path(
    timeline: &CompiledTimeline,
    options: &RenderBackendOptions,
    on_progress: &mut dyn FnMut(u64, u64),
) -> anyhow::Result<Option<Vec<u8>>> {
    let Some(plan) = analyze_fast_path_timeline(timeline)? else {
        return Ok(None);
    };

    let media_root = media_root(options.media_root.as_deref())?;
    let encoder = choose_video_encoder(options.video_encoder.as_deref());
    let fps = timeline.timeline.fps;
    let total_frames = timeline.total_frames();

    let mut filters = Vec::<String>::new();
    filters.push(format!("fps={}/{}", fps.num, fps.den));

    if let Some(trim) = plan.pipeline.trim {
        if let Some(end_frame) = trim.end_frame {
            filters.push(format!(
                "trim=start_frame={}:end_frame={}",
                trim.start_frame, end_frame
            ));
        } else if trim.start_frame > 0 {
            filters.push(format!("trim=start_frame={}", trim.start_frame));
        }
    }

    if plan.pipeline.reverse {
        filters.push("reverse".to_string());
    }

    filters.push("setpts=PTS-STARTPTS".to_string());
    match plan.fit {
        FitMode::Fill => {
            filters.push(format!(
                "scale={}:{}:flags=fast_bilinear",
                timeline.canvas.width, timeline.canvas.height
            ));
        }
        FitMode::Contain => {
            filters.push(format!(
                "scale={}:{}:flags=fast_bilinear:force_original_aspect_ratio=decrease",
                timeline.canvas.width, timeline.canvas.height
            ));
            filters.push(format!(
                "pad={}:{}:(ow-iw)/2:(oh-ih)/2:{}",
                timeline.canvas.width,
                timeline.canvas.height,
                background_hex_rgba(timeline.canvas.background)
            ));
        }
        FitMode::Cover => {
            filters.push(format!(
                "scale={}:{}:flags=fast_bilinear:force_original_aspect_ratio=increase",
                timeline.canvas.width, timeline.canvas.height
            ));
            filters.push(format!(
                "crop={}:{}",
                timeline.canvas.width, timeline.canvas.height
            ));
        }
    }

    let tmp = tempfile::tempdir().context("failed to create temporary fast-path directory")?;
    let output_path = tmp.path().join("fast-path.mp4");

    let mut command = Command::new("ffmpeg");
    command
        .arg("-y")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-nostdin");

    match &plan.source.kind {
        SourceKind::File { path, .. } => {
            let resolved = resolve_source_file_path(path, &media_root)?;
            command.arg("-i").arg(resolved);
        }
        SourceKind::Generator { filter, .. } => {
            command.arg("-f").arg("lavfi").arg("-i").arg(filter);
        }
    }

    command
        .arg("-an")
        .arg("-vf")
        .arg(filters.join(","))
        .arg("-frames:v")
        .arg(total_frames.to_string())
        .arg("-c:v")
        .arg(&encoder)
        .arg("-pix_fmt")
        .arg("yuv420p")
        .arg("-movflags")
        .arg("+faststart")
        .arg(&output_path)
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    let output = command
        .output()
        .context("failed to spawn ffmpeg fast-path process")?;
    if !output.status.success() {
        return Err(anyhow!(
            "ffmpeg fast-path failed with encoder `{encoder}`: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let bytes = std::fs::read(&output_path).with_context(|| {
        format!(
            "failed to read fast-path output `{}`",
            output_path.display()
        )
    })?;
    on_progress(total_frames, total_frames);
    Ok(Some(bytes))
}

pub fn resolve_source_file_path(path: &str, root_override: &Path) -> anyhow::Result<PathBuf> {
    let root = media_root(Some(root_override))?;
    resolve_local_path_with_root(path, &root)
}

pub fn media_root(override_root: Option<&Path>) -> anyhow::Result<PathBuf> {
    let path = match override_root {
        Some(path) => path.to_path_buf(),
        None => {
            let raw = env::var(MEDIA_ROOT_ENV).unwrap_or_else(|_| ".".to_string());
            PathBuf::from(raw)
        }
    };
    path.canonicalize()
        .with_context(|| format!("failed to resolve media root from {MEDIA_ROOT_ENV}"))
}

pub fn resolve_local_path_with_root(source: &str, root: &Path) -> anyhow::Result<PathBuf> {
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
