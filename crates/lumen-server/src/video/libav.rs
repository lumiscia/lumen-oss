use std::{
    collections::HashMap,
    env,
    num::NonZeroUsize,
    path::Path,
    sync::{mpsc, Arc, OnceLock},
    thread,
};

use std::ffi::CString;
use std::ptr;

use anyhow::{anyhow, Context};
use ffmpeg_next::{self as ffmpeg, format, media, software::scaling};
use image::{codecs::png::PngEncoder, ImageEncoder};
use lru::LruCache;
use lumen::{
    backend::{FrameImage, FrameProvider, ProviderError, RenderBackend},
    compile::{CompiledOperationKind, CompiledTimeline},
    model::{Source, SourceKind, SourceMediaType},
    time::Rational,
};

use super::common::{
    choose_video_encoder, create_renderer, decode_image_source, encode_rgba_stream, media_root,
    resolve_source_file_path, DEFAULT_ENCODE_QUEUE,
};

pub use super::common::RenderBackendOptions;

const DEFAULT_LIBAV_CACHE_FRAMES: usize = 64;
const DEFAULT_LIBAV_PREFETCH_QUEUE: usize = 8;
const DEFAULT_LIBAV_PREFETCH_FRAMES: u64 = 4;

// ---------------------------------------------------------------------------
// ffmpeg global init
// ---------------------------------------------------------------------------

static FFMPEG_INIT: OnceLock<Result<(), String>> = OnceLock::new();

fn ensure_ffmpeg_init() -> anyhow::Result<()> {
    let init = FFMPEG_INIT.get_or_init(|| {
        ffmpeg::init()
            .map_err(|err| format!("failed to initialize ffmpeg: {err}"))
            .map(|_| {
                ffmpeg::log::set_level(ffmpeg::log::Level::Error);
            })
    });

    match init {
        Ok(()) => Ok(()),
        Err(err) => Err(anyhow!("{err}")),
    }
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
        SourceKind::Generator { filter, .. } => open_lavfi_input(filter)
            .with_context(|| format!("failed to open lavfi source `{filter}`")),
    }
}

fn open_video_decoder(
    codec_params: ffmpeg::codec::Parameters,
) -> anyhow::Result<ffmpeg::codec::decoder::Video> {
    let mut decoder_ctx = ffmpeg::codec::context::Context::from_parameters(codec_params)
        .context("failed to create codec context")?;

    // Leave count at 0 so ffmpeg selects a suitable thread count.
    let mut threading = ffmpeg::codec::threading::Config::default();
    threading.kind = ffmpeg::codec::threading::Type::Frame;
    threading.count = 0;
    decoder_ctx.set_threading(threading);

    decoder_ctx
        .decoder()
        .video()
        .context("failed to open video decoder")
}

