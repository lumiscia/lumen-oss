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
    error::{MediaError, MediaFailure},
    media::{
        AssetKind, ChromaSubsampling, ColorPrimaries, CpuMediaFrame, GpuVideoMediaFrame,
        MediaFrame, Rotation, TransferFunction, VideoFrameResolver, VideoMetadata, YuvColor,
        YuvFormat, YuvFrame, YuvLayout, YuvMatrix,
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
    /// What the stream's colour tags ask for that it is read without.
    color_warning: Option<String>,
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
        if stream.color.matrix == lumen_ffmpeg::ColorMatrix::Bt2020Cl {
            return Err(MediaFailure::unsupported(&source, CONSTANT_LUMINANCE).into());
        }
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
            color_warning: unsupported_color(stream.color, stream.decodes_as_yuv),
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
            color: yuv_color(decoded.color, decoded.height)
                .map_err(|details| MediaFailure::unsupported(&self.source, details))?,
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
type Startup = Result<(VideoMetadata, GpuDecodeStatus, Option<String>), MediaError>;

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
            let _ = startup_tx.send(Ok((
                decoder.metadata(),
                decoder.gpu_decode_status.clone(),
                decoder.color_warning.clone(),
            )));
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
    color_warning: Option<String>,
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
        let (metadata, decode_status, color_warning) = match startup {
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
            color_warning,
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

    fn color_warning(&self) -> Option<&str> {
        self.color_warning.as_deref()
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

const CONSTANT_LUMINANCE: &str = "BT.2020 constant-luminance YCbCr is not supported; re-encode it with BT.2020 non-constant luminance (FFmpeg `-colorspace bt2020nc`)";

/// The colour FFmpeg's tags describe, or why it can't be read. FCC and SMPTE 240M have no matrix of
/// their own, so they read as the nearest one, and a frame with no usable matrix tag reads by its
/// coded height. Primaries and transfers the renderer doesn't convert read as BT.709 and SDR,
/// which `unsupported_color` names.
pub(crate) fn yuv_color(tags: lumen_ffmpeg::ColorTags, height: u32) -> Result<YuvColor, String> {
    use lumen_ffmpeg::{ColorMatrix, ColorPrimaries as Primaries, ColorTransfer as Transfer};
    let matrix = match tags.matrix {
        ColorMatrix::Bt709 | ColorMatrix::Smpte240m => YuvMatrix::Bt709,
        ColorMatrix::Bt470bg | ColorMatrix::Smpte170m | ColorMatrix::Fcc => YuvMatrix::Bt601,
        ColorMatrix::Bt2020Ncl => YuvMatrix::Bt2020,
        ColorMatrix::Bt2020Cl => return Err(CONSTANT_LUMINANCE.to_string()),
        ColorMatrix::Other => YuvMatrix::untagged(height),
    };
    let primaries = match tags.primaries {
        Primaries::Bt2020 => ColorPrimaries::Bt2020,
        Primaries::Smpte432 => ColorPrimaries::DisplayP3,
        Primaries::Smpte431 => ColorPrimaries::DciP3,
        _ => ColorPrimaries::Bt709,
    };
    let transfer = match tags.transfer {
        Transfer::Smpte2084 => TransferFunction::Pq,
        Transfer::AribStdB67 => TransferFunction::Hlg,
        _ => TransferFunction::Sdr,
    };
    Ok(YuvColor {
        matrix,
        full_range: tags.full_range,
        primaries,
        transfer,
    })
}

/// The tags in `tags` that `yuv_color` reads without converting, as a sentence for a warning.
/// A stream that doesn't decode as YUV has none of its tags converted.
fn unsupported_color(tags: lumen_ffmpeg::ColorTags, decodes_as_yuv: bool) -> Option<String> {
    use lumen_ffmpeg::{ColorPrimaries as Primaries, ColorTransfer as Transfer};
    let wide = matches!(
        tags.primaries,
        Primaries::Bt2020 | Primaries::Smpte431 | Primaries::Smpte432 | Primaries::Other(_)
    );
    let hdr = matches!(
        tags.transfer,
        Transfer::Smpte2084 | Transfer::AribStdB67 | Transfer::Other(_)
    );
    if !decodes_as_yuv {
        return (wide || hdr).then(|| {
            "it is RGB or grey video, which is read as 8-bit sRGB whatever its primaries and \
             transfer, which the renderer converts for YUV video only"
                .to_string()
        });
    }
    let transfer = match tags.transfer {
        Transfer::Other(name) => Some(format!("its `{name}` transfer is read as SDR")),
        _ => None,
    };
    let primaries = match tags.primaries {
        Primaries::Other(name) => Some(format!("its `{name}` primaries are read as BT.709")),
        _ => None,
    };
    let notes: Vec<_> = [transfer, primaries].into_iter().flatten().collect();
    (!notes.is_empty()).then(|| {
        format!(
            "{}, which the renderer doesn't convert",
            notes.join(" and ")
        )
    })
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
#[path = "video_tests.rs"]
mod tests;
