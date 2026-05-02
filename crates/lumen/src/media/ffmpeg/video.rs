use std::{
    collections::{BTreeSet, HashSet},
    sync::{Arc, Mutex, mpsc},
    thread,
};

use ffmpeg::{
    Packet,
    codec::{self, Id as CodecId, decoder::find_by_name},
    format::Pixel,
    media::Type,
    software::scaling::{context::Context as ScalingContext, flag::Flags},
    util::{error::EAGAIN, frame::video::Video},
};
use ffmpeg_next as ffmpeg;

use crate::{
    error::MediaError,
    gpu_image::GpuImageFrame,
    media::{VideoFrameResolver, VideoMetadata},
};

use super::{
    ensure_ffmpeg_init,
    image::{FrameImage, FrameLruCache},
    rational_to_f64,
};

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

    fn resolve_frame(&mut self, frame: u32) -> Result<Arc<GpuImageFrame>, MediaError> {
        if let Some(cached) = self.cache.get(frame) {
            return Ok(cached);
        }

        let decoded = self.decoder.decode_frame(frame)?.into_gpu_image()?;
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
    format: ffmpeg::format::context::Input,
    decoder: ffmpeg::decoder::Video,
    scaler: ScalingContext,
    next_frame_hint: u32,
    sent_eof: bool,
}

struct DecoderComponents {
    format: ffmpeg::format::context::Input,
    stream_index: usize,
    width: u32,
    height: u32,
    frame_count: u32,
    fps: f64,
    time_base_seconds: f64,
    decoder: ffmpeg::decoder::Video,
    scaler: ScalingContext,
}