/// Open a lavfi virtual input by explicitly specifying the lavfi demuxer.
///
/// `format::input("lavfi:<filter>")` only works when ffmpeg is built with the
/// lavfi protocol. When it isn't available (common on Homebrew builds), we
/// fall back to the raw `avformat_open_input` with `iformat` set to the lavfi
/// demuxer, which is always present if libavfilter is linked.
fn open_lavfi_input(filter: &str) -> Result<format::context::Input, ffmpeg::Error> {
    unsafe {
        let iformat = ffmpeg_next::ffi::av_find_input_format(b"lavfi\0".as_ptr() as *const _);
        if iformat.is_null() {
            return Err(ffmpeg::Error::DemuxerNotFound);
        }

        let path = CString::new(filter).map_err(|_| ffmpeg::Error::InvalidData)?;
        let mut ps = ptr::null_mut();

        match ffmpeg_next::ffi::avformat_open_input(
            &mut ps,
            path.as_ptr(),
            iformat,
            ptr::null_mut(),
        ) {
            0 => match ffmpeg_next::ffi::avformat_find_stream_info(ps, ptr::null_mut()) {
                r if r >= 0 => Ok(format::context::Input::wrap(ps)),
                e => {
                    ffmpeg_next::ffi::avformat_close_input(&mut ps);
                    Err(ffmpeg::Error::from(e))
                }
            },
            e => Err(ffmpeg::Error::from(e)),
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
    frame_byte_size: usize,
    time_base: ffmpeg::Rational,
    timeline_time_base: ffmpeg::Rational,
    next_source_frame: u64,
    cache: LruCache<u64, FrameImage>,
    buffer_pool: Vec<Vec<u8>>,
    decoded_frame: ffmpeg::frame::Video,
    scratch_frame: ffmpeg::frame::Video,
    packet: ffmpeg::Packet,
    eof: bool,
    draining: bool,
    last_decoded_source_frame: Option<u64>,
    last_decoded_image: Option<FrameImage>,
    source: Source,
    media_root: std::path::PathBuf,
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
        ensure_ffmpeg_init()?;

        let input_ctx = open_source_input(source, media_root)?;
        let stream = input_ctx
            .streams()
            .best(media::Type::Video)
            .ok_or_else(|| anyhow!("no video stream found in source"))?;

        let video_stream_index = stream.index();
        let time_base = stream.time_base();
        let timeline_time_base = ffmpeg::Rational::new(
            i32::try_from(timeline_fps.den).context("timeline fps denominator exceeded i32")?,
            i32::try_from(timeline_fps.num).context("timeline fps numerator exceeded i32")?,
        );

        let decoder = open_video_decoder(stream.parameters())?;

        let width = decoder.width();
        let height = decoder.height();
        if width == 0 || height == 0 {
            return Err(anyhow!("video source has zero dimensions"));
        }
        let frame_byte_size = (width as usize)
            .checked_mul(height as usize)
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or_else(|| anyhow!("video frame byte size overflow"))?;

        let scaler = scaling::Context::get(
            decoder.format(),
            width,
            height,
            ffmpeg::format::Pixel::RGBA,
            width,
            height,
            scaling::Flags::FAST_BILINEAR,
        )
        .context("failed to create swscale context")?;

        let decoded_frame = ffmpeg::frame::Video::empty();
        let scratch_frame = ffmpeg::frame::Video::empty();
        let packet = ffmpeg::Packet::empty();

        let cap = NonZeroUsize::new(cache_capacity.max(1))
            .ok_or_else(|| anyhow!("invalid cache capacity"))?;

        let mut decoder_instance = Self {
            input_ctx,
            video_stream_index,
            decoder,
            scaler,
            width,
            height,
            frame_byte_size,
            time_base,
            timeline_time_base,
            next_source_frame: 0,
            cache: LruCache::new(cap),
            buffer_pool: Vec::new(),
            decoded_frame,
            scratch_frame,
            packet,
            eof: false,
            draining: false,
            last_decoded_source_frame: None,
            last_decoded_image: None,
            source: source.clone(),
            media_root: media_root.to_path_buf(),
        };

        if start_frame > 0 {
            decoder_instance.skip_to_frame(start_frame)?;
        }

        Ok(decoder_instance)
    }

    // -- PTS math -----------------------------------------------------------

    /// Convert a source_frame index (at timeline_fps) to PTS in the source
    /// stream's time_base.
    fn source_frame_to_pts(&self, source_frame: u64) -> i64 {
        let timestamp_secs = source_frame as f64 * self.timeline_time_base.0 as f64
            / self.timeline_time_base.1 as f64;
        let pts = timestamp_secs * self.time_base.1 as f64 / self.time_base.0 as f64;
        pts.round() as i64
    }

    /// Convert a decoded frame's PTS to a source_frame index at timeline_fps.
    fn pts_to_source_frame(&self, pts: i64) -> u64 {
        let timestamp_secs = pts as f64 * self.time_base.0 as f64 / self.time_base.1 as f64;
        let source_frame =
            timestamp_secs * self.timeline_time_base.1 as f64 / self.timeline_time_base.0 as f64;
        source_frame.round().max(0.0) as u64
    }

    // -- Core decode --------------------------------------------------------

    fn get_frame(&mut self, source_frame: u64) -> anyhow::Result<Option<FrameImage>> {
        // Check LRU cache first.
        if let Some(frame) = self.cache.get(&source_frame) {
            return Ok(Some(frame.clone()));
        }

        // If we need to go backwards, seek or reopen.
        if source_frame < self.next_source_frame {
            self.seek_to_frame(source_frame)?;
        }

        // Decode forward until we reach the target frame or EOF.
        while self.next_source_frame <= source_frame && !self.eof {
            match self.decode_next_frame()? {
                Some((frame_idx, image)) => {
                    self.cache_decoded_frame(frame_idx, image);
                    if frame_idx >= source_frame {
                        break;
                    }
                }
                None => break,
            }
        }

        if let Some(frame) = self.cache.get(&source_frame) {
            return Ok(Some(frame.clone()));
        }

        Ok(self.nearest_cached_frame(source_frame))
    }

    fn cache_decoded_frame(&mut self, frame_idx: u64, image: FrameImage) {
        if let (Some(prev_idx), Some(prev_image)) = (
            self.last_decoded_source_frame,
            self.last_decoded_image.as_ref(),
        ) {
            if frame_idx > prev_idx.saturating_add(1) {
                // Fill gaps caused by source fps < timeline fps by holding the
                // previous frame. This avoids cache misses and fallback seeks.
                let held_frame = prev_image.clone();
                for gap_idx in prev_idx.saturating_add(1)..frame_idx {
                    self.cache_frame(gap_idx, held_frame.clone());
                }
            }
        }

        self.cache_frame(frame_idx, image.clone());
        self.last_decoded_source_frame = Some(frame_idx);
        if let Some(previous) = self.last_decoded_image.replace(image) {
            self.try_recycle_frame_buffer(previous);
        }
    }

    fn cache_frame(&mut self, frame_idx: u64, image: FrameImage) {
        if let Some(evicted) = self.cache.put(frame_idx, image) {
            self.try_recycle_frame_buffer(evicted);
        }
    }

    fn try_recycle_frame_buffer(&mut self, frame: FrameImage) {
        let Ok(mut rgba) = Arc::try_unwrap(frame.rgba) else {
            return;
        };
        if rgba.capacity() < self.frame_byte_size {
            return;
        }
        rgba.clear();
        self.buffer_pool.push(rgba);
    }

    fn take_reusable_buffer(&mut self) -> Vec<u8> {
        if let Some(mut buffer) = self.buffer_pool.pop() {
            buffer.clear();
            buffer
        } else {
            Vec::with_capacity(self.frame_byte_size)
        }
    }

    fn nearest_cached_frame(&self, source_frame: u64) -> Option<FrameImage> {
        let mut prev: Option<(u64, &FrameImage)> = None;
        let mut next: Option<(u64, &FrameImage)> = None;

        for (frame_idx, frame) in self.cache.iter() {
            let frame_idx = *frame_idx;

            if frame_idx <= source_frame {
                if prev
                    .as_ref()
                    .is_none_or(|(best_idx, _)| frame_idx > *best_idx)
                {
                    prev = Some((frame_idx, frame));
                }
            } else if next
                .as_ref()
                .is_none_or(|(best_idx, _)| frame_idx < *best_idx)
            {
                next = Some((frame_idx, frame));
            }
        }

        prev.map(|(_, frame)| frame.clone())
            .or_else(|| next.map(|(_, frame)| frame.clone()))
    }

    fn seek_to_frame(&mut self, target_frame: u64) -> anyhow::Result<()> {
        let target_pts = self.source_frame_to_pts(target_frame);

        // Try a keyframe seek. This fails for non-seekable sources (e.g. lavfi
        // generators), in which case we reopen and decode forward.
        let seeked = self.input_ctx.seek(target_pts, ..target_pts).is_ok();

        if seeked {
            self.decoder.flush();
            self.eof = false;
            self.draining = false;
            self.next_source_frame = 0;
            self.last_decoded_source_frame = None;
            if let Some(previous) = self.last_decoded_image.take() {
                self.try_recycle_frame_buffer(previous);
            }

            // Decode forward past any pre-target frames from the keyframe.
            loop {
                match self.decode_next_raw()? {
                    Some((idx, image)) => {
                        self.cache_decoded_frame(idx, image);
                        self.next_source_frame = idx.saturating_add(1);
                        if idx >= target_frame {
                            break;
                        }
                    }
                    None => break,
                }
            }
        } else {
            // Non-seekable source: reopen from scratch and decode forward.
            self.reopen_and_skip(target_frame)?;
        }

        Ok(())
    }

    /// Reopen the source from scratch and decode forward to `target_frame`.
    /// Used when the demuxer doesn't support seeking (e.g. lavfi generators).
    fn reopen_and_skip(&mut self, target_frame: u64) -> anyhow::Result<()> {
        let input_ctx = open_source_input(&self.source, &self.media_root)?;
        let (video_stream_index, time_base, decoder) = {
            let stream = input_ctx
                .streams()
                .best(media::Type::Video)
                .ok_or_else(|| anyhow!("no video stream found on reopen"))?;

            (
                stream.index(),
                stream.time_base(),
                open_video_decoder(stream.parameters())?,
            )
        };

        self.input_ctx = input_ctx;
        self.video_stream_index = video_stream_index;
        self.time_base = time_base;
        self.decoder = decoder;
        self.eof = false;
        self.draining = false;
        self.next_source_frame = 0;
        self.last_decoded_source_frame = None;
        if let Some(previous) = self.last_decoded_image.take() {
            self.try_recycle_frame_buffer(previous);
        }

        // Decode forward to target, caching along the way.
        self.skip_to_frame(target_frame)?;
        Ok(())
    }

    /// Decode forward from current position to `target_frame`, caching frames.
    fn skip_to_frame(&mut self, target_frame: u64) -> anyhow::Result<()> {
        while self.next_source_frame <= target_frame && !self.eof {
            match self.decode_next_frame()? {
                Some((idx, image)) => {
                    self.cache_decoded_frame(idx, image);
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
        loop {
            // Try to receive a decoded frame (reusing self.decoded_frame).
            match self.decoder.receive_frame(&mut self.decoded_frame) {
                Ok(()) => {
                    if let Some(decoded) = self.convert_decoded()? {
                        return Ok(Some(decoded));
                    }
                    // Some streams omit PTS on a subset of frames. Skip those.
                    continue;
                }
                Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::error::EAGAIN => {
                    if self.draining {
                        self.eof = true;
                        return Ok(None);
                    }

                    // Need more data -- feed the next video packet.
                    if !self.feed_next_packet()? {
                        // No more packets. Signal EOF once and keep draining
                        // receive_frame() until it reports EOF.
                        self.decoder
                            .send_eof()
                            .context("failed to send decoder EOF")?;
                        self.draining = true;
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
            match self.packet.read(&mut self.input_ctx) {
                Ok(()) => {
                    if self.packet.stream() != self.video_stream_index {
                        continue;
                    }

                    self.decoder
                        .send_packet(&self.packet)
                        .context("failed to send packet to decoder")?;
                    return Ok(true);
                }
                Err(ffmpeg::Error::Eof) => return Ok(false),
                Err(err) => return Err(anyhow!("failed to read input packet: {err}")),
            }
        }
    }

    /// Scale self.decoded_frame → self.scratch_frame (RGBA) and copy out.
    fn convert_decoded(&mut self) -> anyhow::Result<Option<(u64, FrameImage)>> {
        let pts = match self.decoded_frame.pts() {
            Some(pts) => pts,
            None => return Ok(None),
        };

        let source_frame = self.pts_to_source_frame(pts);

        // Scale/convert to RGBA.
        self.scaler
            .run(&self.decoded_frame, &mut self.scratch_frame)
            .context("swscale conversion failed")?;

        // Copy RGBA data, handling potential stride padding.
        let width = self.width as usize;
        let height = self.height as usize;
        let stride = self.scratch_frame.stride(0);
        let expected_row = width * 4;

        let mut rgba = self.take_reusable_buffer();
        if stride == expected_row {
            rgba.extend_from_slice(&self.scratch_frame.data(0)[..expected_row * height]);
        } else {
            for row in 0..height {
                let start = row * stride;
                rgba.extend_from_slice(&self.scratch_frame.data(0)[start..start + expected_row]);
            }
        }

        let image = FrameImage::new(self.width, self.height, rgba)
            .map_err(|err| anyhow!("decoded frame was invalid: {err}"))?;

        Ok(Some((source_frame, image)))
    }
}

// ---------------------------------------------------------------------------
// StreamingAssets (FrameProvider for render_to_mp4 and render_frame_png)
// ---------------------------------------------------------------------------

struct StreamingAssets {
    images: HashMap<String, FrameImage>,
    video_workers: HashMap<String, VideoDecodeWorker>,
}

struct DecodeRequest {
    source_frame: u64,
    reply: mpsc::SyncSender<anyhow::Result<Option<FrameImage>>>,
}

struct VideoDecodeWorker {
    source_id: String,
    tx: Option<mpsc::SyncSender<DecodeRequest>>,
    handle: Option<thread::JoinHandle<()>>,
}

impl VideoDecodeWorker {
    fn spawn(
        source_id: &str,
        decoder: LibavStreamDecoder,
        request_queue_capacity: usize,
        prefetch_frames: u64,
    ) -> Self {
        let capacity = request_queue_capacity.max(1);
        let (tx, rx) = mpsc::sync_channel::<DecodeRequest>(capacity);
        let source_id_owned = source_id.to_string();
        let worker_source_id = source_id_owned.clone();
        let handle = thread::spawn(move || {
            run_decode_worker(worker_source_id, decoder, rx, prefetch_frames)
        });

        Self {
            source_id: source_id_owned,
            tx: Some(tx),
            handle: Some(handle),
        }
    }

    fn get_frame(&self, source_frame: u64) -> anyhow::Result<Option<FrameImage>> {
        let tx = self
            .tx
            .as_ref()
            .ok_or_else(|| anyhow!("decode worker for `{}` was shut down", self.source_id))?;
        let (reply_tx, reply_rx) = mpsc::sync_channel::<anyhow::Result<Option<FrameImage>>>(1);
        tx.send(DecodeRequest {
            source_frame,
            reply: reply_tx,
        })
        .map_err(|_| anyhow!("decode worker for `{}` is unavailable", self.source_id))?;
        reply_rx
            .recv()
            .map_err(|_| anyhow!("decode worker for `{}` dropped response", self.source_id))?
    }
}

impl Drop for VideoDecodeWorker {
    fn drop(&mut self) {
        self.tx.take();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn run_decode_worker(
    source_id: String,
    mut decoder: LibavStreamDecoder,
    rx: mpsc::Receiver<DecodeRequest>,
    prefetch_frames: u64,
) {
    let mut last_requested: Option<u64> = None;

    while let Ok(request) = rx.recv() {
        let frame = request.source_frame;
        let result = decoder
            .get_frame(frame)
            .map_err(|err| anyhow!("failed to decode source `{source_id}` frame {frame}: {err}"));

        let should_prefetch = prefetch_frames > 0
            && matches!(result, Ok(Some(_)))
            && last_requested.is_some_and(|last| frame == last.saturating_add(1));

        let _ = request.reply.send(result);

        if should_prefetch {
            for step in 1..=prefetch_frames {
                let next = frame.saturating_add(step);
                match decoder.get_frame(next) {
                    Ok(Some(_)) => {}
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
        }

        last_requested = Some(frame);
    }
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
        let worker = self
            .video_workers
            .get_mut(source_id)
            .ok_or_else(|| ProviderError::MissingSource(source_id.to_string()))?;
        worker
            .get_frame(source_frame)
            .map_err(|err| ProviderError::Decode(err.to_string()))
    }
}

fn prepare_streaming_assets(
    timeline: &CompiledTimeline,
    media_root: &Path,
    cache_capacity: usize,
    request_queue_capacity: usize,
    prefetch_frames: u64,
) -> anyhow::Result<StreamingAssets> {
    ensure_ffmpeg_init()?;
    let fps = timeline.timeline.fps;
    let mut images = HashMap::new();
    let mut video_workers = HashMap::new();

    for source in timeline.sources() {
        match source.media_type() {
            SourceMediaType::Image => {
                let image = decode_image_source(source, media_root)?;
                images.insert(source.id.clone(), image);
            }
            SourceMediaType::Video => {
                let decoder = LibavStreamDecoder::new(source, fps, 0, media_root, cache_capacity)?;
                let worker = VideoDecodeWorker::spawn(
                    source.id.as_str(),
                    decoder,
                    request_queue_capacity,
                    prefetch_frames,
                );
                video_workers.insert(source.id.clone(), worker);
            }
            SourceMediaType::Audio => {}
        }
    }

    Ok(StreamingAssets {
        images,
        video_workers,
    })
}

// ---------------------------------------------------------------------------
// FfmpegRenderBackend (public API)
// ---------------------------------------------------------------------------

pub struct FfmpegRenderBackend {
    timeline: Arc<CompiledTimeline>,
    options: RenderBackendOptions,
    renderer: Option<Box<dyn RenderBackend>>,
    assets: Option<StreamingAssets>,
}

impl FfmpegRenderBackend {
    pub fn new(timeline: Arc<CompiledTimeline>) -> Self {
        Self {
            timeline,
            options: RenderBackendOptions::default(),
            renderer: None,
            assets: None,
        }
    }

    pub fn new_with_options(
        timeline: Arc<CompiledTimeline>,
        options: RenderBackendOptions,
    ) -> Self {
        Self {
            timeline,
            options,
            renderer: None,
            assets: None,
        }
    }

    fn init_if_needed(&mut self) -> anyhow::Result<()> {
        if self.renderer.is_none() {
            let renderer =
                create_renderer(self.timeline.canvas.width, self.timeline.canvas.height)?;
            self.renderer = Some(renderer);
        }
        if self.assets.is_none() {
            let root = media_root(self.options.media_root.as_deref())?;
            let cache_capacity = self
                .options
                .stream_cache_frames
                .or_else(|| {
                    env::var("LUMEN_STREAM_CACHE_FRAMES")
                        .ok()
                        .and_then(|value| value.parse::<usize>().ok())
                })
                .filter(|value| *value > 0)
                .unwrap_or(DEFAULT_LIBAV_CACHE_FRAMES);
            let request_queue_capacity = env::var("LUMEN_LIBAV_PREFETCH_QUEUE")
                .ok()
                .and_then(|value| value.parse::<usize>().ok())
                .filter(|value| *value > 0)
                .unwrap_or(DEFAULT_LIBAV_PREFETCH_QUEUE);
            let prefetch_frames = env::var("LUMEN_LIBAV_PREFETCH_FRAMES")
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(DEFAULT_LIBAV_PREFETCH_FRAMES);

            let assets = prepare_streaming_assets(
                &self.timeline,
                &root,
                cache_capacity,
                request_queue_capacity,
                prefetch_frames,
            )?;
            self.assets = Some(assets);
        }
        Ok(())
    }

    fn decode_video_dependencies_for_frame(&mut self, frame: u64) -> anyhow::Result<()> {
        self.init_if_needed()?;

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

        let assets = self
            .assets
            .as_mut()
            .ok_or_else(|| anyhow!("streaming assets were not initialized"))?;

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
        self.decode_video_dependencies_for_frame(frame)
    }

    /// Decode-only benchmark hook for sequential timeline frames.
    pub fn benchmark_decode_only_sequential(&mut self, frames: u64) -> anyhow::Result<()> {
        let count = frames.min(self.timeline.total_frames());
        for frame in 0..count {
            self.decode_video_dependencies_for_frame(frame)?;
        }
        Ok(())
    }

    /// Decode-only benchmark hook for arbitrary frame access patterns.
    pub fn benchmark_decode_only_random(&mut self, frames: &[u64]) -> anyhow::Result<()> {
        for frame in frames {
            self.decode_video_dependencies_for_frame(*frame)?;
        }
        Ok(())
    }

    pub fn render_to_mp4(
        &mut self,
        on_progress: &mut dyn FnMut(u64, u64),
    ) -> anyhow::Result<Vec<u8>> {
        self.init_if_needed()?;

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

        let renderer = self
            .renderer
            .as_mut()
            .ok_or_else(|| anyhow!("renderer was not initialized"))?;
        let assets = self
            .assets
            .as_mut()
            .ok_or_else(|| anyhow!("streaming assets were not initialized"))?;

        for frame in 0..total_frames {
            let rgba = renderer
                .render_frame(self.timeline.as_ref(), frame, assets)
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

    pub fn render_frame_png(&mut self, frame: u64) -> anyhow::Result<Vec<u8>> {
        self.init_if_needed()?;

        let renderer = self
            .renderer
            .as_mut()
            .ok_or_else(|| anyhow!("renderer was not initialized"))?;
        let assets = self
            .assets
            .as_mut()
            .ok_or_else(|| anyhow!("streaming assets were not initialized"))?;

        let rgba = renderer
            .render_frame(self.timeline.as_ref(), frame, assets)
            .map_err(|err| anyhow!("failed to render preview frame {frame}: {err}"))?;

        let width = self.timeline.canvas.width;
        let height = self.timeline.canvas.height;
        let mut png = Vec::new();
        let encoder = PngEncoder::new(&mut png);
        encoder
            .write_image(&rgba, width, height, image::ExtendedColorType::Rgba8)
            .context("failed to encode preview PNG")?;
        Ok(png)
    }
}
