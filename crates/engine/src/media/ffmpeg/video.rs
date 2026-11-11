use std::{
    collections::{BTreeSet, HashSet},
    sync::{Arc, Mutex, mpsc},
    thread,
};

use lumen_ffmpeg::{
    CpuVideoFrame, DecodeMode, DecodedVideoFrame, GpuBackend, InputContext, VideoDecoder,
    VideoDecoderConfig, VideoStreamInfo, YuvSubsampling, YuvVideoFrame,
};

use crate::{
    error::MediaError,
    media::{
        AssetKind, ChromaSubsampling, CpuMediaFrame, GpuVideoMediaFrame, MediaFrame, Rotation,
        VideoFrameResolver, VideoMetadata, YuvColor, YuvFormat, YuvFrame, YuvLayout, YuvMatrix,
        resolver::{CpuRetry, PixelOrigin, SettledOrigin},
    },
};

use super::{
    ffmpeg_failure,
    image::{FrameLruCache, media_frame_bytes},
    open_failure,
};

const SEEK_REOPEN_THRESHOLD: u32 = 120;
/// Seeks to an earlier keyframe after one that landed past its target, before reopening the input
/// at the start.
const SEEK_RETRIES: u8 = 2;

#[derive(Debug, Clone, Default)]
pub struct FfmpegResolverOptions {
    pub prefer_hardware_decode: bool,
    /// The renderer's device, for hardware decode straight to its textures. Without it, Linux
    /// decodes on the CPU.
    #[cfg(all(target_os = "linux", feature = "vulkan"))]
    pub vulkan_device: Option<lumen_ffmpeg::VulkanDevice>,
}

impl FfmpegResolverOptions {
    pub fn with_hardware_decode(prefer_hardware_decode: bool) -> Self {
        Self {
            prefer_hardware_decode,
            #[cfg(all(target_os = "linux", feature = "vulkan"))]
            vulkan_device: None,
        }
    }
}

struct VideoDecodeWorker {
    decoder: LibavStreamDecoder,
    cache: FrameLruCache,
}

impl VideoDecodeWorker {
    fn new(decoder: LibavStreamDecoder) -> Self {
        Self {
            decoder,
            cache: FrameLruCache::default(),
        }
    }

    fn resolve_frame(&mut self, frame: u32) -> Result<MediaFrame, MediaError> {
        if let Some(cached) = self.cache.get(frame) {
            return Ok(cached);
        }

        let decoded = self.decoder.decode_frame(frame, &mut self.cache)?;
        self.cache.insert(frame, decoded.clone(), false);
        Ok(decoded)
    }

    fn has_room_to_prefetch(&self) -> bool {
        self.cache.has_room_for(self.decoder.frame_bytes())
    }

    /// Moves the source to the CPU, warning unless it is already there, and decodes `frame` on it.
    /// Fails if a GPU frame of this source has already reached a texture.
    fn resolve_on_cpu(&mut self, frame: u32, reason: &str) -> Result<MediaFrame, MediaError> {
        if !self.decoder.settle_on_cpu() {
            return Err(self.decoder.mid_render_failure(frame, reason));
        }
        if matches!(
            self.decoder.gpu_decode_status,
            GpuDecodeStatus::Active { .. }
        ) {
            tracing::warn!(
                source = self.decoder.source,
                reason,
                "hardware decode failed, decoding this source on the CPU"
            );
            self.decoder.move_to_cpu(&mut self.cache)?;
        }
        // A reopen that found the hardware decoder gone may have left GPU frames cached.
        self.cache.retain(&[]);
        self.resolve_frame(frame)
    }

    fn prefetch_frame(&mut self, frame: u32) {
        if !self.cache.contains(frame)
            && let Ok(decoded) = self.decoder.decode_frame(frame, &mut self.cache)
        {
            self.cache.insert(frame, decoded, true);
        }
    }

    fn retain_frames(&mut self, frames: &[u32]) {
        self.cache.retain(frames);
    }
}

struct LibavStreamDecoder {
    source: String,
    options: FfmpegResolverOptions,
    /// Reaches the decode worker that owns this decoder from the GPU frames it returns.
    worker: Option<mpsc::Sender<WorkerRequest>>,
    stream_index: usize,
    width: u32,
    height: u32,
    /// The size of the last frame decoded on the CPU, which sets how many the cache has room to
    /// prefetch.
    last_frame_bytes: usize,
    frame_count: u32,
    fps: f64,
    time_base_seconds: f64,
    start_time: i64,
    input: InputContext,
    /// `None` between releasing a failed hardware decoder and opening its CPU replacement, and after
    /// that replacement failed to open.
    decoder: Option<VideoDecoder>,
    next_frame_hint: u32,
    /// Set by a seek until its first frame arrives.
    seek: Option<PendingSeek>,
    sent_eof: bool,
    gpu_decode_status: GpuDecodeStatus,
    origin: SettledOrigin,
    /// A decode failure after GPU frames were drawn. The stream can't continue from a lost packet
    /// and the CPU would change the pixels, so every later decode fails the same way.
    mid_render_failure: Option<MediaError>,
}

/// A seek lands on a keyframe by the container's index, which for MP4 is by decode time. An open
/// GOP's keyframe decodes before the frames shown just ahead of it, and a decoder starting there
/// drops those frames, so its first frame can come after the one asked for.
struct PendingSeek {
    /// The earliest timestamp of the first packet read after the seek.
    landed: Option<i64>,
    retries: u8,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
enum GpuDecodeStatus {
    NotRequested,
    Active { backend: GpuBackend },
    Unavailable { backend: GpuBackend, reason: String },
}

impl GpuDecodeStatus {
    fn label(&self) -> &'static str {
        match self {
            Self::NotRequested => "cpu_not_requested",
            Self::Active {
                backend: GpuBackend::Metal,
            } => "gpu_metal",
            Self::Active { .. } => "gpu_vulkan",
            Self::Unavailable {
                backend: GpuBackend::Metal,
                ..
            } => "cpu_metal_unavailable",
            Self::Unavailable { .. } => "cpu_vulkan_unavailable",
        }
    }

    fn unavailable_reason(&self) -> Option<&str> {
        match self {
            Self::Unavailable { reason, .. } => Some(reason.as_str()),
            _ => None,
        }
    }
}

impl LibavStreamDecoder {
    /// Opens the source, and clears `prefer_hardware_decode` if the hardware decoder won't open, so
    /// the source stays on the CPU and warns once.
    fn open(
        source: impl Into<String>,
        mut options: FfmpegResolverOptions,
    ) -> Result<Self, MediaError> {
        let source = source.into();
        let mut input = open_input(&source)?;
        let stream = input
            .best_video_stream()
            .map_err(|err| open_failure(&source, AssetKind::Video, &err))?;
        let stream_index = stream.stream_index;
        let time_base_seconds = stream
            .time_base
            .as_f64()
            .filter(|seconds| *seconds > 0.0)
            .ok_or_else(|| {
                MediaError::decode(&source, "stream has no valid time base".to_string())
            })?;
        let fps = resolve_stream_fps(&stream, &source)?;
        let frame_count = match resolve_frame_count(&stream, input.presented_duration_us(), fps) {
            Some(frame_count) => Some(frame_count),
            None if input.is_image() => {
                let frame_count = count_frames(&mut input, stream_index);
                // An image packet can hold the whole file, so the spent input goes before the next
                // one reads it again.
                drop(input);
                input = open_input(&source)?;
                frame_count
            }
            None => None,
        }
        .ok_or_else(|| {
            MediaError::decode(
                &source,
                "unable to determine stream frame count".to_string(),
            )
        })?;
        let (decoder, gpu_decode_status) = open_video_decoder(&source, &input, &stream, &options)?;
        if let Some(reason) = gpu_decode_status.unavailable_reason() {
            tracing::warn!(reason, "hardware decode unavailable, decoding on the CPU");
            options.prefer_hardware_decode = false;
        }
        if decoder.drops_alpha() {
            tracing::warn!(
                source,
                "this FFmpeg has no libvpx decoder, so the video's alpha layer is dropped and it renders opaque"
            );
        }

        Ok(Self {
            source,
            options,
            worker: None,
            stream_index,
            width: stream.width,
            height: stream.height,
            last_frame_bytes: stream.width as usize * stream.height as usize * 4,
            frame_count,
            fps,
            time_base_seconds,
            start_time: stream.start_time,
            input,
            decoder: Some(decoder),
            next_frame_hint: 0,
            seek: None,
            sent_eof: false,
            gpu_decode_status,
            origin: SettledOrigin::default(),
            mid_render_failure: None,
        })
    }

