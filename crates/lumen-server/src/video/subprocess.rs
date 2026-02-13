use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    env,
    io::{self, BufReader, Read},
    num::NonZeroUsize,
    path::Path,
    process::{Child, ChildStdout, Command, Stdio},
    sync::{Arc, mpsc},
    thread,
};

use anyhow::{Context, anyhow};
use image::{ImageEncoder, codecs::png::PngEncoder};
use lru::LruCache;
use lumen::{
    backend::{FrameImage, FrameProvider, ProviderError},
    compile::{CompiledOperationKind, CompiledTimeline},
    model::{Source, SourceKind, SourceMediaType},
    time::Rational,
};

use super::common::{
    DEFAULT_ENCODE_QUEUE, DEFAULT_MAX_DECODED_FRAMES, DEFAULT_STREAM_CACHE_FRAMES,
    FrameRequirements, PreparedAssets, choose_video_encoder, collect_requirements, create_renderer,
    decode_image_source, encode_rgba_stream, frame_size, media_root, resolve_source_file_path,
    try_render_ffmpeg_fast_path,
};

pub use super::common::RenderBackendOptions;

pub struct FfmpegRenderBackend {
    timeline: Arc<CompiledTimeline>,
    options: RenderBackendOptions,
}

impl FfmpegRenderBackend {
    pub fn new(timeline: Arc<CompiledTimeline>) -> Self {
        Self {
            timeline,
            options: RenderBackendOptions::default(),
        }
    }

    pub fn new_with_options(
        timeline: Arc<CompiledTimeline>,
        options: RenderBackendOptions,
    ) -> Self {
        Self { timeline, options }
    }

    pub fn render_to_mp4(
        &mut self,
        on_progress: &mut dyn FnMut(u64, u64),
    ) -> anyhow::Result<Vec<u8>> {
        if let Some(bytes) =
            try_render_ffmpeg_fast_path(self.timeline.as_ref(), &self.options, on_progress)?
        {
            return Ok(bytes);
        }

        let media_root = media_root(self.options.media_root.as_deref())?;
        let stream_cache_capacity = self
            .options
            .stream_cache_frames
            .or_else(|| {
                env::var("LUMEN_STREAM_CACHE_FRAMES")
                    .ok()
                    .and_then(|value| value.parse::<usize>().ok())
            })
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_STREAM_CACHE_FRAMES);
        let mut assets =
            prepare_streaming_assets(&self.timeline, &media_root, stream_cache_capacity)?;

        let width = self.timeline.canvas.width;
        let height = self.timeline.canvas.height;
        let fps = self.timeline.timeline.fps;
        let total_frames = self.timeline.total_frames();

