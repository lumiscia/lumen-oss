use std::{
    collections::{BTreeSet, HashSet},
    sync::{Arc, Mutex, mpsc},
    thread,
};

use lumen_ffmpeg::{
    CpuVideoFrame, DecodeMode, GpuBackend, GpuVideoFrame, InputContext, VideoDecoder,
    VideoDecoderConfig, VideoStreamInfo,
};

use crate::{
    error::MediaError,
    media::{
        GpuVideoMediaFrame, MediaFrame, VideoFrameResolver, VideoMetadata, resolver::CpuRetry,
    },
};

use super::image::{FrameImage, FrameLruCache};

const SEEK_REOPEN_THRESHOLD: u32 = 120;

#[derive(Debug, Clone, Default)]
pub struct FfmpegResolverOptions {
    pub prefer_hardware_decode: bool,
    /// The renderer's device, for hardware decode straight to its textures. Without it, Linux builds
    /// with `cuda` use CUDA and others decode on the CPU.
    #[cfg(all(target_os = "linux", feature = "vulkan"))]
    pub vulkan_device: Option<lumen_ffmpeg::VulkanDevice>,
}

impl FfmpegResolverOptions {
    pub fn with_hardware_decode(prefer_hardware_decode: bool) -> Self {
        Self {
            prefer_hardware_decode,
            ..Self::default()
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

        let decoded = self.decoder.decode_frame(frame)?;
        self.cache.insert(frame, decoded.clone(), false);
        Ok(decoded)
    }

    fn has_room_to_prefetch(&self) -> bool {
        self.cache.has_room_for(self.decoder.frame_bytes())
    }

    /// Moves the source to the CPU, warning unless it is already there, and decodes `frame` on it.
    fn resolve_on_cpu(&mut self, frame: u32, reason: &str) -> Result<MediaFrame, MediaError> {
        if matches!(
            self.decoder.gpu_decode_status,
            GpuDecodeStatus::Active { .. }
        ) {
            tracing::warn!(
                source = self.decoder.source,
                reason,
                "hardware decode failed, decoding this source on the CPU"
            );
            self.decoder.options.prefer_hardware_decode = false;
            self.decoder.reopen()?;
        }
        // A source that moved to the CPU on a decode failure may still have GPU frames cached.
        self.cache.retain(&[]);
        self.resolve_frame(frame)
    }

    fn prefetch_frame(&mut self, frame: u32) {
        if !self.cache.contains(frame)
            && let Ok(decoded) = self.decoder.decode_frame(frame)
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
    frame_count: u32,
    fps: f64,
    time_base_seconds: f64,
    start_time: i64,
    input: InputContext,
    decoder: VideoDecoder,
    next_frame_hint: u32,
    sent_eof: bool,
    gpu_decode_status: GpuDecodeStatus,
}

struct DecoderComponents {
    input: InputContext,
    stream_index: usize,
    width: u32,
    height: u32,
    frame_count: u32,
    fps: f64,
    time_base_seconds: f64,
    start_time: i64,
    decoder: VideoDecoder,
    gpu_decode_status: GpuDecodeStatus,
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
                backend: GpuBackend::Cuda,
            } => "gpu_cuda",
            Self::Active {
                backend: GpuBackend::Metal,
            } => "gpu_metal",
            Self::Active {
                backend: GpuBackend::Vulkan,
            } => "gpu_vulkan",
            Self::Unavailable {
                backend: GpuBackend::Cuda,
                ..
            } => "cpu_cuda_unavailable",
            Self::Unavailable {
                backend: GpuBackend::Metal,
                ..
            } => "cpu_metal_unavailable",
            Self::Unavailable {
                backend: GpuBackend::Vulkan,
                ..
            } => "cpu_vulkan_unavailable",
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
    fn open(
        source: impl Into<String>,
        mut options: FfmpegResolverOptions,
    ) -> Result<Self, MediaError> {
        let source = source.into();
        let components = Self::open_components(&source, &mut options)?;

        Ok(Self {
            source,
            options,
            worker: None,
            stream_index: components.stream_index,
            width: components.width,
            height: components.height,
            frame_count: components.frame_count,
            fps: components.fps,
            time_base_seconds: components.time_base_seconds,
            start_time: components.start_time,
            input: components.input,
            decoder: components.decoder,
            next_frame_hint: 0,
            sent_eof: false,
            gpu_decode_status: components.gpu_decode_status,
        })
    }

    /// Opens the source, and clears `prefer_hardware_decode` if the hardware decoder won't open, so
    /// the source stays on the CPU and warns once.
    fn open_components(
        source: &str,
        options: &mut FfmpegResolverOptions,
    ) -> Result<DecoderComponents, MediaError> {
        let mut input = open_input(source)?;
        let stream = input
            .best_video_stream()
            .map_err(|err| MediaError::Decode {
                media_source: source.to_string(),
                details: format!("failed selecting video stream: {err}"),
            })?;
        let stream_index = stream.stream_index;
        let time_base_seconds = stream
            .time_base
            .as_f64()
            .filter(|seconds| *seconds > 0.0)
            .ok_or_else(|| MediaError::Decode {
                media_source: source.to_string(),
                details: "stream has no valid time base".to_string(),
            })?;
        let fps = resolve_stream_fps(&stream, source)?;
        let width = stream.width;
        let height = stream.height;
        let frame_count = match resolve_frame_count(&stream, input.presented_duration_us(), fps) {
            Some(frame_count) => Some(frame_count),
            None if input.is_image() => {
                let frame_count = count_frames(&mut input, stream_index);
                // An image packet can hold the whole file, so the spent input goes before the next
                // one reads it again.
                drop(input);
                input = open_input(source)?;
                frame_count
            }
            None => None,
        }
        .ok_or_else(|| MediaError::Decode {
            media_source: source.to_string(),
            details: "unable to determine stream frame count".to_string(),
        })?;
        let (decoder, gpu_decode_status) = open_video_decoder(source, &input, &stream, options)?;
        if let Some(reason) = gpu_decode_status.unavailable_reason() {
            tracing::warn!(reason, "hardware decode unavailable, decoding on the CPU");
            options.prefer_hardware_decode = false;
        }

        Ok(DecoderComponents {
            input,
            stream_index,
            width,
            height,
            frame_count,
            fps,
            time_base_seconds,
            start_time: stream.start_time,
            decoder,
            gpu_decode_status,
        })
    }

    fn decode_frame(&mut self, frame: u32) -> Result<MediaFrame, MediaError> {
        if frame >= self.frame_count {
            return Err(MediaError::FrameOutOfRange {
                media_source: self.source.clone(),
                frame,
                frame_count: self.frame_count,
            });
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
                if hardware_failed {
                    tracing::warn!(
                        source = self.source,
                        error = %primary_error,
                        "hardware decode failed, decoding this source on the CPU"
                    );
                    self.options.prefer_hardware_decode = false;
                }
                self.reopen()?;
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
                    self.decoder
                        .send_packet(&packet)
                        .map_err(|err| MediaError::Decode {
                            media_source: self.source.clone(),
                            details: format!("failed sending packet to decoder: {err}"),
                        })?;
                }
                Ok(None) => {
                    self.decoder.send_eof().map_err(|err| MediaError::Decode {
                        media_source: self.source.clone(),
                        details: format!("failed sending decoder EOF: {err}"),
                    })?;
                    self.sent_eof = true;
                }
                Err(err) => {
                    return Err(MediaError::Decode {
                        media_source: self.source.clone(),
                        details: format!("packet read failed: {err}"),
                    });
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
                match self.decoder.receive_gpu_frame() {
                    Ok(Some(decoded)) => {
                        let decoded_frame = self.map_gpu_frame_index(&decoded);
                        self.next_frame_hint = decoded_frame.saturating_add(1);
                        if decoded_frame < target_frame {
                            continue;
                        }
                        return Ok(Some(MediaFrame::GpuVideo(Arc::new(GpuVideoMediaFrame {
                            frame: decoded,
                            cpu_retry: self.cpu_retry(target_frame),
                        }))));
                    }
                    Ok(None) => return Ok(None),
                    Err(err) => {
                        return Err(MediaError::Decode {
                            media_source: self.source.clone(),
                            details: format!("failed receiving decoded GPU frame: {err}"),
                        });
                    }
                }
            } else {
                match self.decoder.receive_rgba_frame() {
                    Ok(Some(decoded)) => {
                        let decoded_frame = self.map_frame_index(&decoded);
                        self.next_frame_hint = decoded_frame.saturating_add(1);
                        if decoded_frame < target_frame {
                            continue;
                        }
                        return self
                            .frame_to_image(decoded)
                            .and_then(FrameImage::into_media_frame)
                            .map(MediaFrame::CpuRgba)
                            .map(Some);
                    }
                    Ok(None) => return Ok(None),
                    Err(err) => {
                        return Err(MediaError::Decode {
                            media_source: self.source.clone(),
                            details: format!("failed receiving decoded frame: {err}"),
                        });
                    }
                }
            }
        }
    }