    /// Returns whether the source can move to the CPU, and from then on its GPU frames stop reaching
    /// textures.
    fn settle_on_cpu(&self) -> bool {
        *self.origin.get_or_init(|| PixelOrigin::Cpu) == PixelOrigin::Cpu
    }

    fn mid_render_failure(&self, frame: u32, cause: &str) -> MediaError {
        MediaError::decode(
            &self.source,
            format!(
                "hardware decode failed at source frame {frame}, after earlier frames were drawn \
                 from the GPU. Finishing on the CPU would change this source's pixels partway \
                 through the render, so the render stops. Retry it, or set \
                 LUMEN_HARDWARE_DECODE=0 to decode on the CPU from the start: {cause}"
            ),
        )
    }

    fn metadata(&self) -> VideoMetadata {
        VideoMetadata {
            width: self.width,
            height: self.height,
            frame_count: self.frame_count,
            fps: self.fps as f32,
        }
    }

    fn decode_frame(
        &mut self,
        frame: u32,
        cache: &mut FrameLruCache,
    ) -> Result<MediaFrame, MediaError> {
        if frame >= self.frame_count {
            return Err(MediaError::FrameOutOfRange {
                media_source: self.source.clone(),
                frame,
                frame_count: self.frame_count,
            });
        }

        if let Some(failure) = &self.mid_render_failure {
            return Err(failure.clone());
        }

        if self.should_seek(frame) {
            self.seek_to_frame(frame)?;
        }

        match self.decode_frame_inner(frame) {
            Ok(decoded) => Ok(decoded),
            Err(primary_error) => {
                let hardware_failed =
                    matches!(self.gpu_decode_status, GpuDecodeStatus::Active { .. })
                        && !matches!(primary_error, MediaError::FrameOutOfRange { .. });
                if hardware_failed && !self.settle_on_cpu() {
                    let failure = self
                        .mid_render_failure(frame, &primary_error.to_string())
                        .caused_by(&primary_error);
                    self.mid_render_failure = Some(failure.clone());
                    return Err(failure);
                }
                if hardware_failed {
                    tracing::warn!(
                        source = self.source,
                        error = %primary_error,
                        "hardware decode failed, decoding this source on the CPU"
                    );
                    self.move_to_cpu(cache)?;
                } else {
                    self.reopen()?;
                }
                self.seek_to_frame(frame)?;
                self.decode_frame_inner(frame).map_err(|fallback_error| {
                    if hardware_failed
                        || matches!(fallback_error, MediaError::FrameOutOfRange { .. })
                    {
                        fallback_error
                    } else {
                        primary_error
                    }
                })
            }
        }
    }

    fn decode_frame_inner(&mut self, target_frame: u32) -> Result<MediaFrame, MediaError> {
        loop {
            if let Some(decoded) = self.receive_frames_until(target_frame)? {
                return Ok(decoded);
            }

            if self.sent_eof {
                return Err(MediaError::FrameOutOfRange {
                    media_source: self.source.clone(),
                    frame: target_frame,
                    frame_count: self.frame_count,
                });
            }

            match self.input.read_packet() {
                Ok(Some(packet)) => {
                    if let Some(seek) = &mut self.seek
                        && seek.landed.is_none()
                        && packet.stream_index() == self.stream_index
                    {
                        seek.landed = [packet.pts(), packet.dts()].into_iter().flatten().min();
                    }
                    self.open_decoder()?.send_packet(&packet).map_err(|err| {
                        ffmpeg_failure(
                            &self.source,
                            AssetKind::Video,
                            "failed sending packet to decoder",
                            &err,
                        )
                    })?;
                }
                Ok(None) => {
                    self.open_decoder()?.send_eof().map_err(|err| {
                        ffmpeg_failure(
                            &self.source,
                            AssetKind::Video,
                            "failed sending decoder EOF",
                            &err,
                        )
                    })?;
                    self.sent_eof = true;
                }
                Err(err) => {
                    return Err(ffmpeg_failure(
                        &self.source,
                        AssetKind::Video,
                        "packet read failed",
                        &err,
                    ));
                }
            }
        }
    }

    fn receive_frames_until(
        &mut self,
        target_frame: u32,
    ) -> Result<Option<MediaFrame>, MediaError> {
        loop {
            if matches!(self.gpu_decode_status, GpuDecodeStatus::Active { .. }) {
                match self.open_decoder()?.receive_gpu_frame() {
                    Ok(Some(decoded)) => {
                        let decoded_frame = self.frame_for_pts(decoded.pts());
                        if let Some(seek) = self.seek.take()
                            && decoded_frame > target_frame
                        {
                            // `GpuVideoFrame` only has drop glue when a hardware backend is enabled.
                            #[allow(clippy::drop_non_drop)]
                            drop(decoded);
                            self.seek_before_landing(target_frame, seek)?;
                            continue;
                        }
                        self.next_frame_hint = decoded_frame.saturating_add(1);
                        if decoded_frame < target_frame {
                            continue;
                        }
                        return Ok(Some(MediaFrame::GpuVideo(Arc::new(GpuVideoMediaFrame {
                            frame: decoded,
                            cpu_retry: self.cpu_retry(target_frame),
                            origin: Arc::clone(&self.origin),
                        }))));
                    }
                    Ok(None) => return Ok(None),
                    Err(err) => {
                        return Err(ffmpeg_failure(
                            &self.source,
                            AssetKind::Video,
                            "failed receiving decoded GPU frame",
                            &err,
                        ));
                    }
                }
            } else {
                match self.open_decoder()?.receive_video_frame() {
                    Ok(Some(decoded)) => {
                        let decoded_frame = self.frame_for_pts(decoded.pts());
                        if let Some(seek) = self.seek.take()
                            && decoded_frame > target_frame
                        {
                            drop(decoded);
                            self.seek_before_landing(target_frame, seek)?;
                            continue;
                        }
                        self.next_frame_hint = decoded_frame.saturating_add(1);
                        if decoded_frame < target_frame {
                            continue;
                        }
                        let frame = self.cpu_media_frame(decoded)?;
                        self.last_frame_bytes = media_frame_bytes(&frame);
                        return Ok(Some(frame));
                    }
                    Ok(None) => return Ok(None),
                    Err(err) => {
                        return Err(ffmpeg_failure(
                            &self.source,
                            AssetKind::Video,
                            "failed receiving decoded frame",
                            &err,
                        ));
                    }
                }
            }
        }
    }

    fn cpu_retry(&self, frame: u32) -> CpuRetry {
        let source = self.source.clone();
        let worker = self.worker.clone();
        Box::new(move |reason| {
            let unavailable =
                || MediaError::decode(&source, "video decode worker is unavailable".to_string());
            let worker = worker.as_ref().ok_or_else(unavailable)?;
            let (response_tx, response_rx) = mpsc::channel();
            worker
                .send(WorkerRequest::ResolveOnCpu {
                    frame,
                    reason: reason.to_string(),
                    response_tx,
                })
                .map_err(|_| unavailable())?;
            response_rx.recv().map_err(|_| unavailable())?
        })
    }

    fn frame_for_pts(&self, pts: Option<i64>) -> u32 {
        pts.map(|timestamp| self.timestamp_to_frame(timestamp))
            .filter(|frame| *frame <= self.frame_count.saturating_sub(1))
            .unwrap_or(self.next_frame_hint)
    }

    fn cpu_media_frame(&self, decoded: DecodedVideoFrame) -> Result<MediaFrame, MediaError> {
        match decoded {
            DecodedVideoFrame::Rgba { frame, has_alpha } => {
                let rgba = self.rgba_media_frame(frame)?;
                Ok(if has_alpha {
                    MediaFrame::RawRgba(rgba)
                } else {
                    MediaFrame::CpuRgba(rgba)
                })
            }
            DecodedVideoFrame::Yuv(frame) => self.yuv_media_frame(frame).map(MediaFrame::Yuv),
        }
    }