        let queue_capacity = self
            .options
            .encode_queue
            .or_else(|| {
                env::var("LUMEN_ENCODE_QUEUE")
                    .ok()
                    .and_then(|value| value.parse::<usize>().ok())
            })
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_ENCODE_QUEUE);

        let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(queue_capacity);
        let encoder = choose_video_encoder(self.options.video_encoder.as_deref());

        let encode_handle =
            thread::spawn(move || encode_rgba_stream(width, height, fps, encoder, rx));

        let mut renderer = create_renderer(width, height)?;

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

    fn decode_video_dependencies_for_frame(
        &self,
        frame: u64,
        assets: &mut StreamingAssets,
    ) -> anyhow::Result<()> {
        let total_frames = self.timeline.total_frames();
        if frame >= total_frames {
            return Err(anyhow!(
                "decode benchmark frame {frame} out of range (total={total_frames})"
            ));
        }

        let operation_indices = self
            .timeline
            .operation_indices_for_frame(frame)
            .map_err(|err| anyhow!(err.to_string()))?;

        for operation_index in operation_indices {
            let operation = self
                .timeline
                .operation(*operation_index)
                .ok_or_else(|| anyhow!("missing operation index {}", operation_index))?;

            if let CompiledOperationKind::Video(video) = &operation.kind {
                let source_frame = operation
                    .resolve_video_source_frame(frame)
                    .map_err(|err| anyhow!(err.to_string()))?;
                if let Some(source_frame) = source_frame {
                    let _ = assets
                        .video_frame(video.source_id.as_str(), source_frame)
                        .map_err(|err| {
                            anyhow!(
                                "failed to decode source `{}` frame {}: {err}",
                                video.source_id,
                                source_frame
                            )
                        })?;
                }
            }
        }

        Ok(())
    }

    /// Decode-only benchmark hook: decode all video dependencies for one frame
    /// without rendering/compositing or PNG encoding.
    pub fn benchmark_decode_only_frame(&mut self, frame: u64) -> anyhow::Result<()> {
        let media_root = media_root(self.options.media_root.as_deref())?;
        let stream_cache_capacity = self
            .options
            .stream_cache_frames
            .or_else(|| {
                env::var("LUMEN_STREAM_CACHE_FRAMES")
                    .ok()
                    .and_then(|value| value.parse::<usize>().ok())
            })
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_STREAM_CACHE_FRAMES);
        let mut assets =
            prepare_streaming_assets(&self.timeline, &media_root, stream_cache_capacity)?;
        self.decode_video_dependencies_for_frame(frame, &mut assets)
    }

    /// Decode-only benchmark hook for sequential timeline frames.
    pub fn benchmark_decode_only_sequential(&mut self, frames: u64) -> anyhow::Result<()> {
        let media_root = media_root(self.options.media_root.as_deref())?;
        let stream_cache_capacity = self
            .options
            .stream_cache_frames
            .or_else(|| {
                env::var("LUMEN_STREAM_CACHE_FRAMES")
                    .ok()
                    .and_then(|value| value.parse::<usize>().ok())
            })
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_STREAM_CACHE_FRAMES);
        let mut assets =
            prepare_streaming_assets(&self.timeline, &media_root, stream_cache_capacity)?;

        let count = frames.min(self.timeline.total_frames());
        for frame in 0..count {
            self.decode_video_dependencies_for_frame(frame, &mut assets)?;
        }

        Ok(())
    }

    /// Decode-only benchmark hook for arbitrary frame access patterns.
    pub fn benchmark_decode_only_random(&mut self, frames: &[u64]) -> anyhow::Result<()> {
        let media_root = media_root(self.options.media_root.as_deref())?;
        let stream_cache_capacity = self
            .options
            .stream_cache_frames
            .or_else(|| {
                env::var("LUMEN_STREAM_CACHE_FRAMES")
                    .ok()
                    .and_then(|value| value.parse::<usize>().ok())
            })
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_STREAM_CACHE_FRAMES);
        let mut assets =
            prepare_streaming_assets(&self.timeline, &media_root, stream_cache_capacity)?;

        for frame in frames {
            self.decode_video_dependencies_for_frame(*frame, &mut assets)?;
        }

        Ok(())
    }

    pub fn render_frame_png(&mut self, frame: u64) -> anyhow::Result<Vec<u8>> {
        let media_root = media_root(self.options.media_root.as_deref())?;
        let max_decoded_source_frames = self
            .options
            .max_decoded_source_frames
            .or_else(|| {
                env::var("LUMEN_MAX_DECODED_SOURCE_FRAMES")
                    .ok()
                    .and_then(|value| value.parse::<usize>().ok())
            })
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_MAX_DECODED_FRAMES);
        let requirements = collect_requirements(self.timeline.as_ref(), std::iter::once(frame))?;
        let mut assets = prepare_assets(
            self.timeline.as_ref(),
            &requirements,
            &media_root,
            max_decoded_source_frames,
        )?;

        let mut renderer =
            create_renderer(self.timeline.canvas.width, self.timeline.canvas.height)?;
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

// -- Streaming video decode ----------------------------------------------------

struct VideoStreamDecoder {
    child: Child,
    stdout: BufReader<ChildStdout>,
    next_source_frame: u64,
    width: u32,
    height: u32,
    frame_byte_size: usize,
    cache: LruCache<u64, FrameImage>,
    source: Source,
    fps: Rational,
    media_root: std::path::PathBuf,
}

