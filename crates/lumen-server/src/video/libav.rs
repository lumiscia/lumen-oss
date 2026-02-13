use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    env,
    num::NonZeroUsize,
    path::Path,
    sync::{Arc, Once, mpsc},
    thread,
};

use anyhow::{Context, anyhow};
use ffmpeg_next::{self as ffmpeg, format, media, software::scaling};
use image::{ImageEncoder, codecs::png::PngEncoder};
use lru::LruCache;
use lumen::{
    backend::{FrameImage, FrameProvider, ProviderError},
    compile::CompiledTimeline,
    model::{Source, SourceKind, SourceMediaType},
    time::Rational,
};

use super::common::{
    DEFAULT_ENCODE_QUEUE, DEFAULT_MAX_DECODED_FRAMES,
    collect_requirements, choose_video_encoder, create_renderer, decode_image_source,
    encode_rgba_stream, media_root, resolve_source_file_path, FrameRequirements,
    PreparedAssets,
};

pub use super::common::RenderBackendOptions;

const DEFAULT_LIBAV_CACHE_FRAMES: usize = 64;

// ---------------------------------------------------------------------------
// ffmpeg global init
// ---------------------------------------------------------------------------

static FFMPEG_INIT: Once = Once::new();

fn ensure_ffmpeg_init() {
    FFMPEG_INIT.call_once(|| {
        ffmpeg::init().expect("failed to initialize ffmpeg");
        ffmpeg::log::set_level(ffmpeg::log::Level::Error);
    });
}

// ---------------------------------------------------------------------------
// Source input helpers
// ---------------------------------------------------------------------------

fn open_source_input(source: &Source, media_root: &Path) -> anyhow::Result<format::context::Input> {
    match &source.kind {
        SourceKind::File { path, .. } => {
            let resolved = resolve_source_file_path(path, media_root)?;
            format::input(&resolved)
                .with_context(|| format!("failed to open video file `{}`", resolved.display()))
        }
        SourceKind::Generator { filter, .. } => {
            let uri = format!("lavfi:{filter}");
            format::input(&uri)
                .with_context(|| format!("failed to open lavfi source `{filter}`"))
        }
    }
}

// ---------------------------------------------------------------------------
// LibavStreamDecoder
// ---------------------------------------------------------------------------

struct LibavStreamDecoder {
    input_ctx: format::context::Input,
    video_stream_index: usize,
    decoder: ffmpeg::codec::decoder::Video,
    scaler: scaling::Context,
    width: u32,
    height: u32,
    time_base: ffmpeg::Rational,
    timeline_fps: Rational,
    next_source_frame: u64,
    cache: LruCache<u64, FrameImage>,
    scratch_frame: ffmpeg::frame::Video,
    eof: bool,
}

/// The libav C types are `!Send` by default (raw pointers inside), but each
/// `LibavStreamDecoder` is created and used exclusively from a single thread
/// (the render loop or a dedicated decode thread). This mirrors the existing
/// `unsafe impl Send for SkiaRenderer` pattern.
unsafe impl Send for LibavStreamDecoder {}

impl LibavStreamDecoder {
    fn new(
        source: &Source,
        timeline_fps: Rational,
        start_frame: u64,
        media_root: &Path,
        cache_capacity: usize,
    ) -> anyhow::Result<Self> {
        ensure_ffmpeg_init();

        let input_ctx = open_source_input(source, media_root)?;
        let stream = input_ctx
            .streams()
            .best(media::Type::Video)
            .ok_or_else(|| anyhow!("no video stream found in source"))?;

        let video_stream_index = stream.index();
        let time_base = stream.time_base();

        let codec_params = stream.parameters();
        let mut decoder_ctx = ffmpeg::codec::context::Context::from_parameters(codec_params)
            .context("failed to create codec context")?;

        // Enable multi-threaded decoding when available.
        unsafe {
            (*decoder_ctx.as_mut_ptr()).thread_count = 0;
        }

        let decoder = decoder_ctx
            .decoder()
            .video()
            .context("failed to open video decoder")?;

        let width = decoder.width();
        let height = decoder.height();
        if width == 0 || height == 0 {
            return Err(anyhow!("video source has zero dimensions"));
        }

        let scaler = scaling::Context::get(
            decoder.format(),
            width,
            height,
            ffmpeg::format::Pixel::RGBA,
            width,
            height,
            scaling::Flags::BILINEAR,
        )
        .context("failed to create swscale context")?;

        let scratch_frame = ffmpeg::frame::Video::empty();

        let cap = NonZeroUsize::new(cache_capacity.max(1))
            .ok_or_else(|| anyhow!("invalid cache capacity"))?;

        let mut decoder_instance = Self {
            input_ctx,
            video_stream_index,
            decoder,
            scaler,
            width,
            height,
            time_base,
            timeline_fps,
            next_source_frame: 0,
            cache: LruCache::new(cap),
            scratch_frame,
            eof: false,
        };

        if start_frame > 0 {
            decoder_instance.seek_to_frame(start_frame)?;
        }

        Ok(decoder_instance)
    }