    fn rgba_media_frame(&self, decoded: CpuVideoFrame) -> Result<Arc<CpuMediaFrame>, MediaError> {
        let row_bytes = (decoded.width as usize).saturating_mul(4);
        let expected = row_bytes.saturating_mul(decoded.height as usize);
        if decoded.stride != row_bytes || decoded.data.len() < expected {
            return Err(MediaError::decode(
                &self.source,
                format!(
                    "decoded frame has stride {} and {} bytes, expected RGBA rows of {row_bytes} bytes and {expected} in total",
                    decoded.stride,
                    decoded.data.len()
                ),
            ));
        }

        Ok(Arc::new(CpuMediaFrame {
            rgba: Arc::new(decoded.data),
            width: decoded.width,
            height: decoded.height,
            row_bytes,
        }))
    }

    fn yuv_media_frame(&self, decoded: YuvVideoFrame) -> Result<Arc<YuvFrame>, MediaError> {
        let format = YuvFormat {
            width: decoded.width,
            height: decoded.height,
            subsampling: match decoded.subsampling {
                YuvSubsampling::Yuv420 => ChromaSubsampling::Yuv420,
                YuvSubsampling::Yuv422 => ChromaSubsampling::Yuv422,
                YuvSubsampling::Yuv444 => ChromaSubsampling::Yuv444,
            },
            depth: decoded.depth,
            msb_aligned: decoded.msb_aligned,
            color: yuv_color(decoded.color, decoded.height),
            rotation: Rotation::from_degrees(decoded.rotation).unwrap_or_default(),
        };
        let (layout, _) = YuvLayout::packed(&format, decoded.interleaved_chroma, decoded.has_alpha);
        let frame = YuvFrame {
            format,
            layout,
            data: Arc::new(decoded.data),
        };
        frame.validate().map_err(|details| {
            MediaError::decode(
                &self.source,
                format!("decoded frame can't be read as YUV: {details}"),
            )
        })?;
        Ok(Arc::new(frame))
    }

    fn should_seek(&self, target_frame: u32) -> bool {
        target_frame < self.next_frame_hint
            || target_frame.saturating_sub(self.next_frame_hint) > SEEK_REOPEN_THRESHOLD
    }

    fn seek_to_frame(&mut self, target_frame: u32) -> Result<(), MediaError> {
        let timestamp = self.frame_to_timestamp(target_frame);
        if self
            .input
            .seek_stream(self.stream_index, timestamp)
            .is_err()
        {
            self.reopen()?;
            self.input
                .seek_stream(self.stream_index, timestamp)
                .map_err(|err| {
                    ffmpeg_failure(&self.source, AssetKind::Video, "seek failed", &err)
                })?;
        }
        self.after_seek(target_frame, 0)
    }

    /// Seeks to the keyframe before the one `seek` landed on, whose first frame came after
    /// `target_frame`. Once the retries run out, or that seek fails, the input reopens at the start.
    fn seek_before_landing(
        &mut self,
        target_frame: u32,
        seek: PendingSeek,
    ) -> Result<(), MediaError> {
        let earlier = seek
            .landed
            .filter(|_| seek.retries < SEEK_RETRIES)
            .map(|landed| landed.saturating_sub(1));
        match earlier {
            Some(timestamp) if self.input.seek_stream(self.stream_index, timestamp).is_ok() => {
                self.after_seek(target_frame, seek.retries + 1)
            }
            _ => {
                self.reopen()?;
                self.next_frame_hint = target_frame;
                Ok(())
            }
        }
    }

    fn after_seek(&mut self, target_frame: u32, retries: u8) -> Result<(), MediaError> {
        if let Some(decoder) = &mut self.decoder {
            decoder.flush().map_err(|err| {
                ffmpeg_failure(
                    &self.source,
                    AssetKind::Video,
                    "failed flushing decoder",
                    &err,
                )
            })?;
        }
        self.sent_eof = false;
        self.next_frame_hint = target_frame;
        self.seek = Some(PendingSeek {
            landed: None,
            retries,
        });
        Ok(())
    }

    fn frame_to_timestamp(&self, frame: u32) -> i64 {
        let seconds = f64::from(frame) / self.fps;
        (seconds / self.time_base_seconds).round() as i64 + self.start_time
    }

    fn timestamp_to_frame(&self, timestamp: i64) -> u32 {
        let elapsed = timestamp.saturating_sub(self.start_time);
        if elapsed <= 0 {
            return 0;
        }
        let seconds = (elapsed as f64) * self.time_base_seconds;
        (seconds * self.fps).round() as u32
    }

    fn frame_bytes(&self) -> usize {
        self.last_frame_bytes
    }

    fn open_decoder(&mut self) -> Result<&mut VideoDecoder, MediaError> {
        self.decoder.as_mut().ok_or_else(|| {
            MediaError::decode(
                &self.source,
                "the CPU decoder that replaces the hardware one is not open".to_string(),
            )
        })
    }

    /// Moves the source to the CPU. Cached GPU frames and the hardware decoder go first, as they
    /// hold the file descriptors the replacement may need to open. If it still can't open, the next
    /// decode tries again.
    fn move_to_cpu(&mut self, cache: &mut FrameLruCache) -> Result<(), MediaError> {
        self.options.prefer_hardware_decode = false;
        self.gpu_decode_status = GpuDecodeStatus::NotRequested;
        cache.retain(&[]);
        self.decoder = None;
        self.reopen()
    }

    fn reopen(&mut self) -> Result<(), MediaError> {
        let mut reopened = Self::open(self.source.clone(), self.options.clone())?;
        if let Some(reason) = reopened.gpu_decode_status.unavailable_reason()
            && !self.settle_on_cpu()
        {
            let failure = self.mid_render_failure(self.next_frame_hint, reason);
            self.mid_render_failure = Some(failure.clone());
            return Err(failure);
        }
        reopened.worker = self.worker.take();
        reopened.origin = Arc::clone(&self.origin);
        reopened.mid_render_failure = self.mid_render_failure.take();
        *self = reopened;
        Ok(())
    }
}

enum WorkerRequest {
    Enqueue {
        frame: u32,
    },
    Resolve {
        frame: u32,
        response_tx: mpsc::Sender<Result<MediaFrame, MediaError>>,
    },
    ResolveOnCpu {
        frame: u32,
        reason: String,
        response_tx: mpsc::Sender<Result<MediaFrame, MediaError>>,
    },
    Retain {
        frames: Vec<u32>,
    },
    Shutdown,
}

/// What the worker reports once its decoder is open.
type Startup = Result<(VideoMetadata, GpuDecodeStatus), MediaError>;