    fn cpu_retry(&self, frame: u32) -> CpuRetry {
        let source = self.source.clone();
        let worker = self.worker.clone();
        Box::new(move |reason| {
            let unavailable = || MediaError::Decode {
                media_source: source.clone(),
                details: "video decode worker is unavailable".to_string(),
            };
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

    fn map_frame_index(&self, decoded: &CpuVideoFrame) -> u32 {
        decoded
            .pts
            .map(|timestamp| self.timestamp_to_frame(timestamp))
            .filter(|frame| *frame <= self.frame_count.saturating_sub(1))
            .unwrap_or(self.next_frame_hint)
    }

    fn map_gpu_frame_index(&self, decoded: &GpuVideoFrame) -> u32 {
        decoded
            .pts()
            .map(|timestamp| self.timestamp_to_frame(timestamp))
            .filter(|frame| *frame <= self.frame_count.saturating_sub(1))
            .unwrap_or(self.next_frame_hint)
    }

    fn frame_to_image(&self, decoded: CpuVideoFrame) -> Result<FrameImage, MediaError> {
        let row_bytes = (decoded.width as usize).saturating_mul(4);
        if decoded.stride != row_bytes {
            return Err(MediaError::Decode {
                media_source: self.source.clone(),
                details: format!(
                    "decoded frame stride {} does not match expected RGBA stride {row_bytes}",
                    decoded.stride
                ),
            });
        }

        Ok(FrameImage {
            source: self.source.clone(),
            width: decoded.width,
            height: decoded.height,
            rgba: decoded.data,
            premultiply: false,
        })
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
                .map_err(|err| MediaError::Decode {
                    media_source: self.source.clone(),
                    details: format!("seek failed: {err}"),
                })?;
        }
        self.decoder.flush();
        self.sent_eof = false;
        self.next_frame_hint = target_frame;
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
        self.width as usize * self.height as usize * 4
    }

    fn reopen(&mut self) -> Result<(), MediaError> {
        let components = Self::open_components(&self.source, &mut self.options)?;
        self.input = components.input;
        self.stream_index = components.stream_index;
        self.width = components.width;
        self.height = components.height;
        self.frame_count = components.frame_count;
        self.fps = components.fps;
        self.time_base_seconds = components.time_base_seconds;
        self.start_time = components.start_time;
        self.decoder = components.decoder;
        self.gpu_decode_status = components.gpu_decode_status;
        self.next_frame_hint = 0;
        self.sent_eof = false;
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

fn run_decode_worker(
    source: String,
    options: FfmpegResolverOptions,
    request_tx: mpsc::Sender<WorkerRequest>,
    request_rx: mpsc::Receiver<WorkerRequest>,
) {
    let mut worker = match LibavStreamDecoder::open(source.clone(), options) {
        Ok(mut decoder) => {
            decoder.worker = Some(request_tx);
            Ok(VideoDecodeWorker::new(decoder))
        }
        Err(error) => Err(error),
    };
    let mut pending = BTreeSet::new();

    loop {
        let can_prefetch = |worker: &Result<VideoDecodeWorker, MediaError>| {
            worker
                .as_ref()
                .is_ok_and(VideoDecodeWorker::has_room_to_prefetch)
        };

        if pending.is_empty() || !can_prefetch(&worker) {
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

        if can_prefetch(&worker)
            && let Some(frame) = pending.pop_first()
            && let Ok(worker) = &mut worker
        {
            worker.prefetch_frame(frame);
        }
    }
}

fn handle_worker_request(
    request: WorkerRequest,
    worker: &mut Result<VideoDecodeWorker, MediaError>,
    pending: &mut BTreeSet<u32>,
) -> bool {
    match request {
        WorkerRequest::Enqueue { frame } => {
            pending.insert(frame);
            false
        }
        WorkerRequest::Resolve { frame, response_tx } => {
            pending.remove(&frame);
            let result = match worker {
                Ok(worker) => worker.resolve_frame(frame),
                Err(error) => Err(error.clone()),
            };
            let _ = response_tx.send(result);
            false
        }
        WorkerRequest::ResolveOnCpu {
            frame,
            reason,
            response_tx,
        } => {
            pending.remove(&frame);
            let result = match worker {
                Ok(worker) => worker.resolve_on_cpu(frame, &reason),
                Err(error) => Err(error.clone()),
            };
            let _ = response_tx.send(result);
            false
        }
        WorkerRequest::Retain { frames } => {
            pending.retain(|frame| frames.binary_search(frame).is_ok());
            if let Ok(worker) = worker {
                worker.retain_frames(&frames);
            }
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
        let decoder = LibavStreamDecoder::open(source.clone(), options)?;
        let metadata = VideoMetadata {
            width: decoder.width,
            height: decoder.height,
            frame_count: decoder.frame_count,
            fps: decoder.fps as f32,
        };
        let decode_status = decoder.gpu_decode_status.clone();
        let options = decoder.options.clone();
        drop(decoder);

        let (request_tx, request_rx) = mpsc::channel::<WorkerRequest>();
        let worker_source = source.clone();
        let worker_tx = request_tx.clone();
        let worker_handle =
            thread::spawn(move || run_decode_worker(worker_source, options, worker_tx, request_rx));

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
    fn id(&self) -> &str {
        &self.id
    }

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
            .map_err(|_| MediaError::Decode {
                media_source: self.id.clone(),
                details: "video decode worker is unavailable".to_string(),
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
            .map_err(|_| MediaError::Decode {
                media_source: self.id.clone(),
                details: "video decode worker is unavailable".to_string(),
            })?;

        response_rx.recv().map_err(|_| MediaError::Decode {
            media_source: self.id.clone(),
            details: format!("video decode worker did not return frame {frame}"),
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

fn resolve_stream_fps(stream: &VideoStreamInfo, source: &str) -> Result<f64, MediaError> {
    stream
        .frame_rate
        .and_then(|rate| rate.as_f64())
        .filter(|fps| *fps > 0.0)
        .ok_or_else(|| MediaError::Decode {
            media_source: source.to_string(),
            details: "unable to determine stream frame rate".to_string(),
        })
}

fn resolve_frame_count(
    stream: &VideoStreamInfo,
    container_duration_us: Option<i64>,
    fps: f64,
) -> Option<u32> {
    if let Some(explicit) = stream.frame_count {
        return Some(u32::try_from(explicit).unwrap_or(u32::MAX));
    }

    let stream_seconds = stream
        .duration_ts
        .zip(stream.time_base.as_f64())
        .map(|(duration, time_base)| duration as f64 * time_base);
    let container_seconds = container_duration_us.map(|duration| duration as f64 / 1_000_000.0);
    let seconds = stream_seconds.or(container_seconds)?;
    Some((seconds * fps).round().clamp(1.0, f64::from(u32::MAX)) as u32)
}

fn open_input(source: &str) -> Result<InputContext, MediaError> {
    InputContext::open(source.to_string()).map_err(|err| MediaError::Decode {
        media_source: source.to_string(),
        details: format!("failed opening media source: {err}"),
    })
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
    let opened = if stream.rotation != 0 {
        // GPU frames stay in coded orientation.
        Err("rotated sources need the CPU path's rotation".to_string())
    } else if stream.has_alpha {
        // GPU frames carry no alpha.
        Err("sources with alpha need the CPU path's RGBA conversion".to_string())
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
    .map_err(|err| MediaError::Decode {
        media_source: source.to_string(),
        details: format!("failed opening CPU video decoder: {err}"),
    })
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
    } else if cfg!(all(target_os = "linux", feature = "cuda")) {
        GpuBackend::Cuda
    } else {
        GpuBackend::Vulkan
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf, process::Command};

    use super::*;

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

    fn frame_index(frame: &MediaFrame) -> u32 {
        let MediaFrame::CpuRgba(decoded) = frame else {
            panic!("expected a CPU frame");
        };
        // Limited-range luma back to full-range grey, then to the index it encodes.
        let luma = f64::from(decoded.rgba[0]) * 219.0 / 255.0 + 16.0;
        ((luma - 16.0) / 3.0).round() as u32
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
            let decoded = decoder.decode_frame(frame).expect("decode on the CPU");
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
    fn a_frame_the_renderer_rejects_moves_the_source_to_the_cpu_and_drops_cached_gpu_frames() {
        let Some(path) = encode_counting_clip("import_failure.mp4", &[]) else {
            return;
        };
        let mut decoder =
            LibavStreamDecoder::open(path.to_string_lossy().to_string(), Default::default())
                .expect("open clip");
        let cached = decoder.decode_frame(8).expect("frame to retire");
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
        let MediaFrame::CpuRgba(decoded) = resolver.frame(0).expect("decode frame") else {
            panic!("expected a CPU frame");
        };
        assert!(
            (i32::from(decoded.rgba[3]) - 128).abs() <= 2,
            "{:?}",
            &decoded.rgba[..4]
        );
        let _ = fs::remove_file(path);
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