impl LibavStreamDecoder {
    fn open(source: impl Into<String>, prefer_hardware_decode: bool) -> Result<Self, MediaError> {
        ensure_ffmpeg_init()?;
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
            format: components.format,
            decoder: components.decoder,
            scaler: components.scaler,
            next_frame_hint: 0,
            sent_eof: false,
        })
    }

    fn open_components(
        source: &str,
        prefer_hardware_decode: bool,
    ) -> Result<DecoderComponents, MediaError> {
        let format = ffmpeg::format::input(source).map_err(|err| MediaError::Decode {
            media_source: source.to_string(),
            details: format!("failed opening media source: {err}"),
        })?;

        let stream =
            format
                .streams()
                .best(Type::Video)
                .ok_or_else(|| MediaError::SourceNotFound {
                    media_source: source.to_string(),
                })?;
        let stream_index = stream.index();
        let stream_time_base = rational_to_f64(stream.time_base()).unwrap_or(1.0 / 30.0);
        let fps = resolve_stream_fps(&stream, source)?;
        let parameters = stream.parameters();
        let decoder = open_video_decoder(parameters, prefer_hardware_decode, source)?;
        let width = decoder.width();
        let height = decoder.height();
        let frame_count = resolve_frame_count(&stream, fps);

        let scaler = ScalingContext::get(
            decoder.format(),
            width,
            height,
            Pixel::RGBA,
            width,
            height,
            Flags::BILINEAR,
        )
        .map_err(|err| MediaError::Decode {
            media_source: source.to_string(),
            details: format!("failed creating color conversion pipeline: {err}"),
        })?;

        Ok(DecoderComponents {
            format,
            stream_index,
            width,
            height,
            frame_count,
            fps,
            time_base_seconds: stream_time_base.max(1e-12),
            decoder,
            scaler,
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

            let mut packet = Packet::empty();
            match packet.read(&mut self.format) {
                Ok(()) => {
                    if packet.stream() != self.stream_index {
                        continue;
                    }
                    self.decoder
                        .send_packet(&packet)
                        .map_err(|err| MediaError::Decode {
                            media_source: self.source.clone(),
                            details: format!("failed sending packet to decoder: {err}"),
                        })?;
                }
                Err(ffmpeg::Error::Eof) => {
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
        let mut decoded = Video::empty();
        loop {
            match self.decoder.receive_frame(&mut decoded) {
                Ok(()) => {
                    let decoded_frame = self.map_frame_index(&decoded);
                    self.next_frame_hint = decoded_frame.saturating_add(1);
                    if decoded_frame < target_frame {
                        continue;
                    }
                    return self.frame_to_rgba(&decoded).map(Some);
                }
                Err(ffmpeg::Error::Other { errno }) if errno == EAGAIN => return Ok(None),
                Err(ffmpeg::Error::Eof) => return Ok(None),
                Err(err) => {
                    return Err(MediaError::Decode {
                        media_source: self.source.clone(),
                        details: format!("failed receiving decoded frame: {err}"),
                    });
                }
            }
        }
    }

    fn map_frame_index(&self, decoded: &Video) -> u32 {
        decoded
            .timestamp()
            .or(decoded.pts())
            .map(|timestamp| self.timestamp_to_frame(timestamp))
            .filter(|frame| *frame <= self.frame_count.saturating_sub(1))
            .unwrap_or(self.next_frame_hint)
    }

    fn frame_to_rgba(&mut self, decoded: &Video) -> Result<FrameImage, MediaError> {
        let mut rgba = Video::empty();
        self.scaler
            .run(decoded, &mut rgba)
            .map_err(|err| MediaError::Decode {
                media_source: self.source.clone(),
                details: format!("pixel format conversion failed: {err}"),
            })?;

        let row_bytes = usize::try_from(self.width)
            .unwrap_or_default()
            .saturating_mul(4);
        let total_bytes =
            row_bytes.saturating_mul(usize::try_from(self.height).unwrap_or_default());
        let mut output = vec![0_u8; total_bytes];
        let stride = rgba.stride(0);
        let data = rgba.data(0);

        for row in 0..usize::try_from(self.height).unwrap_or_default() {
            let src_start = row.saturating_mul(stride);
            let src_end = src_start.saturating_add(row_bytes);
            let dst_start = row.saturating_mul(row_bytes);
            let dst_end = dst_start.saturating_add(row_bytes);
            if src_end > data.len() || dst_end > output.len() {
                return Err(MediaError::Decode {
                    media_source: self.source.clone(),
                    details: "decoded frame buffer dimensions are invalid".to_string(),
                });
            }
            output[dst_start..dst_end].copy_from_slice(&data[src_start..src_end]);
        }

        Ok(FrameImage {
            source: self.source.clone(),
            width: self.width,
            height: self.height,
            rgba: output,
        })
    }

    fn should_seek(&self, target_frame: u32) -> bool {
        target_frame < self.next_frame_hint
            || target_frame.saturating_sub(self.next_frame_hint) > SEEK_REOPEN_THRESHOLD
    }

    fn seek_to_frame(&mut self, target_frame: u32) -> Result<(), MediaError> {
        let timestamp = self.frame_to_timestamp(target_frame);
        if self.format.seek(timestamp, ..).is_err() {
            self.reopen()?;
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
        self.format = components.format;
        self.stream_index = components.stream_index;
        self.width = components.width;
        self.height = components.height;
        self.frame_count = components.frame_count;
        self.fps = components.fps;
        self.time_base_seconds = components.time_base_seconds;
        self.decoder = components.decoder;
        self.scaler = components.scaler;
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
        response_tx: mpsc::Sender<Result<Arc<GpuImageFrame>, MediaError>>,
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

    fn frame(&self, frame: u32) -> Result<Arc<GpuImageFrame>, MediaError> {
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

fn resolve_stream_fps(
    stream: &ffmpeg::format::stream::Stream<'_>,
    source: &str,
) -> Result<f64, MediaError> {
    let fps = [
        stream.avg_frame_rate(),
        stream.rate(),
        stream.time_base().invert(),
    ]
    .iter()
    .find_map(|rational| rational_to_f64(*rational))
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

fn resolve_frame_count(stream: &ffmpeg::format::stream::Stream<'_>, fps: f64) -> u32 {
    let explicit = stream.frames();
    if explicit > 0 {
        return u32::try_from(explicit).unwrap_or(u32::MAX);
    }

    let duration = stream.duration();
    let Some(time_base_seconds) = rational_to_f64(stream.time_base()) else {
        return 1;
    };
    if duration <= 0 || fps <= 0.0 {
        return 1;
    }

    let estimated = ((duration as f64) * time_base_seconds * fps).round();
    estimated.clamp(1.0, f64::from(u32::MAX)) as u32
}

fn open_video_decoder(
    parameters: codec::Parameters,
    prefer_hardware_decode: bool,
    source: &str,
) -> Result<ffmpeg::decoder::Video, MediaError> {
    if prefer_hardware_decode {
        for codec_name in hardware_decoder_candidates(parameters.id()) {
            if let Some(candidate) = find_by_name(codec_name) {
                let Ok(context) = codec::context::Context::from_parameters(parameters.clone())
                else {
                    continue;
                };
                if let Ok(opened) = context.decoder().open_as(candidate)
                    && let Ok(video_decoder) = opened.video()
                {
                    return Ok(video_decoder);
                }
            }
        }
    }

    codec::context::Context::from_parameters(parameters)
        .map_err(|err| MediaError::Decode {
            media_source: source.to_string(),
            details: format!("failed creating decoder context: {err}"),
        })?
        .decoder()
        .video()
        .map_err(|err| MediaError::Decode {
            media_source: source.to_string(),
            details: format!("failed opening software decoder: {err}"),
        })
}

fn hardware_decoder_candidates(codec_id: CodecId) -> &'static [&'static str] {
    match codec_id {
        CodecId::H264 => &[
            "h264_videotoolbox",
            "h264_nvdec",
            "h264_cuvid",
            "h264_qsv",
            "h264_vaapi",
        ],
        CodecId::HEVC | CodecId::H265 => &[
            "hevc_videotoolbox",
            "hevc_nvdec",
            "hevc_cuvid",
            "hevc_qsv",
            "hevc_vaapi",
        ],
        CodecId::VP9 => &["vp9_videotoolbox", "vp9_qsv", "vp9_vaapi"],
        CodecId::AV1 => &["av1_videotoolbox", "av1_qsv", "av1_vaapi"],
        _ => &[],
    }
}