fn run_decode_worker(
    source: String,
    options: FfmpegResolverOptions,
    request_tx: mpsc::Sender<WorkerRequest>,
    request_rx: mpsc::Receiver<WorkerRequest>,
    startup_tx: mpsc::Sender<Startup>,
) {
    let mut worker = match LibavStreamDecoder::open(source, options) {
        Ok(mut decoder) => {
            decoder.worker = Some(request_tx);
            let _ = startup_tx.send(Ok((decoder.metadata(), decoder.gpu_decode_status.clone())));
            VideoDecodeWorker::new(decoder)
        }
        Err(error) => {
            let _ = startup_tx.send(Err(error));
            return;
        }
    };
    let mut pending = BTreeSet::new();

    loop {
        if pending.is_empty() || !worker.has_room_to_prefetch() {
            let Ok(request) = request_rx.recv() else {
                return;
            };
            if handle_worker_request(request, &mut worker, &mut pending) {
                return;
            }
        }

        loop {
            match request_rx.try_recv() {
                Ok(request) => {
                    if handle_worker_request(request, &mut worker, &mut pending) {
                        return;
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => return,
            }
        }

        if worker.has_room_to_prefetch()
            && let Some(frame) = pending.pop_first()
        {
            worker.prefetch_frame(frame);
        }
    }
}

fn handle_worker_request(
    request: WorkerRequest,
    worker: &mut VideoDecodeWorker,
    pending: &mut BTreeSet<u32>,
) -> bool {
    match request {
        WorkerRequest::Enqueue { frame } => {
            pending.insert(frame);
            false
        }
        WorkerRequest::Resolve { frame, response_tx } => {
            pending.remove(&frame);
            let _ = response_tx.send(worker.resolve_frame(frame));
            false
        }
        WorkerRequest::ResolveOnCpu {
            frame,
            reason,
            response_tx,
        } => {
            pending.remove(&frame);
            let _ = response_tx.send(worker.resolve_on_cpu(frame, &reason));
            false
        }
        WorkerRequest::Retain { frames } => {
            pending.retain(|frame| frames.binary_search(frame).is_ok());
            worker.retain_frames(&frames);
            false
        }
        WorkerRequest::Shutdown => true,
    }
}

pub struct FfmpegVideoResolver {
    id: String,
    metadata: VideoMetadata,
    decode_status: GpuDecodeStatus,
    request_tx: mpsc::Sender<WorkerRequest>,
    worker_handle: Mutex<Option<thread::JoinHandle<()>>>,
    scheduled_frames: Mutex<HashSet<u32>>,
}

impl FfmpegVideoResolver {
    pub fn open(source: impl Into<String>) -> Result<Self, MediaError> {
        Self::open_with_options(source, FfmpegResolverOptions::default())
    }

    pub fn open_with_options(
        source: impl Into<String>,
        options: FfmpegResolverOptions,
    ) -> Result<Self, MediaError> {
        let source = source.into();
        let (request_tx, request_rx) = mpsc::channel::<WorkerRequest>();
        let (startup_tx, startup_rx) = mpsc::channel();
        let worker_source = source.clone();
        let worker_tx = request_tx.clone();
        let worker_handle = thread::spawn(move || {
            run_decode_worker(worker_source, options, worker_tx, request_rx, startup_tx)
        });
        let startup = startup_rx.recv().unwrap_or_else(|_| {
            Err(MediaError::decode(
                &source,
                "video decode worker stopped while opening".to_string(),
            ))
        });
        let (metadata, decode_status) = match startup {
            Ok(opened) => opened,
            Err(error) => {
                let _ = worker_handle.join();
                return Err(error);
            }
        };

        Ok(Self {
            id: source,
            metadata,
            decode_status,
            request_tx,
            worker_handle: Mutex::new(Some(worker_handle)),
            scheduled_frames: Mutex::new(HashSet::new()),
        })
    }

    pub fn decode_mode_label(&self) -> &'static str {
        self.decode_status.label()
    }

    pub fn decode_unavailable_reason(&self) -> Option<&str> {
        self.decode_status.unavailable_reason()
    }
}

impl Drop for FfmpegVideoResolver {
    fn drop(&mut self) {
        let _ = self.request_tx.send(WorkerRequest::Shutdown);
        if let Ok(mut handle_guard) = self.worker_handle.lock()
            && let Some(handle) = handle_guard.take()
        {
            let _ = handle.join();
        }
    }
}

impl VideoFrameResolver for FfmpegVideoResolver {
    fn metadata(&self) -> VideoMetadata {
        self.metadata
    }

    fn enqueue_frame(&self, frame: u32) -> Result<(), MediaError> {
        if frame >= self.metadata.frame_count {
            return Err(MediaError::FrameOutOfRange {
                media_source: self.id.clone(),
                frame,
                frame_count: self.metadata.frame_count,
            });
        }

        if let Ok(mut scheduled) = self.scheduled_frames.lock()
            && !scheduled.insert(frame)
        {
            return Ok(());
        }

        self.request_tx
            .send(WorkerRequest::Enqueue { frame })
            .map_err(|_| {
                MediaError::decode(&self.id, "video decode worker is unavailable".to_string())
            })
    }

    fn frame(&self, frame: u32) -> Result<MediaFrame, MediaError> {
        if frame >= self.metadata.frame_count {
            return Err(MediaError::FrameOutOfRange {
                media_source: self.id.clone(),
                frame,
                frame_count: self.metadata.frame_count,
            });
        }

        let (response_tx, response_rx) = mpsc::channel();
        self.request_tx
            .send(WorkerRequest::Resolve { frame, response_tx })
            .map_err(|_| {
                MediaError::decode(&self.id, "video decode worker is unavailable".to_string())
            })?;

        response_rx.recv().map_err(|_| {
            MediaError::decode(
                &self.id,
                format!("video decode worker did not return frame {frame}"),
            )
        })?
    }

    fn retain_frames(&self, frames: &[u32]) {
        let mut frames = frames.to_vec();
        frames.sort_unstable();
        frames.dedup();

        if let Ok(mut scheduled) = self.scheduled_frames.lock() {
            let keep: HashSet<_> = frames.iter().copied().collect();
            scheduled.retain(|frame| keep.contains(frame));
        }

        let _ = self.request_tx.send(WorkerRequest::Retain { frames });
    }
}

/// The colour FFmpeg's tags describe. FCC and SMPTE 240M have no matrix of their own, so they read
/// as the nearest one, and a frame with no usable tag reads by its coded height.
pub(crate) fn yuv_color(tags: lumen_ffmpeg::ColorTags, height: u32) -> YuvColor {
    use lumen_ffmpeg::ColorMatrix;
    let matrix = match tags.matrix {
        ColorMatrix::Bt709 | ColorMatrix::Smpte240m => YuvMatrix::Bt709,
        ColorMatrix::Bt470bg | ColorMatrix::Smpte170m | ColorMatrix::Fcc => YuvMatrix::Bt601,
        ColorMatrix::Bt2020Ncl | ColorMatrix::Bt2020Cl => YuvMatrix::Bt2020,
        ColorMatrix::Other => YuvMatrix::untagged(height),
    };
    YuvColor {
        matrix,
        full_range: tags.full_range,
    }
}

fn resolve_stream_fps(stream: &VideoStreamInfo, source: &str) -> Result<f64, MediaError> {
    stream
        .frame_rate
        .and_then(|rate| rate.as_f64())
        .filter(|fps| *fps > 0.0)
        .ok_or_else(|| {
            MediaError::decode(source, "unable to determine stream frame rate".to_string())
        })
}

/// The stream's frame count, capped by what the presented duration holds. A stream-copied cut keeps
/// preroll packets that an edit list hides, and the stream still counts them.
fn resolve_frame_count(
    stream: &VideoStreamInfo,
    container_duration_us: Option<i64>,
    fps: f64,
) -> Option<u32> {
    let stream_seconds = stream
        .duration_ts
        .zip(stream.time_base.as_f64())
        .map(|(duration, time_base)| duration as f64 * time_base);
    let container_seconds = container_duration_us.map(|duration| duration as f64 / 1_000_000.0);
    let presented = stream_seconds
        .or(container_seconds)
        .map(|seconds| (seconds * fps).round().clamp(1.0, f64::from(u32::MAX)) as u32);
    let explicit = stream
        .frame_count
        .map(|count| u32::try_from(count).unwrap_or(u32::MAX));
    match (explicit, presented) {
        (Some(explicit), Some(presented)) => Some(explicit.min(presented)),
        (explicit, presented) => explicit.or(presented),
    }
}

fn open_input(source: &str) -> Result<InputContext, MediaError> {
    InputContext::open(source.to_string())
        .map_err(|err| open_failure(source, AssetKind::Video, &err).into())
}

/// Reads `input` to its end, releasing each packet before reading the next.
fn count_frames(input: &mut InputContext, stream_index: usize) -> Option<u32> {
    let mut frames = 0;
    while let Some(packet) = input.read_packet().ok()? {
        if packet.stream_index() == stream_index {
            frames += 1;
        }
    }
    (frames > 0).then_some(frames)
}

fn open_video_decoder(
    source: &str,
    input: &InputContext,
    stream: &VideoStreamInfo,
    options: &FfmpegResolverOptions,
) -> Result<(VideoDecoder, GpuDecodeStatus), MediaError> {
    let stream_index = stream.stream_index;
    if !options.prefer_hardware_decode {
        return Ok((
            open_cpu_video_decoder(source, input, stream_index)?,
            GpuDecodeStatus::NotRequested,
        ));
    }

    let backend = preferred_gpu_backend(options);
    let opened = if stream.has_alpha {
        // GPU frames carry no alpha.
        Err("sources with alpha need the CPU path, which decodes the alpha layer".to_string())
    } else {
        open_hardware_decoder(input, stream_index, backend, options)
    };
    match opened {
        Ok(decoder) => Ok((decoder, GpuDecodeStatus::Active { backend })),
        Err(reason) => Ok((
            open_cpu_video_decoder(source, input, stream_index)?,
            GpuDecodeStatus::Unavailable {
                backend,
                reason: format!("{source}: {reason}"),
            },
        )),
    }
}

fn open_hardware_decoder(
    input: &InputContext,
    stream_index: usize,
    backend: GpuBackend,
    options: &FfmpegResolverOptions,
) -> Result<VideoDecoder, String> {
    #[cfg(all(target_os = "linux", feature = "vulkan"))]
    if backend == GpuBackend::Vulkan {
        let device = options.vulkan_device.as_ref().ok_or(
            "the renderer's Vulkan device has no video decode queue, or the renderer isn't on Vulkan",
        )?;
        return VideoDecoder::open_on_vulkan_device(input, stream_index, device)
            .map_err(|error| error.to_string());
    }
    #[cfg(not(all(target_os = "linux", feature = "vulkan")))]
    let _ = options;
    if backend != GpuBackend::Metal {
        return Err(format!(
            "{backend:?} hardware decode isn't available in this build"
        ));
    }
    VideoDecoder::open(
        input,
        VideoDecoderConfig {
            stream_index,
            mode: DecodeMode::Gpu(backend),
        },
    )
    .map_err(|error| error.to_string())
}

fn open_cpu_video_decoder(
    source: &str,
    input: &InputContext,
    stream_index: usize,
) -> Result<VideoDecoder, MediaError> {
    VideoDecoder::open(
        input,
        VideoDecoderConfig {
            stream_index,
            mode: DecodeMode::Cpu,
        },
    )
    .map_err(|err| open_failure(source, AssetKind::Video, &err).into())
}

fn preferred_gpu_backend(options: &FfmpegResolverOptions) -> GpuBackend {
    #[cfg(all(target_os = "linux", feature = "vulkan"))]
    if options.vulkan_device.is_some() {
        return GpuBackend::Vulkan;
    }
    #[cfg(not(all(target_os = "linux", feature = "vulkan")))]
    let _ = options;
    if cfg!(target_os = "macos") {
        GpuBackend::Metal
    } else {
        GpuBackend::Vulkan
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf, process::Command};

    use super::*;
    use crate::media::YuvChroma;

    const NTSC_RATE: &str = "30000/1001";

    /// Encodes frames whose luma is `16 + 3 * n`, so a decoded frame identifies its own index.
    fn encode_counting_clip(name: &str, args: &[&str]) -> Option<PathBuf> {
        let source = format!(
            "nullsrc=s=32x32:r={NTSC_RATE}:d=2,geq=lum='16+3*N':cb=128:cr=128,format=yuv420p"
        );
        encode_clip(name, &source, args)
    }

    fn encode_clip(name: &str, source: &str, args: &[&str]) -> Option<PathBuf> {
        let path = std::env::temp_dir().join(format!("lumen_engine_{}_{name}", std::process::id()));
        let output = Command::new("ffmpeg")
            .args(["-y", "-hide_banner", "-loglevel", "error", "-f", "lavfi"])
            .args(["-i", source])
            .args(args)
            .args([
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-qp",
                "1",
                "-bf",
                "0",
            ])
            .arg(&path)
            .output();
        let output = match output {
            Ok(output) => output,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                eprintln!("ffmpeg CLI not installed; skipping {name}");
                return None;
            }
            Err(error) => panic!("{name}: failed to run ffmpeg: {error}"),
        };
        if output.status.success() {
            return Some(path);
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("Unknown encoder") || stderr.contains("Encoder not found") {
            eprintln!(
                "ffmpeg encoder unavailable; skipping {name}: {}",
                stderr.trim()
            );
            return None;
        }
        panic!(
            "{name}: ffmpeg fixture failed ({}): {stderr}",
            output.status
        );
    }

    fn open_cpu(path: &std::path::Path) -> Result<FfmpegVideoResolver, MediaError> {
        FfmpegVideoResolver::open_with_options(
            path.to_string_lossy().to_string(),
            FfmpegResolverOptions::default(),
        )
    }

    fn decoded_frame_index(resolver: &FfmpegVideoResolver, frame: u32) -> u32 {
        frame_index(&resolver.frame(frame).expect("decode frame"))
    }

    fn yuv_frame(frame: MediaFrame) -> Arc<YuvFrame> {
        let MediaFrame::Yuv(frame) = frame else {
            panic!("expected a YUV frame");
        };
        frame
    }

    fn frame_index(frame: &MediaFrame) -> u32 {
        let MediaFrame::Yuv(decoded) = frame else {
            panic!("expected a YUV frame");
        };
        // The first luma sample is `16 + 3 * index`.
        (f64::from(decoded.data[0] - 16) / 3.0).round() as u32
    }

    /// The alpha at a frame's first pixel, from 0 to 1, or `None` if it has no alpha plane.
    fn first_alpha(frame: &YuvFrame) -> Option<f32> {
        let plane = frame.layout.alpha?;
        let depth = frame.format.depth;
        let sample = if depth > 8 {
            u16::from_le_bytes([frame.data[plane.offset], frame.data[plane.offset + 1]])
        } else {
            u16::from(frame.data[plane.offset])
        };
        Some(f32::from(sample) / ((1u32 << depth) - 1) as f32)
    }

    #[test]
    fn frame_indexes_count_from_the_stream_start_time() {
        // MPEG-TS seeks land between keyframes, so that case only reads forward.
        for (name, frames) in [
            ("offset.mp4", &[0, 5, 40, 12][..]),
            ("offset.ts", &[0, 5, 40][..]),
        ] {
            let Some(path) = encode_counting_clip(name, &["-output_ts_offset", "1.4"]) else {
                return;
            };
            let resolver = open_cpu(&path).expect("open clip");
            for frame in frames {
                assert_eq!(decoded_frame_index(&resolver, *frame), *frame, "{name}");
            }
            let _ = fs::remove_file(path);
        }
    }

    #[test]
    fn frame_count_falls_back_to_the_container_duration() {
        // Matroska stores no per-stream duration or frame count here, and its container duration
        // includes the start offset.
        for (name, args) in [
            ("no_stream_duration.mkv", &[][..]),
            (
                "offset_no_stream_duration.mkv",
                &["-output_ts_offset", "1.4"][..],
            ),
        ] {
            let Some(path) = encode_counting_clip(name, args) else {
                return;
            };
            let resolver = open_cpu(&path).expect("open clip");
            let metadata = resolver.metadata();
            assert_eq!(metadata.frame_count, 60, "{name}");
            assert!((f64::from(metadata.fps) - 30_000.0 / 1001.0).abs() < 0.001);
            assert_eq!(decoded_frame_index(&resolver, 59), 59, "{name}");
            let _ = fs::remove_file(path);
        }
    }

    #[test]
    fn audio_preroll_does_not_add_frames_to_the_container_duration() {
        // AAC priming puts the Matroska start before zero, ahead of a video stream at zero.
        let Some(path) = encode_clip(
            "audio_preroll.mkv",
            "testsrc2=s=32x32:r=20:d=2",
            &[
                "-f",
                "lavfi",
                "-i",
                "sine=d=2",
                "-c:a",
                "aac",
                "-avoid_negative_ts",
                "disabled",
            ],
        ) else {
            return;
        };
        let resolver = open_cpu(&path).expect("open clip");
        assert_eq!(resolver.metadata().frame_count, 40);
        assert!(resolver.frame(39).is_ok());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn a_stream_copied_cut_counts_only_the_frames_that_play() {
        // The cut starts on a keyframe, and an edit list hides its 15 preroll frames.
        let dir = std::env::temp_dir().join(format!("lumen_engine_{}_cut", std::process::id()));
        fs::create_dir_all(&dir).expect("create fixture dir");
        let full = dir.join("full.mp4");
        let cut = dir.join("cut.mp4");
        let run = |args: &[&str]| {
            let status = Command::new("ffmpeg")
                .args(["-y", "-hide_banner", "-loglevel", "error"])
                .args(args)
                .status();
            match status {
                Ok(status) => {
                    assert!(status.success(), "ffmpeg {args:?}");
                    true
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    eprintln!("ffmpeg CLI not installed; skipping the cut clip");
                    false
                }
                Err(error) => panic!("failed to run ffmpeg: {error}"),
            }
        };
        let encoded = run(&[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=160x90:rate=30:duration=10",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-bf",
            "2",
            "-g",
            "24",
            "-keyint_min",
            "24",
            "-sc_threshold",
            "0",
            "-pix_fmt",
            "yuv420p",
            full.to_str().expect("utf-8 path"),
        ]);
        if !encoded {
            return;
        }
        assert!(run(&[
            "-ss",
            "2.1",
            "-i",
            full.to_str().expect("utf-8 path"),
            "-t",
            "8",
            "-c",
            "copy",
            cut.to_str().expect("utf-8 path"),
        ]));

        let full_clip = open_cpu(&full).expect("open full clip");
        let cut_clip = open_cpu(&cut).expect("open cut clip");
        assert_eq!(full_clip.metadata().frame_count, 300);
        assert_eq!(cut_clip.metadata().frame_count, 237);
        let last = yuv_frame(cut_clip.frame(236).expect("decode the last frame"));
        let source_last = yuv_frame(
            full_clip
                .frame(299)
                .expect("decode the source's last frame"),
        );
        assert_eq!(last.data, source_last.data);
        assert!(cut_clip.frame(237).is_err());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn images_read_as_video_count_their_own_frames() {
        const ONE_PIXEL_PNG: &[u8] = &[
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9c, 0x63, 0xf8, 0xef, 0xa0, 0xf0, 0x1f, 0x00, 0x06, 0x00, 0x02, 0x5f, 0x34, 0xcd,
            0x40, 0xc0, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
        ];
        let still =
            std::env::temp_dir().join(format!("lumen_engine_{}_still.png", std::process::id()));
        fs::write(&still, ONE_PIXEL_PNG).expect("write png");
        let resolver = open_cpu(&still).expect("open png");
        assert_eq!(resolver.metadata().frame_count, 1);
        assert!(resolver.frame(0).is_ok());
        let _ = fs::remove_file(still);

        let animated =
            std::env::temp_dir().join(format!("lumen_engine_{}_animated.apng", std::process::id()));
        let Ok(generated) = Command::new("ffmpeg")
            .args(["-y", "-hide_banner", "-loglevel", "error", "-f", "lavfi"])
            .args(["-i", "testsrc2=s=32x32:r=25:d=0.4", "-f", "apng"])
            .arg(&animated)
            .status()
        else {
            eprintln!("ffmpeg CLI unavailable; skipping APNG frame count");
            return;
        };
        assert!(generated.success(), "generate APNG");
        let resolver = open_cpu(&animated).expect("open apng");
        assert_eq!(resolver.metadata().frame_count, 10);
        assert!(resolver.frame(9).is_ok());
        let _ = fs::remove_file(animated);
    }

    #[test]
    fn a_missing_file_fails_to_open() {
        let error = open_cpu(&std::env::temp_dir().join("lumen_engine_no_such_clip.mp4"))
            .err()
            .expect("no such file");
        assert_eq!(error.code(), "media.not_found", "{error}");
    }

    #[test]
    fn missing_duration_is_an_error_instead_of_a_one_frame_clip() {
        let Some(path) = encode_counting_clip("elementary.h264", &["-f", "h264"]) else {
            return;
        };
        let error = open_cpu(&path)
            .err()
            .expect("no duration to count frames from");
        assert!(error.to_string().contains("frame count"), "{error}");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn a_failing_hardware_decoder_moves_the_source_to_the_cpu_for_good() {
        let Some(path) = encode_counting_clip("hardware_failure.mp4", &[]) else {
            return;
        };
        // A CPU decoder marked active as hardware refuses `receive_gpu_frame` on the first frame,
        // the state a hardware decoder that opens and then can't decode is in, on any machine.
        let mut decoder =
            LibavStreamDecoder::open(path.to_string_lossy().to_string(), Default::default())
                .expect("open clip");
        decoder.options.prefer_hardware_decode = true;
        decoder.gpu_decode_status = GpuDecodeStatus::Active {
            backend: GpuBackend::Vulkan,
        };

        for frame in [5, 6, 2] {
            let decoded = decoder
                .decode_frame(frame, &mut FrameLruCache::default())
                .expect("decode on the CPU");
            assert_eq!(frame_index(&decoded), frame);
            assert!(!decoder.options.prefer_hardware_decode);
            assert!(!matches!(
                decoder.gpu_decode_status,
                GpuDecodeStatus::Active { .. }
            ));
        }
        decoder.reopen().expect("reopen");
        assert!(!matches!(
            decoder.gpu_decode_status,
            GpuDecodeStatus::Active { .. }
        ));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn a_failed_hardware_decoder_is_released_before_its_replacement_opens() {
        let Some(path) = encode_counting_clip("release_first.mp4", &[]) else {
            return;
        };
        let source = path.to_string_lossy().to_string();
        let mut decoder =
            LibavStreamDecoder::open(source.clone(), Default::default()).expect("open clip");
        let cached = decoder
            .decode_frame(2, &mut FrameLruCache::default())
            .expect("frame to retire");
        decoder.options.prefer_hardware_decode = true;
        decoder.gpu_decode_status = GpuDecodeStatus::Active {
            backend: GpuBackend::Vulkan,
        };
        let mut worker = VideoDecodeWorker::new(decoder);
        worker.cache.insert(2, cached, false);

        // A source that can't reopen stands in for one out of file descriptors.
        worker.decoder.source = format!("{source}.missing");
        worker.resolve_frame(3).unwrap_err();
        assert!(worker.decoder.decoder.is_none());
        assert!(!worker.cache.contains(2));
        worker.resolve_frame(3).unwrap_err();

        worker.decoder.source = source;
        let decoded = worker
            .resolve_frame(3)
            .expect("the next decode tries again");
        assert_eq!(frame_index(&decoded), 3);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn a_frame_the_renderer_rejects_moves_the_source_to_the_cpu_and_drops_cached_gpu_frames() {
        let Some(path) = encode_counting_clip("import_failure.mp4", &[]) else {
            return;
        };
        let mut decoder =
            LibavStreamDecoder::open(path.to_string_lossy().to_string(), Default::default())
                .expect("open clip");
        let cached = decoder
            .decode_frame(8, &mut FrameLruCache::default())
            .expect("frame to retire");
        decoder.gpu_decode_status = GpuDecodeStatus::Active {
            backend: GpuBackend::Vulkan,
        };
        decoder.options.prefer_hardware_decode = true;
        let mut worker = VideoDecodeWorker::new(decoder);
        worker.cache.insert(8, cached, false);

        let retried = worker.resolve_on_cpu(9, "import failed").expect("retry");
        assert_eq!(frame_index(&retried), 9);
        assert!(!worker.decoder.options.prefer_hardware_decode);
        assert!(matches!(
            worker.decoder.gpu_decode_status,
            GpuDecodeStatus::NotRequested
        ));
        assert!(!worker.cache.contains(8));

        let again = worker.resolve_on_cpu(3, "still failing").expect("retry");
        assert_eq!(frame_index(&again), 3);
        // A source already on the CPU may still have cached a GPU frame before it moved.
        assert!(!worker.cache.contains(9) && worker.cache.contains(3));
        let _ = fs::remove_file(path);
    }

    /// A CPU decoder marked active as hardware, whose next frame fails like a hardware decoder's.
    fn failing_hardware_decoder(path: &std::path::Path) -> LibavStreamDecoder {
        let mut decoder =
            LibavStreamDecoder::open(path.to_string_lossy().to_string(), Default::default())
                .expect("open clip");
        decoder.options.prefer_hardware_decode = true;
        decoder.gpu_decode_status = GpuDecodeStatus::Active {
            backend: GpuBackend::Vulkan,
        };
        decoder
    }

    #[test]
    fn a_source_settled_on_the_gpu_keeps_failing_across_a_reopen() {
        let Some(path) = encode_counting_clip("reopen_settled.mp4", &[]) else {
            return;
        };
        let mut decoder = failing_hardware_decoder(&path);
        let (request_tx, _request_rx) = mpsc::channel();
        decoder.worker = Some(request_tx);
        decoder.origin.set(PixelOrigin::Gpu).ok();
        let origin = Arc::clone(&decoder.origin);
        let mut cache = FrameLruCache::default();

        let failure = decoder.decode_frame(5, &mut cache).unwrap_err().to_string();
        assert!(failure.contains("would change"), "{failure}");

        // A reopen that finds the hardware decoder gone fails the same way.
        let gone = decoder.reopen().unwrap_err().to_string();
        assert!(gone.contains("would change"), "{gone}");

        decoder.options.prefer_hardware_decode = false;
        decoder.reopen().expect("reopen");
        assert!(Arc::ptr_eq(&origin, &decoder.origin));
        assert!(decoder.worker.is_some());
        for frame in [9, 2] {
            let later = decoder.decode_frame(frame, &mut cache).unwrap_err();
            assert_eq!(later.to_string(), gone);
        }
        let _ = fs::remove_file(path);
    }

    #[test]
    fn seeks_after_a_reopen_decode_the_right_frames_and_leave_the_source_unsettled() {
        let Some(path) = encode_counting_clip("reopen_seeks.mp4", &[]) else {
            return;
        };
        let mut decoder =
            LibavStreamDecoder::open(path.to_string_lossy().to_string(), Default::default())
                .expect("open clip");
        let (request_tx, _request_rx) = mpsc::channel();
        decoder.worker = Some(request_tx);
        let origin = Arc::clone(&decoder.origin);
        let mut cache = FrameLruCache::default();

        for frame in [6, 7] {
            let decoded = decoder.decode_frame(frame, &mut cache).expect("decode");
            assert_eq!(frame_index(&decoded), frame);
        }
        decoder.reopen().expect("reopen");
        assert!(Arc::ptr_eq(&origin, &decoder.origin));
        assert!(decoder.worker.is_some());
        for frame in [12, 3, 4, 40, 1] {
            let decoded = decoder.decode_frame(frame, &mut cache).expect("decode");
            assert_eq!(frame_index(&decoded), frame);
        }
        assert!(origin.get().is_none());
        let _ = fs::remove_file(path);
    }

    /// The fixture is an open GOP, with CRA keyframes every 12 frames and a RASL frame before each:
    /// `ffmpeg -f lavfi -i testsrc2=s=128x72:r=30:d=3 -g 12 -c:v libx265 -preset veryfast -crf 24
    /// -tag:v hvc1 -pix_fmt yuv420p open_gop.mp4`.
    #[test]
    fn stepping_back_before_an_open_gop_keyframe_decodes_the_frame_asked_for() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/open_gop.mp4");
        let mut cache = FrameLruCache::default();
        let mut stepping = LibavStreamDecoder::open(path, Default::default()).expect("open clip");
        // Frames 59 and 35 are RASL frames, and a seek to 57 also lands on CRA 60.
        for frame in [60, 59, 36, 35, 60, 57] {
            let stepped = yuv_frame(stepping.decode_frame(frame, &mut cache).expect("decode"));
            let fresh = LibavStreamDecoder::open(path, Default::default())
                .expect("open clip")
                .decode_frame(frame, &mut cache)
                .map(yuv_frame)
                .expect("decode");
            assert!(stepped.data == fresh.data, "frame {frame}");
        }
    }

    /// Runs on a GPU with Vulkan decode for `libx264` or a file in `LUMEN_VULKAN_DECODE_FILES`.
    #[cfg(all(target_os = "linux", feature = "vulkan"))]
    #[test]
    fn a_hardware_decoder_stays_on_the_gpu_through_a_reopen_and_seeks() {
        let Ok((_renderer, device)) =
            pollster::block_on(crate::gpu::GpuCompositionRenderer::new_vulkan_decode())
        else {
            return;
        };
        let mut options = FfmpegResolverOptions::with_hardware_decode(true);
        options.vulkan_device = Some(device);
        let mut clips: Vec<_> = std::env::var_os("LUMEN_VULKAN_DECODE_FILES")
            .map(|files| std::env::split_paths(&files).collect())
            .unwrap_or_default();
        clips.extend(encode_clip(
            "reopen_gpu.mp4",
            "testsrc2=s=320x240:r=30:d=2",
            &["-pix_fmt", "yuv420p"],
        ));
        let mut cache = FrameLruCache::default();
        for clip in &clips {
            let source = clip.to_string_lossy().to_string();
            let Ok(mut decoder) = LibavStreamDecoder::open(source.clone(), options.clone()) else {
                continue;
            };
            let on_gpu = |decoded: &MediaFrame| matches!(decoded, MediaFrame::GpuVideo(_));
            if !decoder
                .decode_frame(6, &mut cache)
                .is_ok_and(|d| on_gpu(&d))
            {
                eprintln!("{source} doesn't decode on this hardware; skipping");
                continue;
            }
            let Ok(MediaFrame::GpuVideo(drawn)) = decoder.decode_frame(7, &mut cache) else {
                panic!("{source}: frame 7 should decode on the GPU like frame 6");
            };
            assert!(drawn.settle_on_gpu());

            decoder.reopen().expect("reopen");
            for frame in [20, 3, 4, 30] {
                let Ok(MediaFrame::GpuVideo(decoded)) = decoder.decode_frame(frame, &mut cache)
                else {
                    panic!("{source}: frame {frame} left the GPU after a reopen");
                };
                assert!(decoded.settle_on_gpu());
                assert!(!decoder.settle_on_cpu());
                assert_eq!(decoder.frame_for_pts(decoded.frame.pts()), frame);
            }
        }
    }

    #[test]
    fn sources_with_alpha_warn_skip_hardware_decode_and_keep_their_alpha() {
        let path =
            std::env::temp_dir().join(format!("lumen_engine_{}_alpha.mov", std::process::id()));
        let Ok(output) = Command::new("ffmpeg")
            .args(["-y", "-hide_banner", "-loglevel", "error", "-f", "lavfi"])
            .args([
                "-i",
                "color=c=0x2080ff:s=32x32:r=30:d=0.2,format=rgba,colorchannelmixer=aa=0.5,format=yuva444p10le",
            ])
            .args(["-c:v", "prores_ks", "-profile:v", "4444", "-alpha_bits", "16"])
            .arg(&path)
            .output()
        else {
            eprintln!("ffmpeg CLI not installed; skipping alpha routing test");
            return;
        };
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(stderr.contains("Unknown encoder"), "{stderr}");
            eprintln!("prores_ks unavailable; skipping alpha routing test");
            return;
        }

        let resolver = FfmpegVideoResolver::open_with_options(
            path.to_string_lossy().to_string(),
            FfmpegResolverOptions::with_hardware_decode(true),
        )
        .expect("open clip");
        let reason = resolver.decode_unavailable_reason().expect("a warning");
        assert!(reason.contains("alpha"), "{reason}");
        let decoded = yuv_frame(resolver.frame(0).expect("decode frame"));
        assert_eq!(decoded.format.depth, 12);
        let alpha = first_alpha(&decoded).expect("an alpha plane");
        assert!((alpha - 0.5).abs() <= 0.01, "{alpha}");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn vp9_alpha_layers_are_detected_and_decoded_into_an_alpha_plane() {
        let path =
            std::env::temp_dir().join(format!("lumen_engine_{}_alpha.webm", std::process::id()));
        let Ok(output) = Command::new("ffmpeg")
            .args(["-y", "-hide_banner", "-loglevel", "error", "-f", "lavfi"])
            .args([
                "-i",
                "color=c=0x2080ff:s=32x32:r=30:d=0.2,format=rgba,colorchannelmixer=aa=0.5,format=yuva420p",
            ])
            .args(["-c:v", "libvpx-vp9", "-crf", "10", "-b:v", "0"])
            .arg(&path)
            .output()
        else {
            eprintln!("ffmpeg CLI not installed; skipping VP9 alpha test");
            return;
        };
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(stderr.contains("Unknown encoder"), "{stderr}");
            eprintln!("libvpx-vp9 unavailable; skipping VP9 alpha test");
            return;
        }

        let source = path.to_string_lossy().to_string();
        let input = open_input(&source).expect("open clip");
        assert!(input.best_video_stream().expect("video stream").has_alpha);
        let resolver =
            FfmpegVideoResolver::open_with_options(source, FfmpegResolverOptions::default())
                .expect("open clip");
        let decoded = yuv_frame(resolver.frame(0).expect("decode frame"));
        let alpha = first_alpha(&decoded).expect("an alpha plane");
        assert!((alpha - 0.5).abs() <= 0.01, "{alpha}");
        let _ = fs::remove_file(path);
    }

    /// The stream's parameters come from its opaque start, so only the frames show the alpha.
    #[test]
    fn frames_that_gain_alpha_mid_stream_carry_an_alpha_plane() {
        let directory =
            std::env::temp_dir().join(format!("lumen_engine_{}_gains_alpha", std::process::id()));
        fs::create_dir_all(&directory).expect("create directory");
        let encode = |name: &str, filter: &str, pixel_format: &str| {
            let path = directory.join(name);
            Command::new("ffmpeg")
                .args(["-y", "-hide_banner", "-loglevel", "error", "-f", "lavfi"])
                .args(["-i", filter])
                .args(["-pix_fmt", pixel_format, "-c:v", "prores_ks"])
                .args(["-profile:v", "4444", "-alpha_bits", "16"])
                .arg(&path)
                .output()
                .map(|output| (path, output))
        };
        let parts = [
            encode(
                "opaque.mov",
                "color=c=0x2080ff:s=32x32:r=30:d=0.1",
                "yuv444p10le",
            ),
            encode(
                "alpha.mov",
                "color=c=0x2080ff:s=32x32:r=30:d=0.1,format=rgba,colorchannelmixer=aa=0.5",
                "yuva444p10le",
            ),
        ];
        let mut list = String::new();
        for part in &parts {
            let Ok((path, output)) = part else {
                eprintln!("ffmpeg CLI not installed; skipping mid-stream alpha test");
                return;
            };
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                assert!(stderr.contains("Unknown encoder"), "{stderr}");
                eprintln!("prores_ks unavailable; skipping mid-stream alpha test");
                return;
            }
            list.push_str(&format!("file '{}'\n", path.display()));
        }
        let list_path = directory.join("list.txt");
        fs::write(&list_path, list).expect("write concat list");
        let path = directory.join("gains_alpha.mov");
        let output = Command::new("ffmpeg")
            .args(["-y", "-hide_banner", "-loglevel", "error"])
            .args(["-f", "concat", "-safe", "0", "-i"])
            .arg(&list_path)
            .args(["-c", "copy"])
            .arg(&path)
            .output()
            .expect("concat clips");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );

        let source = path.to_string_lossy().to_string();
        let input = open_input(&source).expect("open clip");
        assert!(!input.best_video_stream().expect("video stream").has_alpha);
        let resolver =
            FfmpegVideoResolver::open_with_options(source, FfmpegResolverOptions::default())
                .expect("open clip");
        let opaque = yuv_frame(resolver.frame(0).expect("decode frame"));
        assert!(first_alpha(&opaque).is_none());
        let translucent = yuv_frame(resolver.frame(4).expect("decode frame"));
        let alpha = first_alpha(&translucent).expect("an alpha plane");
        assert!((alpha - 0.5).abs() <= 0.01, "{alpha}");
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn yuv_frames_carry_the_stream_tags_and_the_untagged_rule() {
        struct Clip {
            name: &'static str,
            source: &'static str,
            args: &'static [&'static str],
            /// Clockwise degrees to rotate the file's display by, through a remux.
            display_rotation: Option<&'static str>,
            matrix: YuvMatrix,
            full_range: bool,
            rotation: Rotation,
        }
        const SD: &str = "testsrc2=s=64x48:r=30:d=0.2";
        let clips = [
            Clip {
                name: "tagged_709.mp4",
                source: SD,
                args: &[
                    "-pix_fmt",
                    "yuv420p",
                    "-colorspace",
                    "bt709",
                    "-color_range",
                    "tv",
                ],
                display_rotation: None,
                matrix: YuvMatrix::Bt709,
                full_range: false,
                rotation: Rotation::None,
            },
            Clip {
                name: "tagged_2020_full.mp4",
                source: SD,
                args: &[
                    "-pix_fmt",
                    "yuv420p",
                    "-colorspace",
                    "bt2020nc",
                    "-color_range",
                    "pc",
                ],
                display_rotation: None,
                matrix: YuvMatrix::Bt2020,
                full_range: true,
                rotation: Rotation::None,
            },
            Clip {
                name: "tagged_601_hd.mp4",
                source: "testsrc2=s=1280x720:r=30:d=0.1",
                args: &["-pix_fmt", "yuv420p", "-colorspace", "smpte170m"],
                display_rotation: None,
                matrix: YuvMatrix::Bt601,
                full_range: false,
                rotation: Rotation::None,
            },
            Clip {
                name: "untagged_sd.mp4",
                source: SD,
                args: &["-pix_fmt", "yuv420p"],
                display_rotation: None,
                matrix: YuvMatrix::Bt601,
                full_range: false,
                rotation: Rotation::None,
            },
            Clip {
                name: "untagged_hd.mp4",
                source: "testsrc2=s=1280x720:r=30:d=0.1",
                args: &["-pix_fmt", "yuv420p"],
                display_rotation: None,
                matrix: YuvMatrix::Bt709,
                full_range: false,
                rotation: Rotation::None,
            },
            Clip {
                name: "jpeg_range.mp4",
                source: SD,
                args: &["-pix_fmt", "yuvj420p"],
                display_rotation: None,
                matrix: YuvMatrix::Bt601,
                full_range: true,
                rotation: Rotation::None,
            },
            Clip {
                name: "rotated.mp4",
                source: SD,
                args: &["-pix_fmt", "yuv420p"],
                // FFmpeg's `display_rotation` is counterclockwise.
                display_rotation: Some("90"),
                matrix: YuvMatrix::Bt601,
                full_range: false,
                rotation: Rotation::Cw270,
            },
        ];
        for clip in clips {
            let Some(mut path) = encode_clip(clip.name, clip.source, clip.args) else {
                return;
            };
            if let Some(degrees) = clip.display_rotation {
                let rotated = path.with_extension("rotated.mp4");
                let status = Command::new("ffmpeg")
                    .args(["-y", "-hide_banner", "-loglevel", "error"])
                    .args(["-display_rotation", degrees, "-i"])
                    .arg(&path)
                    .args(["-c", "copy"])
                    .arg(&rotated)
                    .status()
                    .expect("run ffmpeg");
                assert!(status.success(), "{}: remux", clip.name);
                let _ = fs::remove_file(&path);
                path = rotated;
            }
            let resolver = open_cpu(&path).expect("open clip");
            let decoded = yuv_frame(resolver.frame(0).expect("decode frame"));
            let format = decoded.format;
            let name = clip.name;
            assert_eq!(format.color.matrix, clip.matrix, "{name}");
            assert_eq!(format.color.full_range, clip.full_range, "{name}");
            assert_eq!(format.rotation, clip.rotation, "{name}");
            assert_eq!(format.subsampling, ChromaSubsampling::Yuv420, "{name}");
            assert_eq!((format.depth, format.msb_aligned), (8, false), "{name}");
            assert!(matches!(decoded.layout.chroma, YuvChroma::Planar { .. }));
            assert!(decoded.layout.alpha.is_none(), "{name}");
            // The frame keeps its coded size and the stream's size is the upright one.
            let metadata = resolver.metadata();
            assert_eq!(
                format.display_size(),
                (metadata.width, metadata.height),
                "{name}"
            );
            let _ = fs::remove_file(path);
        }
    }

    #[test]
    fn unusual_yuv_sources_reshape_without_changing_luma_and_rgb_sources_stay_rgba() {
        let encode = |name: &str, pixel_format: &str| {
            let path =
                std::env::temp_dir().join(format!("lumen_engine_{}_{name}", std::process::id()));
            let output = Command::new("ffmpeg")
                .args(["-y", "-hide_banner", "-loglevel", "error", "-f", "lavfi"])
                .args([
                    "-i",
                    "nullsrc=s=32x32:r=30:d=0.5,geq=lum='16+3*N':cb=128:cr=128,format=yuv420p",
                ])
                .args(["-pix_fmt", pixel_format, "-c:v", "ffv1"])
                .arg(&path)
                .output();
            match output {
                Ok(output) if output.status.success() => Some(path),
                _ => {
                    eprintln!("ffmpeg CLI with ffv1 unavailable; skipping {name}");
                    None
                }
            }
        };
        let (Some(yuv411), Some(rgb)) =
            (encode("yuv411.mkv", "yuv411p"), encode("rgb.mkv", "bgr0"))
        else {
            return;
        };

        let reshaped = open_cpu(&yuv411)
            .expect("open clip")
            .frame(5)
            .expect("decode");
        assert_eq!(frame_index(&reshaped), 5);
        let reshaped = yuv_frame(reshaped);
        assert_eq!(reshaped.format.subsampling, ChromaSubsampling::Yuv422);
        assert_eq!(reshaped.data.len(), 32 * 32 * 2);
        reshaped.validate().expect("planes fit the buffer");

        let rgb_frame = open_cpu(&rgb).expect("open clip").frame(5).expect("decode");
        assert!(matches!(rgb_frame, MediaFrame::CpuRgba(_)));
        let _ = fs::remove_file(yuv411);
        let _ = fs::remove_file(rgb);
    }

    #[test]
    fn opaque_sources_report_no_alpha() {
        let Some(path) = encode_counting_clip("opaque.mp4", &[]) else {
            return;
        };
        let input = open_input(&path.to_string_lossy()).expect("open clip");
        assert!(!input.best_video_stream().expect("video stream").has_alpha);
        let _ = fs::remove_file(path);
    }
}
