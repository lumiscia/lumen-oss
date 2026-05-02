use std::{
    ffi::CString,
    ptr,
    time::{Duration, Instant},
};

use crate::{
    FfmpegError, Result,
    ffi::{self, AvFrame, AvPacket, sys},
    gpu::{GpuBackend, GpuVideoInput},
    video::{CpuVideoFrame, EncodeMode, PixelFormat, VideoCodec},
};
use sys::{
    AVMediaType::AVMEDIA_TYPE_VIDEO, AVPixelFormat::AV_PIX_FMT_YUV420P, SwsFlags::SWS_BILINEAR,
};

#[derive(Debug, Clone)]
pub struct VideoEncoderConfig {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub codec: VideoCodec,
    pub encoder_name: Option<String>,
    pub mode: EncodeMode,
    pub bit_rate: i64,
}

impl VideoEncoderConfig {
    pub fn h264_rgba(width: u32, height: u32, fps: u32) -> Self {
        Self {
            width,
            height,
            fps,
            codec: VideoCodec::H264,
            encoder_name: None,
            mode: EncodeMode::CpuUpload,
            bit_rate: 8_000_000,
        }
    }

    pub fn h264_videotoolbox(width: u32, height: u32, fps: u32) -> Self {
        Self {
            encoder_name: Some("h264_videotoolbox".to_string()),
            ..Self::h264_rgba(width, height, fps)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuTextureEncodeSupport {
    pub backend: GpuBackend,
    pub codec: VideoCodec,
    pub encoder_name: Option<&'static str>,
    pub available: bool,
    pub direct_texture_path: bool,
    pub reason: Option<String>,
}

pub fn gpu_texture_encode_support(
    codec: VideoCodec,
    backend: GpuBackend,
) -> GpuTextureEncodeSupport {
    let encoder_name = match (backend, codec) {
        (GpuBackend::Metal, VideoCodec::H264) => Some("h264_videotoolbox"),
        (GpuBackend::Metal, VideoCodec::Hevc) => Some("hevc_videotoolbox"),
        (GpuBackend::Vulkan, VideoCodec::H264) => Some("h264_vulkan"),
        (GpuBackend::Vulkan, VideoCodec::Hevc) => Some("hevc_vulkan"),
        _ => None,
    };

    let encoder_available = encoder_name.is_some_and(|name| encoder_by_name(name).is_ok());
    let reason = if !encoder_available {
        Some(match encoder_name {
            Some(name) => format!("FFmpeg encoder `{name}` is unavailable"),
            None => format!("{backend:?} texture encode is unavailable for {codec:?}"),
        })
    } else if backend == GpuBackend::Metal {
        Some(
            "Metal texture encode needs CVPixelBuffer-backed render targets before it can avoid readback"
                .to_string(),
        )
    } else {
        Some(
            "Vulkan texture encode needs exported AVVkFrame/image interop before it can avoid readback"
                .to_string(),
        )
    };

    GpuTextureEncodeSupport {
        backend,
        codec,
        encoder_name,
        available: encoder_available,
        direct_texture_path: false,
        reason,
    }
}

pub struct OutputContext {
    path: String,
    ptr: *mut sys::AVFormatContext,
    opened_io: bool,
}

unsafe impl Send for OutputContext {}

impl OutputContext {
    pub fn create(path: impl Into<String>) -> Result<Self> {
        ffi::init();
        let path = path.into();
        let c_path = ffi::cstring("avformat_alloc_output_context2", &path)?;
        let mut ptr: *mut sys::AVFormatContext = ptr::null_mut();
        unsafe {
            ffi::check(
                sys::avformat_alloc_output_context2(
                    &mut ptr,
                    ptr::null_mut(),
                    ptr::null(),
                    c_path.as_ptr(),
                ),
                "avformat_alloc_output_context2",
            )
            .map_err(|error| error.with_path(path.clone()))?;
        }
        if ptr.is_null() {
            return Err(FfmpegError::new(
                "avformat_alloc_output_context2",
                "failed to allocate output context",
            )
            .with_path(path));
        }
        Ok(Self {
            path,
            ptr,
            opened_io: false,
        })
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    fn open_io(&mut self) -> Result<()> {
        unsafe {
            if ((*(*self.ptr).oformat).flags & sys::AVFMT_NOFILE) == 0 {
                let c_path = ffi::cstring("avio_open", &self.path)?;
                ffi::check(
                    sys::avio_open(&mut (*self.ptr).pb, c_path.as_ptr(), sys::AVIO_FLAG_WRITE),
                    "avio_open",
                )
                .map_err(|error| error.with_path(self.path.clone()))?;
                self.opened_io = true;
            }
        }
        Ok(())
    }

    fn write_header(&mut self) -> Result<()> {
        self.open_io()?;
        unsafe {
            ffi::check(
                sys::avformat_write_header(self.ptr, ptr::null_mut()),
                "avformat_write_header",
            )
            .map_err(|error| error.with_path(self.path.clone()))
        }
    }

    fn write_trailer(&mut self) -> Result<()> {
        unsafe {
            ffi::check(sys::av_write_trailer(self.ptr), "av_write_trailer")
                .map_err(|error| error.with_path(self.path.clone()))
        }
    }
}

impl Drop for OutputContext {
    fn drop(&mut self) {
        unsafe {
            if self.opened_io && !(*self.ptr).pb.is_null() {
                sys::avio_closep(&mut (*self.ptr).pb);
            }
            sys::avformat_free_context(self.ptr);
        }
    }
}

pub struct VideoEncoder {
    stream_index: usize,
    stream_time_base: sys::AVRational,
    context: *mut sys::AVCodecContext,
    frame: AvFrame,
    scaler: *mut sys::SwsContext,
    next_pts: i64,
    mode: EncodeMode,
    gpu_telemetry: GpuEncodeTelemetry,
}

unsafe impl Send for VideoEncoder {}

impl VideoEncoder {
    pub fn create(output: &mut OutputContext, config: VideoEncoderConfig) -> Result<Self> {
        if config.width == 0 || config.height == 0 || config.fps == 0 {
            return Err(FfmpegError::new(
                "VideoEncoder::create",
                "width, height, and fps must be greater than zero",
            ));
        }
        let codec = find_encoder(&config)?;
        if codec.is_null() {
            return Err(FfmpegError::new(
                "avcodec_find_encoder",
                "requested encoder is unavailable",
            )
            .with_codec(config.codec));
        }
        let stream = unsafe { sys::avformat_new_stream(output.ptr, ptr::null()) };
        if stream.is_null() {
            return Err(FfmpegError::new(
                "avformat_new_stream",
                "failed to allocate output stream",
            ));
        }
        let context = unsafe { sys::avcodec_alloc_context3(codec) };
        if context.is_null() {
            return Err(FfmpegError::new(
                "avcodec_alloc_context3",
                "failed to allocate encoder context",
            ));
        }

        let time_base = sys::AVRational {
            num: 1,
            den: config.fps as i32,
        };
        unsafe {
            (*context).codec_id = config.codec.to_av_codec_id();
            (*context).codec_type = AVMEDIA_TYPE_VIDEO;
            (*context).width = config.width as i32;
            (*context).height = config.height as i32;
            (*context).time_base = time_base;
            (*context).framerate = sys::AVRational {
                num: config.fps as i32,
                den: 1,
            };
            (*context).pix_fmt = AV_PIX_FMT_YUV420P;
            (*context).bit_rate = config.bit_rate;
            (*context).gop_size = config.fps as i32 * 2;
            if ((*(*output.ptr).oformat).flags & sys::AVFMT_GLOBALHEADER) != 0 {
                (*context).flags |= sys::AV_CODEC_FLAG_GLOBAL_HEADER as i32;
            }
            ffi::check(
                sys::avcodec_open2(context, codec, ptr::null_mut()),
                "avcodec_open2",
            )?;
            ffi::check(
                sys::avcodec_parameters_from_context((*stream).codecpar, context),
                "avcodec_parameters_from_context",
            )?;
            (*stream).time_base = time_base;
        }

        let mut frame = AvFrame::new()?;
        unsafe {
            (*frame.as_mut_ptr()).format = AV_PIX_FMT_YUV420P as i32;
            (*frame.as_mut_ptr()).width = config.width as i32;
            (*frame.as_mut_ptr()).height = config.height as i32;
            ffi::check(
                sys::av_frame_get_buffer(frame.as_mut_ptr(), 32),
                "av_frame_get_buffer",
            )?;
        }

        let scaler = unsafe {
            sys::sws_getContext(
                config.width as i32,
                config.height as i32,
                PixelFormat::Rgba8.to_av_pixel_format(),
                config.width as i32,
                config.height as i32,
                AV_PIX_FMT_YUV420P,
                SWS_BILINEAR as i32,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null(),
            )
        };
        if scaler.is_null() {
            return Err(FfmpegError::new(
                "sws_getContext",
                "failed to create encoder color conversion context",
            ));
        }

        Ok(Self {
            stream_index: unsafe { (*stream).index as usize },
            stream_time_base: time_base,
            context,
            frame,
            scaler,
            next_pts: 0,
            mode: config.mode,
            gpu_telemetry: GpuEncodeTelemetry::default(),
        })
    }

    pub fn gpu_telemetry(&self) -> &GpuEncodeTelemetry {
        &self.gpu_telemetry
    }

    fn send_cpu_frame(&mut self, output: &mut OutputContext, frame: &CpuVideoFrame) -> Result<()> {
        if let EncodeMode::GpuTexture(backend) = self.mode {
            return Err(FfmpegError::new(
                "VideoEncoder::send_cpu_frame",
                "hardware texture encoders consume GPU inputs; create a CPU upload encoder to send CPU bytes",
            )
            .with_backend(backend));
        }
        unsafe {
            ffi::check(
                sys::av_frame_make_writable(self.frame.as_mut_ptr()),
                "av_frame_make_writable",
            )?;
        }
        let src_data = [frame.data.as_ptr(), ptr::null(), ptr::null(), ptr::null()];
        let src_stride = [frame.stride as i32, 0, 0, 0];
        unsafe {
            sys::sws_scale(
                self.scaler,
                src_data.as_ptr(),
                src_stride.as_ptr(),
                0,
                frame.height as i32,
                (*self.frame.as_mut_ptr()).data.as_mut_ptr(),
                (*self.frame.as_mut_ptr()).linesize.as_mut_ptr(),
            );
            (*self.frame.as_mut_ptr()).pts = frame.pts.unwrap_or(self.next_pts);
        }
        self.next_pts = self.next_pts.saturating_add(1);
        self.send_frame(output, self.frame.as_ptr())
    }

    fn flush(&mut self, output: &mut OutputContext) -> Result<()> {
        self.send_frame(output, ptr::null())
    }

    fn send_frame(&mut self, output: &mut OutputContext, frame: *const sys::AVFrame) -> Result<()> {
        unsafe {
            ffi::check(
                sys::avcodec_send_frame(self.context, frame),
                "avcodec_send_frame",
            )?;
        }
        loop {
            let mut packet = AvPacket::new()?;
            let result = unsafe { sys::avcodec_receive_packet(self.context, packet.as_mut_ptr()) };
            if result == sys::AVERROR(libc::EAGAIN) || result == sys::AVERROR_EOF {
                break;
            }
            if result < 0 {
                return Err(ffi::error_from_code("avcodec_receive_packet", result));
            }
            unsafe {
                (*packet.as_mut_ptr()).stream_index = self.stream_index as i32;
                sys::av_packet_rescale_ts(
                    packet.as_mut_ptr(),
                    (*self.context).time_base,
                    self.stream_time_base,
                );
                ffi::check(
                    sys::av_interleaved_write_frame(output.ptr, packet.as_mut_ptr()),
                    "av_interleaved_write_frame",
                )
                .map_err(|error| error.with_path(output.path.clone()))?;
            }
        }
        Ok(())
    }
}

fn find_encoder(config: &VideoEncoderConfig) -> Result<*const sys::AVCodec> {
    let encoder_name = match config.mode {
        EncodeMode::GpuTexture(backend) => Some(hardware_encoder_name(config.codec, backend)?),
        EncodeMode::CpuUpload => config.encoder_name.as_deref(),
    };

    if let Some(name) = encoder_name {
        encoder_by_name(name).map_err(|error| error.with_codec(config.codec))
    } else {
        let codec = unsafe { sys::avcodec_find_encoder(config.codec.to_av_codec_id()) };
        if codec.is_null() {
            Err(
                FfmpegError::new("avcodec_find_encoder", "requested encoder is unavailable")
                    .with_codec(config.codec),
            )
        } else {
            Ok(codec)
        }
    }
}

fn encoder_by_name(name: &str) -> Result<*const sys::AVCodec> {
    let c_name = CString::new(name).map_err(|_| {
        FfmpegError::new(
            "avcodec_find_encoder_by_name",
            "encoder name contains NUL byte",
        )
    })?;
    let codec = unsafe { sys::avcodec_find_encoder_by_name(c_name.as_ptr()) };
    if codec.is_null() {
        Err(FfmpegError::new(
            "avcodec_find_encoder_by_name",
            format!("requested encoder `{name}` is unavailable"),
        ))
    } else {
        Ok(codec)
    }
}

fn hardware_encoder_name(codec: VideoCodec, backend: GpuBackend) -> Result<&'static str> {
    match (backend, codec) {
        (GpuBackend::Metal, VideoCodec::H264) => Ok("h264_videotoolbox"),
        (GpuBackend::Metal, VideoCodec::Hevc) => Ok("hevc_videotoolbox"),
        (GpuBackend::Vulkan, _) => Err(FfmpegError::new(
            "VideoEncoder::create",
            format!("{backend:?} hardware texture encode is not available yet"),
        )
        .with_backend(backend)
        .with_codec(codec)),
        _ => Err(FfmpegError::new(
            "VideoEncoder::create",
            format!("{backend:?} hardware encode is unavailable for {codec:?}"),
        )
        .with_backend(backend)
        .with_codec(codec)),
    }
}

impl Drop for VideoEncoder {
    fn drop(&mut self) {
        unsafe {
            if !self.scaler.is_null() {
                sys::sws_freeContext(self.scaler);
            }
            sys::avcodec_free_context(&mut self.context);
        }
    }
}

pub struct MuxedEncoder {
    output: OutputContext,
    video: VideoEncoder,
    wrote_header: bool,
}

impl MuxedEncoder {
    pub fn create(path: impl Into<String>, video: VideoEncoderConfig) -> Result<Self> {
        let mut output = OutputContext::create(path)?;
        let video = VideoEncoder::create(&mut output, video)?;
        output.write_header()?;
        Ok(Self {
            output,
            video,
            wrote_header: true,
        })
    }

    pub fn write_video_frame(&mut self, frame: &CpuVideoFrame) -> Result<()> {
        self.video.send_cpu_frame(&mut self.output, frame)
    }

    pub fn write_gpu_frame(&mut self, frame: &GpuVideoInput<'_>) -> Result<()> {
        self.video.send_gpu_frame(&mut self.output, frame)
    }

    pub fn gpu_telemetry(&self) -> &GpuEncodeTelemetry {
        self.video.gpu_telemetry()
    }

    pub fn finish(mut self) -> Result<()> {
        self.video.flush(&mut self.output)?;
        self.wrote_header = false;
        self.output.write_trailer()
    }
}

impl VideoEncoder {
    fn send_gpu_frame(
        &mut self,
        _output: &mut OutputContext,
        frame: &GpuVideoInput<'_>,
    ) -> Result<()> {
        if self.mode == EncodeMode::CpuUpload {
            return Err(FfmpegError::new(
                "VideoEncoder::send_gpu_frame",
                "CPU upload encoders consume CPU frames; create a hardware texture encoder to send GPU inputs",
            )
            .with_backend(frame.backend()));
        }
        let upload = GpuUploadDescriptor::from_frame(frame);
        self.gpu_telemetry.record_upload_started(&upload);
        let started = Instant::now();
        let error = FfmpegError::new(
            "VideoEncoder::send_gpu_frame",
            "GPU texture encode is reserved for the Metal/Vulkan backend implementation",
        )
        .with_backend(frame.backend());
        self.gpu_telemetry
            .record_upload_failed(&upload, started.elapsed(), error.message.clone());
        Err(error)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GpuEncodeTelemetry {
    pub upload_attempts: u64,
    pub upload_successes: u64,
    pub upload_failures: u64,
    pub encode_attempts: u64,
    pub encode_successes: u64,
    pub encode_failures: u64,
    pub metal_frames: u64,
    pub vulkan_frames: u64,
    pub estimated_upload_bytes: u64,
    pub upload_time_us: u128,
    pub encode_time_us: u128,
    pub last_error: Option<String>,
    pub recent_events: Vec<GpuEncodeEvent>,
}

impl GpuEncodeTelemetry {
    const MAX_RECENT_EVENTS: usize = 128;

    pub fn record_upload_started(&mut self, descriptor: &GpuUploadDescriptor) {
        self.upload_attempts = self.upload_attempts.saturating_add(1);
        self.estimated_upload_bytes = self
            .estimated_upload_bytes
            .saturating_add(descriptor.estimated_bytes);
        match descriptor.backend {
            GpuBackend::Metal => self.metal_frames = self.metal_frames.saturating_add(1),
            GpuBackend::Vulkan => self.vulkan_frames = self.vulkan_frames.saturating_add(1),
        }
        self.push_event(GpuEncodeEvent::started(GpuEncodeStage::Upload, descriptor));
    }

    pub fn record_upload_finished(&mut self, descriptor: &GpuUploadDescriptor, elapsed: Duration) {
        self.upload_successes = self.upload_successes.saturating_add(1);
        self.upload_time_us = self.upload_time_us.saturating_add(elapsed.as_micros());
        self.push_event(GpuEncodeEvent::finished(
            GpuEncodeStage::Upload,
            descriptor,
            elapsed,
        ));
    }

    pub fn record_upload_failed(
        &mut self,
        descriptor: &GpuUploadDescriptor,
        elapsed: Duration,
        message: impl Into<String>,
    ) {
        let message = message.into();
        self.upload_failures = self.upload_failures.saturating_add(1);
        self.upload_time_us = self.upload_time_us.saturating_add(elapsed.as_micros());
        self.last_error = Some(message.clone());
        self.push_event(GpuEncodeEvent::failed(
            GpuEncodeStage::Upload,
            descriptor,
            elapsed,
            message,
        ));
    }

    pub fn record_encode_started(&mut self, descriptor: &GpuUploadDescriptor) {
        self.encode_attempts = self.encode_attempts.saturating_add(1);
        self.push_event(GpuEncodeEvent::started(GpuEncodeStage::Encode, descriptor));
    }

    pub fn record_encode_finished(&mut self, descriptor: &GpuUploadDescriptor, elapsed: Duration) {
        self.encode_successes = self.encode_successes.saturating_add(1);
        self.encode_time_us = self.encode_time_us.saturating_add(elapsed.as_micros());
        self.push_event(GpuEncodeEvent::finished(
            GpuEncodeStage::Encode,
            descriptor,
            elapsed,
        ));
    }

    pub fn record_encode_failed(
        &mut self,
        descriptor: &GpuUploadDescriptor,
        elapsed: Duration,
        message: impl Into<String>,
    ) {
        let message = message.into();
        self.encode_failures = self.encode_failures.saturating_add(1);
        self.encode_time_us = self.encode_time_us.saturating_add(elapsed.as_micros());
        self.last_error = Some(message.clone());
        self.push_event(GpuEncodeEvent::failed(
            GpuEncodeStage::Encode,
            descriptor,
            elapsed,
            message,
        ));
    }

    fn push_event(&mut self, event: GpuEncodeEvent) {
        if self.recent_events.len() == Self::MAX_RECENT_EVENTS {
            self.recent_events.remove(0);
        }
        self.recent_events.push(event);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuUploadDescriptor {
    pub backend: GpuBackend,
    pub width: u32,
    pub height: u32,
    pub estimated_bytes: u64,
}

impl GpuUploadDescriptor {
    pub fn from_frame(frame: &GpuVideoInput<'_>) -> Self {
        let (width, height) = frame.dimensions();
        Self {
            backend: frame.backend(),
            width,
            height,
            estimated_bytes: frame.estimated_rgba_bytes(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuEncodeStage {
    Upload,
    Encode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuEncodeEvent {
    pub stage: GpuEncodeStage,
    pub outcome: GpuEncodeOutcome,
    pub backend: GpuBackend,
    pub width: u32,
    pub height: u32,
    pub estimated_bytes: u64,
    pub elapsed_us: Option<u128>,
    pub message: Option<String>,
}

impl GpuEncodeEvent {
    fn started(stage: GpuEncodeStage, descriptor: &GpuUploadDescriptor) -> Self {
        Self::new(stage, GpuEncodeOutcome::Started, descriptor, None, None)
    }

    fn finished(
        stage: GpuEncodeStage,
        descriptor: &GpuUploadDescriptor,
        elapsed: Duration,
    ) -> Self {
        Self::new(
            stage,
            GpuEncodeOutcome::Finished,
            descriptor,
            Some(elapsed),
            None,
        )
    }

    fn failed(
        stage: GpuEncodeStage,
        descriptor: &GpuUploadDescriptor,
        elapsed: Duration,
        message: String,
    ) -> Self {
        Self::new(
            stage,
            GpuEncodeOutcome::Failed,
            descriptor,
            Some(elapsed),
            Some(message),
        )
    }

    fn new(
        stage: GpuEncodeStage,
        outcome: GpuEncodeOutcome,
        descriptor: &GpuUploadDescriptor,
        elapsed: Option<Duration>,
        message: Option<String>,
    ) -> Self {
        Self {
            stage,
            outcome,
            backend: descriptor.backend,
            width: descriptor.width,
            height: descriptor.height,
            estimated_bytes: descriptor.estimated_bytes,
            elapsed_us: elapsed.map(|duration| duration.as_micros()),
            message,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuEncodeOutcome {
    Started,
    Finished,
    Failed,
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "vulkan")]
    #[test]
    fn telemetry_tracks_upload_and_encode_events() {
        use std::time::Duration;

        use ash::vk::Handle;

        use crate::gpu::GpuVideoInput;

        use super::*;

        let frame = GpuVideoInput::Vulkan {
            image: ash::vk::Image::from_raw(1),
            image_view: ash::vk::ImageView::from_raw(2),
            memory: ash::vk::DeviceMemory::from_raw(3),
            format: ash::vk::Format::R8G8B8A8_UNORM,
            extent: ash::vk::Extent3D {
                width: 1280,
                height: 720,
                depth: 1,
            },
        };
        let descriptor = GpuUploadDescriptor::from_frame(&frame);
        let mut telemetry = GpuEncodeTelemetry::default();

        telemetry.record_upload_started(&descriptor);
        telemetry.record_upload_finished(&descriptor, Duration::from_micros(12));
        telemetry.record_encode_started(&descriptor);
        telemetry.record_encode_failed(&descriptor, Duration::from_micros(34), "not yet");

        assert_eq!(telemetry.upload_attempts, 1);
        assert_eq!(telemetry.upload_successes, 1);
        assert_eq!(telemetry.encode_attempts, 1);
        assert_eq!(telemetry.encode_failures, 1);
        assert_eq!(telemetry.vulkan_frames, 1);
        assert_eq!(telemetry.last_error.as_deref(), Some("not yet"));
        assert_eq!(telemetry.recent_events.len(), 4);
    }

    #[cfg(feature = "vulkan")]
    #[test]
    fn cpu_upload_encoder_rejects_gpu_input() {
        use std::{
            fs,
            path::PathBuf,
            time::{SystemTime, UNIX_EPOCH},
        };

        use ash::vk::Handle;

        use crate::gpu::GpuVideoInput;

        use super::*;

        let path = temp_path("gpu_failure", "mp4");
        let Ok(mut encoder) = MuxedEncoder::create(
            path.to_string_lossy().to_string(),
            VideoEncoderConfig::h264_rgba(16, 16, 30),
        ) else {
            eprintln!("H.264 encoder unavailable; skipping GPU input mismatch test");
            return;
        };
        let frame = GpuVideoInput::Vulkan {
            image: ash::vk::Image::from_raw(1),
            image_view: ash::vk::ImageView::from_raw(2),
            memory: ash::vk::DeviceMemory::from_raw(3),
            format: ash::vk::Format::R8G8B8A8_UNORM,
            extent: ash::vk::Extent3D {
                width: 16,
                height: 16,
                depth: 1,
            },
        };

        let error = encoder
            .write_gpu_frame(&frame)
            .expect_err("CPU encoder should reject GPU input");
        assert_eq!(error.backend, Some(GpuBackend::Vulkan));
        assert_eq!(encoder.gpu_telemetry().upload_attempts, 0);
        assert_eq!(encoder.gpu_telemetry().upload_failures, 0);

        let _ = fs::remove_file(path);

        fn temp_path(name: &str, extension: &str) -> PathBuf {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            std::env::temp_dir().join(format!("lumen_ffmpeg_{name}_{unique}.{extension}"))
        }
    }

    #[cfg(feature = "vulkan")]
    #[test]
    fn unavailable_hardware_texture_encoder_errors_at_create() {
        use std::{
            fs,
            path::PathBuf,
            time::{SystemTime, UNIX_EPOCH},
        };

        use super::*;

        let path = temp_path("gpu_create_failure", "mp4");
        let mut config = VideoEncoderConfig::h264_rgba(16, 16, 30);
        config.mode = EncodeMode::GpuTexture(GpuBackend::Vulkan);

        let error = match MuxedEncoder::create(path.to_string_lossy().to_string(), config) {
            Ok(_) => panic!("Vulkan texture encode should fail at create"),
            Err(error) => error,
        };
        assert_eq!(error.operation, "VideoEncoder::create");
        assert_eq!(error.backend, Some(GpuBackend::Vulkan));

        let _ = fs::remove_file(path);

        fn temp_path(name: &str, extension: &str) -> PathBuf {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            std::env::temp_dir().join(format!("lumen_ffmpeg_{name}_{unique}.{extension}"))
        }
    }
}