impl VideoStreamDecoder {
    fn new(
        source: Source,
        fps: Rational,
        width: u32,
        height: u32,
        start_frame: u64,
        media_root: &Path,
        cache_capacity: usize,
    ) -> anyhow::Result<Self> {
        let frame_byte_size = frame_size(width, height)?;
        let cap = NonZeroUsize::new(cache_capacity.max(1))
            .ok_or_else(|| anyhow!("invalid cache capacity"))?;
        let (child, stdout) =
            Self::spawn_ffmpeg(&source, fps, width, height, start_frame, media_root)?;
        Ok(Self {
            child,
            stdout,
            next_source_frame: start_frame,
            width,
            height,
            frame_byte_size,
            cache: LruCache::new(cap),
            source,
            fps,
            media_root: media_root.to_path_buf(),
        })
    }

    fn spawn_ffmpeg(
        source: &Source,
        fps: Rational,
        _width: u32,
        _height: u32,
        start_frame: u64,
        media_root: &Path,
    ) -> anyhow::Result<(Child, BufReader<ChildStdout>)> {
        let mut command = Command::new("ffmpeg");
        command
            .arg("-hide_banner")
            .arg("-loglevel")
            .arg("error")
            .arg("-nostdin");

        match &source.kind {
            SourceKind::File { path, .. } => {
                let resolved = resolve_source_file_path(path, media_root)?;
                command.arg("-hwaccel").arg("auto").arg("-i").arg(resolved);
            }
            SourceKind::Generator { filter, .. } => {
                command.arg("-f").arg("lavfi").arg("-i").arg(filter);
            }
        }

        // Stream decode: use trim to start at start_frame, no end limit.
        command
            .arg("-an")
            .arg("-vf")
            .arg(format!(
                "fps={}/{},trim=start_frame={},setpts=PTS-STARTPTS",
                fps.num, fps.den, start_frame,
            ))
            .arg("-f")
            .arg("rawvideo")
            .arg("-pix_fmt")
            .arg("rgba")
            .arg("pipe:1")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = command
            .spawn()
            .context("failed to spawn ffmpeg streaming decode process")?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("ffmpeg streaming decode stdout was unavailable"))?;

        Ok((child, BufReader::new(stdout)))
    }

    fn get_frame(&mut self, source_frame: u64) -> anyhow::Result<Option<FrameImage>> {
        // Check LRU cache first.
        if let Some(frame) = self.cache.get(&source_frame) {
            return Ok(Some(frame.clone()));
        }

        // If we need to go backwards, restart ffmpeg from the target frame.
        if source_frame < self.next_source_frame {
            self.restart_from(source_frame)?;
        }

        // Read forward from ffmpeg, caching each intermediate frame.
        while self.next_source_frame <= source_frame {
            let mut buffer = vec![0u8; self.frame_byte_size];
            match self.stdout.read_exact(&mut buffer) {
                Ok(()) => {
                    let current = self.next_source_frame;
                    let frame = FrameImage::new(self.width, self.height, buffer)
                        .map_err(|err| anyhow!("decoded streaming frame was invalid: {err}"))?;
                    self.cache.put(current, frame);
                    self.next_source_frame = current.saturating_add(1);
                }
                Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => {
                    return Ok(None);
                }
                Err(err) => {
                    return Err(anyhow!("failed reading streaming video frame: {err}"));
                }
            }
        }

        Ok(self.cache.get(&source_frame).cloned())
    }

    fn restart_from(&mut self, start_frame: u64) -> anyhow::Result<()> {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let (child, stdout) = Self::spawn_ffmpeg(
            &self.source,
            self.fps,
            self.width,
            self.height,
            start_frame,
            &self.media_root,
        )?;
        self.child = child;
        self.stdout = stdout;
        self.next_source_frame = start_frame;
        Ok(())
    }
}