    // -- PTS math -----------------------------------------------------------

    /// Convert a source_frame index (at timeline_fps) to PTS in the source
    /// stream's time_base.
    fn source_frame_to_pts(&self, source_frame: u64) -> i64 {
        let timestamp_secs = source_frame as f64 / self.timeline_fps.as_f64();
        let pts = timestamp_secs * self.time_base.1 as f64 / self.time_base.0 as f64;
        pts.round() as i64
    }

    /// Convert a decoded frame's PTS to a source_frame index at timeline_fps.
    fn pts_to_source_frame(&self, pts: i64) -> u64 {
        let timestamp_secs = pts as f64 * self.time_base.0 as f64 / self.time_base.1 as f64;
        (timestamp_secs * self.timeline_fps.as_f64())
            .round()
            .max(0.0) as u64
    }

    // -- Core decode --------------------------------------------------------

    fn get_frame(&mut self, source_frame: u64) -> anyhow::Result<Option<FrameImage>> {
        // Check LRU cache first.
        if let Some(frame) = self.cache.get(&source_frame) {
            return Ok(Some(frame.clone()));
        }

        // If we need to go backwards, seek.
        if source_frame < self.next_source_frame {
            self.seek_to_frame(source_frame)?;
        }

        // Decode forward until we reach the target frame or EOF.
        while self.next_source_frame <= source_frame && !self.eof {
            match self.decode_next_frame()? {
                Some((frame_idx, image)) => {
                    self.cache.put(frame_idx, image);
                    if frame_idx >= source_frame {
                        break;
                    }
                }
                None => break,
            }
        }

        Ok(self.cache.get(&source_frame).cloned())
    }

    fn seek_to_frame(&mut self, target_frame: u64) -> anyhow::Result<()> {
        let target_pts = self.source_frame_to_pts(target_frame);

        // Seek to the nearest keyframe at or before target_pts.
        self.input_ctx
            .seek(target_pts, ..target_pts)
            .context("failed to seek in video stream")?;

        self.decoder.flush();
        self.eof = false;
        self.next_source_frame = target_frame;

        // Decode forward past any frames before target to prime the decoder.
        loop {
            match self.decode_next_raw()? {
                Some((idx, image)) => {
                    self.cache.put(idx, image);
                    if idx >= target_frame {
                        break;
                    }
                }
                None => break,
            }
        }

        Ok(())
    }

    fn decode_next_frame(&mut self) -> anyhow::Result<Option<(u64, FrameImage)>> {
        let result = self.decode_next_raw()?;
        if let Some((idx, _)) = &result {
            self.next_source_frame = idx.saturating_add(1);
        }
        Ok(result)
    }

