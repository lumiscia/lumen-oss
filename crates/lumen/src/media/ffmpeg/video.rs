use std::{
    collections::{BTreeSet, HashSet},
    sync::{Arc, Mutex, mpsc},
    thread,
};

use lumen_ffmpeg::{
    CpuVideoFrame, DecodeMode, GpuBackend, InputContext, Rational, VideoDecoder,
    VideoDecoderConfig, VideoStreamInfo,
};

use crate::{
    error::MediaError,
    media::{CpuMediaFrame, MediaFrame, VideoFrameResolver, VideoMetadata},
};

use super::image::{FrameImage, FrameLruCache};

const SEEK_REOPEN_THRESHOLD: u32 = 120;

#[derive(Debug, Clone, Copy)]
pub struct FfmpegResolverOptions {
    pub prefer_hardware_decode: bool,
}

impl Default for FfmpegResolverOptions {
    fn default() -> Self {
        Self {
            prefer_hardware_decode: true,
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

    fn resolve_frame(&mut self, frame: u32) -> Result<Arc<CpuMediaFrame>, MediaError> {
        if let Some(cached) = self.cache.get(frame) {
            return Ok(cached);
        }

        let decoded = self.decoder.decode_frame(frame)?.into_media_frame()?;
        self.cache.insert(frame, Arc::clone(&decoded));
        Ok(decoded)
    }

    fn retain_frames(&mut self, frames: &[u32]) {
        self.cache.retain(frames);
    }
}

struct LibavStreamDecoder {
    source: String,
    prefer_hardware_decode: bool,
    stream_index: usize,
    width: u32,
    height: u32,
    frame_count: u32,
    fps: f64,
    time_base_seconds: f64,
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
    decoder: VideoDecoder,
    gpu_decode_status: GpuDecodeStatus,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
enum GpuDecodeStatus {
    NotRequested,
    AvailableButNotImported { backend: GpuBackend },
    Unavailable { backend: GpuBackend, reason: String },
}

impl LibavStreamDecoder {
    fn open(source: impl Into<String>, prefer_hardware_decode: bool) -> Result<Self, MediaError> {
        let source = source.into();
        let components = Self::open_components(&source, prefer_hardware_decode)?;

        Ok(Self {
            source,
            prefer_hardware_decode,
            stream_index: components.stream_index,
            width: components.width,
            height: components.height,
            frame_count: components.frame_count,
            fps: components.fps,
            time_base_seconds: components.time_base_seconds,
            input: components.input,
            decoder: components.decoder,
            next_frame_hint: 0,
            sent_eof: false,
            gpu_decode_status: components.gpu_decode_status,
        })
    }

    fn open_components(
        source: &str,
        prefer_hardware_decode: bool,
    ) -> Result<DecoderComponents, MediaError> {
        let input = InputContext::open(source.to_string()).map_err(|err| MediaError::Decode {
            media_source: source.to_string(),
            details: format!("failed opening media source: {err}"),
        })?;
        let stream = input
            .best_video_stream()
            .map_err(|err| MediaError::Decode {
                media_source: source.to_string(),
                details: format!("failed selecting video stream: {err}"),
            })?;
        let stream_index = stream.stream_index;
        let stream_time_base = stream.time_base.as_f64().unwrap_or(1.0 / 30.0);
        let fps = resolve_stream_fps(&stream, source)?;
        let width = stream.width;
        let height = stream.height;
        let frame_count = resolve_frame_count(&stream, fps);
        let gpu_decode_status = probe_gpu_decode(source, &input, &stream, prefer_hardware_decode);
        let decoder = VideoDecoder::open(
            &input,
            VideoDecoderConfig {
                stream_index,
                mode: DecodeMode::Cpu,
            },
        )
        .map_err(|err| MediaError::Decode {
            media_source: source.to_string(),
            details: format!("failed opening CPU video decoder: {err}"),
        })?;

        Ok(DecoderComponents {
            input,
            stream_index,
            width,
            height,
            frame_count,
            fps,
            time_base_seconds: stream_time_base.max(1e-12),
            decoder,
            gpu_decode_status,
        })
    }

    fn decode_frame(&mut self, frame: u32) -> Result<FrameImage, MediaError> {
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
                self.reopen()?;
                self.seek_to_frame(frame)?;
                self.decode_frame_inner(frame).map_err(|fallback_error| {
                    if matches!(fallback_error, MediaError::FrameOutOfRange { .. }) {
                        fallback_error
                    } else {
                        primary_error
                    }
                })
            }
        }
    }

    fn decode_frame_inner(&mut self, target_frame: u32) -> Result<FrameImage, MediaError> {
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
    ) -> Result<Option<FrameImage>, MediaError> {
        loop {
            match self.decoder.receive_cpu_frame() {
                Ok(Some(decoded)) => {
                    let decoded_frame = self.map_frame_index(&decoded);
                    self.next_frame_hint = decoded_frame.saturating_add(1);
                    if decoded_frame < target_frame {
                        continue;
                    }
                    return self.frame_to_image(decoded).map(Some);
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

    fn map_frame_index(&self, decoded: &CpuVideoFrame) -> u32 {
        decoded
            .pts
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
        if self.fps <= 0.0 || self.time_base_seconds <= 0.0 {
            return i64::from(frame);
        }
        let seconds = f64::from(frame) / self.fps;
        (seconds / self.time_base_seconds).round() as i64
    }

    fn timestamp_to_frame(&self, timestamp: i64) -> u32 {
        if timestamp <= 0 || self.fps <= 0.0 {
            return 0;
        }
        let seconds = (timestamp as f64) * self.time_base_seconds;
        (seconds * self.fps).round() as u32
    }

    fn reopen(&mut self) -> Result<(), MediaError> {
        let components = Self::open_components(&self.source, self.prefer_hardware_decode)?;
        self.input = components.input;
        self.stream_index = components.stream_index;
        self.width = components.width;
        self.height = components.height;
        self.frame_count = components.frame_count;
        self.fps = components.fps;
        self.time_base_seconds = components.time_base_seconds;
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
        response_tx: mpsc::Sender<Result<Arc<CpuMediaFrame>, MediaError>>,
    },
    Retain {
        frames: Vec<u32>,
    },
    Shutdown,
}

fn run_decode_worker(
    source: String,
    options: FfmpegResolverOptions,
    request_rx: mpsc::Receiver<WorkerRequest>,
) {
    let mut worker = match LibavStreamDecoder::open(source.clone(), options.prefer_hardware_decode)
    {
        Ok(decoder) => Ok(VideoDecodeWorker::new(decoder)),
        Err(error) => Err(error),
    };
    let mut pending = BTreeSet::new();

    while let Ok(request) = request_rx.recv() {
        if handle_worker_request(request, &mut worker, &mut pending) {
            break;
        }

        while let Ok(request) = request_rx.try_recv() {
            if handle_worker_request(request, &mut worker, &mut pending) {
                return;
            }
        }

        if let Some(frame) = pending.pop_first()
            && let Ok(worker) = &mut worker
        {
            let _ = worker.resolve_frame(frame);
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
        let decoder = LibavStreamDecoder::open(source.clone(), options.prefer_hardware_decode)?;
        let metadata = VideoMetadata {
            width: decoder.width,
            height: decoder.height,
            frame_count: decoder.frame_count,
            fps: decoder.fps as f32,
        };
        drop(decoder);

        let (request_tx, request_rx) = mpsc::channel::<WorkerRequest>();
        let worker_source = source.clone();
        let worker_handle =
            thread::spawn(move || run_decode_worker(worker_source, options, request_rx));

        Ok(Self {
            id: source,
            metadata,
            request_tx,
            worker_handle: Mutex::new(Some(worker_handle)),
            scheduled_frames: Mutex::new(HashSet::new()),
        })
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

        response_rx
            .recv()
            .map_err(|_| MediaError::Decode {
                media_source: self.id.clone(),
                details: format!("video decode worker did not return frame {frame}"),
            })?
            .map(MediaFrame::CpuRgba)
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
    let fps = stream
        .avg_frame_rate
        .as_f64()
        .filter(|fps| *fps > 0.0)
        .or_else(|| invert_rational(stream.time_base))
        .unwrap_or(0.0);

    if fps > 0.0 {
        Ok(fps)
    } else {
        Err(MediaError::Decode {
            media_source: source.to_string(),
            details: "unable to determine stream frame rate".to_string(),
        })
    }
}

fn resolve_frame_count(stream: &VideoStreamInfo, fps: f64) -> u32 {
    if let Some(explicit) = stream.frame_count {
        return u32::try_from(explicit).unwrap_or(u32::MAX);
    }

    let Some(duration) = stream.duration_ts else {
        return 1;
    };
    let Some(time_base_seconds) = stream.time_base.as_f64() else {
        return 1;
    };

    if duration <= 0 || fps <= 0.0 {
        return 1;
    }

    let estimated = ((duration as f64) * time_base_seconds * fps).round();
    estimated.clamp(1.0, f64::from(u32::MAX)) as u32
}

fn invert_rational(value: Rational) -> Option<f64> {
    (value.numerator != 0).then_some(value.denominator as f64 / value.numerator as f64)
}

fn probe_gpu_decode(
    source: &str,
    input: &InputContext,
    stream: &VideoStreamInfo,
    prefer_hardware_decode: bool,
) -> GpuDecodeStatus {
    if !prefer_hardware_decode {
        return GpuDecodeStatus::NotRequested;
    }

    let backend = preferred_gpu_backend();
    match VideoDecoder::open(
        input,
        VideoDecoderConfig {
            stream_index: stream.stream_index,
            mode: DecodeMode::Gpu(backend),
        },
    ) {
        Ok(_) => GpuDecodeStatus::AvailableButNotImported { backend },
        Err(error) => GpuDecodeStatus::Unavailable {
            backend,
            reason: format!("{source}: {error}"),
        },
    }
}

fn preferred_gpu_backend() -> GpuBackend {
    if cfg!(target_os = "macos") {
        GpuBackend::Metal
    } else {
        GpuBackend::Vulkan
    }
}