impl Drop for VideoStreamDecoder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct StreamingAssets {
    images: HashMap<String, FrameImage>,
    video_decoders: HashMap<String, VideoStreamDecoder>,
}

impl FrameProvider for StreamingAssets {
    fn image(&mut self, source_id: &str) -> Result<Option<FrameImage>, ProviderError> {
        Ok(self.images.get(source_id).cloned())
    }

    fn video_frame(
        &mut self,
        source_id: &str,
        source_frame: u64,
    ) -> Result<Option<FrameImage>, ProviderError> {
        let decoder = self
            .video_decoders
            .get_mut(source_id)
            .ok_or_else(|| ProviderError::MissingSource(source_id.to_string()))?;
        decoder
            .get_frame(source_frame)
            .map_err(|err| ProviderError::Decode(err.to_string()))
    }
}

fn prepare_streaming_assets(
    timeline: &CompiledTimeline,
    media_root: &Path,
    cache_capacity: usize,
) -> anyhow::Result<StreamingAssets> {
    let fps = timeline.timeline.fps;
    let mut images = HashMap::new();
    let mut video_decoders = HashMap::new();

    for source in timeline.sources() {
        match source.media_type() {
            SourceMediaType::Image => {
                let image = decode_image_source(source, media_root)?;
                images.insert(source.id.clone(), image);
            }
            SourceMediaType::Video => {
                let (width, height) = probe_video_dimensions(source, media_root)?;
                let decoder = VideoStreamDecoder::new(
                    source.clone(),
                    fps,
                    width,
                    height,
                    0,
                    media_root,
                    cache_capacity,
                )?;
                video_decoders.insert(source.id.clone(), decoder);
            }
            SourceMediaType::Audio => {}
        }
    }

    Ok(StreamingAssets {
        images,
        video_decoders,
    })
}

fn prepare_assets(
    timeline: &CompiledTimeline,
    requirements: &FrameRequirements,
    media_root: &Path,
    max_frames: usize,
) -> anyhow::Result<PreparedAssets> {
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
        let image = decode_image_source(source, media_root)?;
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
        let decode_root = media_root.to_path_buf();
        decode_handles.push(thread::spawn(move || {
            decode_video_source_frames(&source, fps, frame_set, &decode_root)
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

fn decode_video_source_frames(
    source: &Source,
    fps: Rational,
    requested_frames: BTreeSet<u64>,
    media_root: &Path,
) -> anyhow::Result<BTreeMap<u64, FrameImage>> {
    if requested_frames.is_empty() {
        return Ok(BTreeMap::new());
    }

    let (width, height) = probe_video_dimensions(source, media_root)?;
    let fsize = frame_size(width, height)?;
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

    let mut command = decode_command(source, fps, min_requested, max_requested, media_root)?;
    command.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = command
        .spawn()
        .context("failed to spawn ffmpeg decode process")?;

    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("ffmpeg decode stdout was unavailable"))?;

    let mut decoded = BTreeMap::new();
    let frame_count = max_requested
        .saturating_sub(min_requested)
        .saturating_add(1);
    for decoded_index in 0..frame_count {
        let frame_index = min_requested.saturating_add(decoded_index);
        let mut buffer = vec![0u8; fsize];
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
    media_root: &Path,
) -> anyhow::Result<Command> {
    let mut command = Command::new("ffmpeg");
    command
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-nostdin");

    match &source.kind {
        SourceKind::File { path, .. } => {
            let resolved = resolve_source_file_path(path, media_root)?;
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
        .arg(
            max_frame
                .saturating_sub(min_frame)
                .saturating_add(1)
                .to_string(),
        )
        .arg("-f")
        .arg("rawvideo")
        .arg("-pix_fmt")
        .arg("rgba")
        .arg("pipe:1");

    Ok(command)
}

fn probe_video_dimensions(source: &Source, media_root: &Path) -> anyhow::Result<(u32, u32)> {
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
            command.arg(resolve_source_file_path(path, media_root)?);
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