    fn decode_next_raw(&mut self) -> anyhow::Result<Option<(u64, FrameImage)>> {
        let mut decoded = ffmpeg::frame::Video::empty();

        loop {
            // Try to receive a decoded frame.
            match self.decoder.receive_frame(&mut decoded) {
                Ok(()) => {
                    return self.convert_frame(&decoded);
                }
                Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::error::EAGAIN => {
                    // Need more data -- feed the next video packet.
                    if !self.feed_next_packet()? {
                        // No more packets. Signal EOF to drain.
                        self.decoder.send_eof().ok();
                        self.eof = true;

                        // Drain remaining frames.
                        match self.decoder.receive_frame(&mut decoded) {
                            Ok(()) => return self.convert_frame(&decoded),
                            Err(_) => return Ok(None),
                        }
                    }
                }
                Err(ffmpeg::Error::Eof) => {
                    self.eof = true;
                    return Ok(None);
                }
                Err(err) => return Err(anyhow!("video decode error: {err}")),
            }
        }
    }

    fn feed_next_packet(&mut self) -> anyhow::Result<bool> {
        loop {
            match self.input_ctx.packets().next() {
                Some((stream, packet)) => {
                    if stream.index() == self.video_stream_index {
                        self.decoder
                            .send_packet(&packet)
                            .context("failed to send packet to decoder")?;
                        return Ok(true);
                    }
                    // Skip non-video packets.
                }
                None => return Ok(false),
            }
        }
    }

    fn convert_frame(
        &mut self,
        decoded: &ffmpeg::frame::Video,
    ) -> anyhow::Result<Option<(u64, FrameImage)>> {
        let pts = match decoded.pts() {
            Some(pts) => pts,
            None => return Ok(None),
        };

        let source_frame = self.pts_to_source_frame(pts);

        // Scale/convert to RGBA.
        self.scaler
            .run(decoded, &mut self.scratch_frame)
            .context("swscale conversion failed")?;

        // Copy RGBA data, handling potential stride padding.
        let width = self.width as usize;
        let height = self.height as usize;
        let stride = self.scratch_frame.stride(0);
        let expected_row = width * 4;

        let rgba = if stride == expected_row {
            self.scratch_frame.data(0)[..expected_row * height].to_vec()
        } else {
            let mut buf = Vec::with_capacity(expected_row * height);
            for row in 0..height {
                let start = row * stride;
                buf.extend_from_slice(&self.scratch_frame.data(0)[start..start + expected_row]);
            }
            buf
        };

        let image = FrameImage::new(self.width, self.height, rgba)
            .map_err(|err| anyhow!("decoded frame was invalid: {err}"))?;

        Ok(Some((source_frame, image)))
    }
}

// ---------------------------------------------------------------------------
// StreamingAssets (FrameProvider for render_to_mp4)
// ---------------------------------------------------------------------------

struct StreamingAssets {
    images: HashMap<String, FrameImage>,
    video_decoders: HashMap<String, LibavStreamDecoder>,
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
    ensure_ffmpeg_init();
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
                let decoder = LibavStreamDecoder::new(
                    source,
                    fps,
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

// ---------------------------------------------------------------------------
// Batch decode (for render_frame_png)
// ---------------------------------------------------------------------------

fn decode_video_source_frames(
    source: &Source,
    timeline_fps: Rational,
    requested_frames: BTreeSet<u64>,
    media_root: &Path,
) -> anyhow::Result<BTreeMap<u64, FrameImage>> {
    if requested_frames.is_empty() {
        return Ok(BTreeMap::new());
    }

    ensure_ffmpeg_init();

    let min_requested = *requested_frames.iter().next().unwrap();
    let cache_size = requested_frames.len().max(16);

    let mut decoder = LibavStreamDecoder::new(
        source,
        timeline_fps,
        min_requested,
        media_root,
        cache_size,
    )?;

    let mut decoded = BTreeMap::new();
    for &frame in &requested_frames {
        match decoder.get_frame(frame)? {
            Some(image) => {
                decoded.insert(frame, image);
            }
            None => break,
        }
    }

    Ok(decoded)
}

// ---------------------------------------------------------------------------
// FfmpegRenderBackend (public API)
// ---------------------------------------------------------------------------

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

    pub fn render_to_mp4(&self, on_progress: &mut dyn FnMut(u64, u64)) -> anyhow::Result<Vec<u8>> {
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
            .unwrap_or(DEFAULT_LIBAV_CACHE_FRAMES);
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

    pub fn render_frame_png(&self, frame: u64) -> anyhow::Result<Vec<u8>> {
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
