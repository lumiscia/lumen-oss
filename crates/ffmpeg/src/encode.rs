use std::{
    ffi::CString,
    ptr,
    time::{Duration, Instant},
};

#[cfg(feature = "metal")]
use std::ptr::NonNull;

use crate::{
    FfmpegError, Result,
    audio::{AudioFrame, SampleFormat},
    ffi::{self, AvFrame, AvPacket, sys},
    gpu::{GpuBackend, GpuVideoInput},
    video::{CpuVideoFrame, EncodeMode, PixelFormat, VideoCodec},
};
#[cfg(target_os = "linux")]
use sys::SWS_BILINEAR;
#[cfg(not(target_os = "linux"))]
use sys::SwsFlags::SWS_BILINEAR;
use sys::{
    AVHWDeviceType::AV_HWDEVICE_TYPE_CUDA,
    AVMediaType::{AVMEDIA_TYPE_AUDIO, AVMEDIA_TYPE_VIDEO},
    AVPixelFormat::{
        AV_PIX_FMT_CUDA, AV_PIX_FMT_RGBA, AV_PIX_FMT_VIDEOTOOLBOX, AV_PIX_FMT_YUV420P,
    },
    AVSampleFormat::{AV_SAMPLE_FMT_FLT, AV_SAMPLE_FMT_FLTP, AV_SAMPLE_FMT_NONE},
};

#[cfg(feature = "metal")]
use objc2_core_foundation::CFRetained;
#[cfg(feature = "metal")]
use objc2_core_video::CVPixelBuffer;

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

#[derive(Debug, Clone)]
pub struct AudioEncoderConfig {
    pub sample_rate: u32,
    pub channels: u16,
    pub bit_rate: i64,
    pub encoder_name: Option<String>,
}

impl AudioEncoderConfig {
    pub fn aac(sample_rate: u32, channels: u16) -> Self {
        Self {
            sample_rate,
            channels,
            bit_rate: 192_000,
            encoder_name: None,
        }
    }
}

impl VideoEncoderConfig {
    pub fn cpu_rgba(width: u32, height: u32, fps: u32, codec: VideoCodec) -> Self {
        Self {
            width,
            height,
            fps,
            codec,
            encoder_name: None,
            mode: EncodeMode::CpuUpload,
            bit_rate: 8_000_000,
        }
    }

    pub fn h264_videotoolbox(width: u32, height: u32, fps: u32) -> Self {
        Self {
            encoder_name: Some("h264_videotoolbox".to_string()),
            ..Self::cpu_rgba(width, height, fps, VideoCodec::H264)
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
        (GpuBackend::Cuda, VideoCodec::H264) => Some("h264_nvenc"),
        (GpuBackend::Cuda, VideoCodec::Hevc) => Some("hevc_nvenc"),
        (GpuBackend::Cuda, VideoCodec::Av1) => Some("av1_nvenc"),
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
    } else {
        Some(
            match backend {
                GpuBackend::Cuda => {
                    "CUDA frame encode is available; Vulkan texture encode uses external memory import plus a GPU-side copy into linear CUDA frame memory"
                }
                GpuBackend::Metal => {
            "Metal texture encode needs CVPixelBuffer-backed render targets before it can avoid readback"
            }
                GpuBackend::Vulkan => {
            "Vulkan texture encode needs exported AVVkFrame/image interop before it can avoid readback"
                }
            }
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
    _hw_device: Option<AvBufferRef>,
    _hw_frames: Option<AvBufferRef>,
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
        let hw_contexts = create_encoder_hw_contexts(&config)?;

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
            (*context).pix_fmt = match config.mode {
                EncodeMode::GpuTexture(GpuBackend::Cuda) => sys::AVPixelFormat::AV_PIX_FMT_CUDA,
                EncodeMode::GpuTexture(GpuBackend::Metal) => AV_PIX_FMT_VIDEOTOOLBOX,
                EncodeMode::GpuTexture(GpuBackend::Vulkan) | EncodeMode::CpuUpload => {
                    AV_PIX_FMT_YUV420P
                }
            };
            (*context).bit_rate = config.bit_rate;
            (*context).gop_size = config.fps as i32 * 2;
            if let Some(frames) = hw_contexts
                .as_ref()
                .and_then(|contexts| contexts.frames.as_ref())
            {
                (*context).hw_frames_ctx = sys::av_buffer_ref(frames.ptr);
                if (*context).hw_frames_ctx.is_null() {
                    return Err(FfmpegError::new(
                        "av_buffer_ref",
                        "failed to reference encoder hardware frames context",
                    )
                    .with_backend(GpuBackend::Cuda));
                }
            }
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
        let scaler = if config.mode == EncodeMode::CpuUpload {
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
            scaler
        } else {
            ptr::null_mut()
        };

        Ok(Self {
            stream_index: unsafe { (*stream).index as usize },
            stream_time_base: time_base,
            context,
            frame,
            scaler,
            _hw_device: hw_contexts
                .as_ref()
                .and_then(|contexts| contexts.device.clone_ref()),
            _hw_frames: hw_contexts.and_then(|contexts| contexts.frames),
            next_pts: 0,
            mode: config.mode,
            gpu_telemetry: GpuEncodeTelemetry::default(),
        })
    }

    pub fn gpu_telemetry(&self) -> &GpuEncodeTelemetry {
        &self.gpu_telemetry
    }

    fn refresh_stream_time_base(&mut self, output: &OutputContext) -> Result<()> {
        unsafe {
            let stream_count = (*output.ptr).nb_streams as usize;
            if self.stream_index >= stream_count {
                return Err(FfmpegError::new(
                    "VideoEncoder::refresh_stream_time_base",
                    format!(
                        "stream index {} is outside output stream count {stream_count}",
                        self.stream_index
                    ),
                )
                .with_path(output.path.clone()));
            }
            let stream = *(*output.ptr).streams.add(self.stream_index);
            if stream.is_null() {
                return Err(FfmpegError::new(
                    "VideoEncoder::refresh_stream_time_base",
                    "output stream is null",
                )
                .with_path(output.path.clone()));
            }
            self.stream_time_base = (*stream).time_base;
        }
        Ok(())
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
            (*self.frame.as_mut_ptr()).duration = 1;
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
                if (*packet.as_mut_ptr()).duration == 0 {
                    (*packet.as_mut_ptr()).duration = 1;
                }
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
        (GpuBackend::Cuda, VideoCodec::H264) => Ok("h264_nvenc"),
        (GpuBackend::Cuda, VideoCodec::Hevc) => Ok("hevc_nvenc"),
        (GpuBackend::Cuda, VideoCodec::Av1) => Ok("av1_nvenc"),
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

pub struct AudioEncoder {
    stream_index: usize,
    stream_time_base: sys::AVRational,
    context: *mut sys::AVCodecContext,
    sample_format: sys::AVSampleFormat,
    sample_rate: u32,
    channels: u16,
    frame_size: usize,
    pending: Vec<f32>,
    next_pts: i64,
}

unsafe impl Send for AudioEncoder {}

impl AudioEncoder {
    pub fn create(output: &mut OutputContext, config: AudioEncoderConfig) -> Result<Self> {
        if config.sample_rate == 0 || config.channels == 0 {
            return Err(FfmpegError::new(
                "AudioEncoder::create",
                "sample_rate and channels must be greater than zero",
            ));
        }
        let codec = find_audio_encoder(&config)?;
        let sample_format = select_audio_sample_format(codec)?;
        let stream = unsafe { sys::avformat_new_stream(output.ptr, ptr::null()) };
        if stream.is_null() {
            return Err(FfmpegError::new(
                "avformat_new_stream",
                "failed to allocate audio output stream",
            ));
        }
        let context = unsafe { sys::avcodec_alloc_context3(codec) };
        if context.is_null() {
            return Err(FfmpegError::new(
                "avcodec_alloc_context3",
                "failed to allocate audio encoder context",
            ));
        }
        let time_base = sys::AVRational {
            num: 1,
            den: config.sample_rate as i32,
        };
        unsafe {
            (*context).codec_id = sys::AVCodecID::AV_CODEC_ID_AAC;
            (*context).codec_type = AVMEDIA_TYPE_AUDIO;
            (*context).sample_rate = config.sample_rate as i32;
            (*context).sample_fmt = sample_format;
            (*context).bit_rate = config.bit_rate;
            (*context).time_base = time_base;
            sys::av_channel_layout_default(&mut (*context).ch_layout, config.channels as i32);
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
        let frame_size = unsafe { (*context).frame_size.max(0) as usize };
        let frame_size = frame_size.max(1);
        Ok(Self {
            stream_index: unsafe { (*stream).index as usize },
            stream_time_base: time_base,
            context,
            sample_format,
            sample_rate: config.sample_rate,
            channels: config.channels,
            frame_size,
            pending: Vec::with_capacity(frame_size.saturating_mul(config.channels as usize)),
            next_pts: 0,
        })
    }

    fn refresh_stream_time_base(&mut self, output: &OutputContext) -> Result<()> {
        unsafe {
            let stream_count = (*output.ptr).nb_streams as usize;
            if self.stream_index >= stream_count {
                return Err(FfmpegError::new(
                    "AudioEncoder::refresh_stream_time_base",
                    format!(
                        "stream index {} is outside output stream count {stream_count}",
                        self.stream_index
                    ),
                )
                .with_path(output.path.clone()));
            }
            let stream = *(*output.ptr).streams.add(self.stream_index);
            if stream.is_null() {
                return Err(FfmpegError::new(
                    "AudioEncoder::refresh_stream_time_base",
                    "output stream is null",
                )
                .with_path(output.path.clone()));
            }
            self.stream_time_base = (*stream).time_base;
        }
        Ok(())
    }

    fn send_audio_frame(&mut self, output: &mut OutputContext, frame: &AudioFrame) -> Result<()> {
        self.validate_audio_frame(frame)?;
        self.pending.extend_from_slice(&frame.interleaved_f32);
        let samples_per_packet = self.frame_size.saturating_mul(self.channels as usize);
        while self.pending.len() >= samples_per_packet {
            let chunk: Vec<f32> = self.pending.drain(..samples_per_packet).collect();
            self.send_samples(output, &chunk, self.frame_size)?;
        }
        Ok(())
    }

    fn validate_audio_frame(&self, frame: &AudioFrame) -> Result<()> {
        if frame.sample_rate != self.sample_rate
            || frame.channels != self.channels
            || frame.sample_format != SampleFormat::F32
        {
            return Err(FfmpegError::new(
                "AudioEncoder::send_audio_frame",
                format!(
                    "expected {} Hz, {} channel interleaved f32 audio; got {} Hz, {} channel {:?}",
                    self.sample_rate,
                    self.channels,
                    frame.sample_rate,
                    frame.channels,
                    frame.sample_format
                ),
            ));
        }
        let channels = self.channels as usize;
        if frame.interleaved_f32.len() != frame.samples.saturating_mul(channels) {
            return Err(FfmpegError::new(
                "AudioEncoder::send_audio_frame",
                "audio sample count does not match interleaved buffer length",
            ));
        }
        Ok(())
    }

    fn flush(&mut self, output: &mut OutputContext) -> Result<()> {
        if !self.pending.is_empty() {
            let channels = self.channels as usize;
            let samples = self.pending.len().div_ceil(channels);
            let mut chunk = std::mem::take(&mut self.pending);
            chunk.resize(samples.saturating_mul(channels), 0.0);
            self.send_samples(output, &chunk, samples)?;
        }
        self.send_frame(output, ptr::null())
    }

    fn send_samples(
        &mut self,
        output: &mut OutputContext,
        samples: &[f32],
        sample_count: usize,
    ) -> Result<()> {
        let mut frame = AvFrame::new()?;
        unsafe {
            (*frame.as_mut_ptr()).format = self.sample_format as i32;
            (*frame.as_mut_ptr()).nb_samples = sample_count as i32;
            (*frame.as_mut_ptr()).sample_rate = self.sample_rate as i32;
            sys::av_channel_layout_default(
                &mut (*frame.as_mut_ptr()).ch_layout,
                self.channels as i32,
            );
            ffi::check(
                sys::av_frame_get_buffer(frame.as_mut_ptr(), 0),
                "av_frame_get_buffer",
            )?;
            ffi::check(
                sys::av_frame_make_writable(frame.as_mut_ptr()),
                "av_frame_make_writable",
            )?;
            fill_audio_frame(frame.as_mut_ptr(), samples, sample_count, self.channels)?;
            (*frame.as_mut_ptr()).pts = self.next_pts;
            (*frame.as_mut_ptr()).duration = sample_count as i64;
        }
        self.next_pts = self.next_pts.saturating_add(sample_count as i64);
        self.send_frame(output, frame.as_ptr())
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

impl Drop for AudioEncoder {
    fn drop(&mut self) {
        unsafe {
            sys::avcodec_free_context(&mut self.context);
        }
    }
}

fn find_audio_encoder(config: &AudioEncoderConfig) -> Result<*const sys::AVCodec> {
    if let Some(name) = config.encoder_name.as_deref() {
        return encoder_by_name(name);
    }
    let codec = unsafe { sys::avcodec_find_encoder(sys::AVCodecID::AV_CODEC_ID_AAC) };
    if codec.is_null() {
        Err(FfmpegError::new(
            "avcodec_find_encoder",
            "requested AAC encoder is unavailable",
        ))
    } else {
        Ok(codec)
    }
}

fn select_audio_sample_format(codec: *const sys::AVCodec) -> Result<sys::AVSampleFormat> {
    unsafe {
        let formats = (*codec).sample_fmts;
        if formats.is_null() {
            return Ok(AV_SAMPLE_FMT_FLTP);
        }
        let mut index = 0;
        let mut fallback = AV_SAMPLE_FMT_NONE;
        loop {
            let format = *formats.add(index);
            if format == AV_SAMPLE_FMT_NONE {
                break;
            }
            if format == AV_SAMPLE_FMT_FLTP {
                return Ok(format);
            }
            if format == AV_SAMPLE_FMT_FLT {
                fallback = format;
            }
            index += 1;
        }
        if fallback != AV_SAMPLE_FMT_NONE {
            Ok(fallback)
        } else {
            Err(FfmpegError::new(
                "AudioEncoder::create",
                "AAC encoder does not support f32 input",
            ))
        }
    }
}

unsafe fn fill_audio_frame(
    frame: *mut sys::AVFrame,
    samples: &[f32],
    sample_count: usize,
    channels: u16,
) -> Result<()> {
    let channels = channels as usize;
    let format = unsafe { std::mem::transmute::<i32, sys::AVSampleFormat>((*frame).format) };
    match format {
        AV_SAMPLE_FMT_FLTP => {
            for channel in 0..channels {
                let plane = unsafe { (*frame).data[channel] }.cast::<f32>();
                if plane.is_null() {
                    return Err(FfmpegError::new(
                        "AudioEncoder::fill_audio_frame",
                        "audio frame plane is null",
                    ));
                }
                for sample in 0..sample_count {
                    unsafe {
                        *plane.add(sample) = samples[sample.saturating_mul(channels) + channel];
                    }
                }
            }
            Ok(())
        }
        AV_SAMPLE_FMT_FLT => {
            let data = unsafe { (*frame).data[0] }.cast::<f32>();
            if data.is_null() {
                return Err(FfmpegError::new(
                    "AudioEncoder::fill_audio_frame",
                    "audio frame data is null",
                ));
            }
            unsafe {
                std::ptr::copy_nonoverlapping(samples.as_ptr(), data, samples.len());
            }
            Ok(())
        }
        _ => Err(FfmpegError::new(
            "AudioEncoder::fill_audio_frame",
            "unsupported audio encoder sample format",
        )),
    }
}

pub struct MuxedEncoder {
    output: OutputContext,
    video: VideoEncoder,
    audio: Option<AudioEncoder>,
    wrote_header: bool,
}

impl MuxedEncoder {
    pub fn create(path: impl Into<String>, video: VideoEncoderConfig) -> Result<Self> {
        Self::create_with_audio(path, video, None)
    }

    pub fn create_with_audio(
        path: impl Into<String>,
        video: VideoEncoderConfig,
        audio: Option<AudioEncoderConfig>,
    ) -> Result<Self> {
        let mut output = OutputContext::create(path)?;
        let mut video = VideoEncoder::create(&mut output, video)?;
        let mut audio = audio
            .map(|config| AudioEncoder::create(&mut output, config))
            .transpose()?;
        output.write_header()?;
        video.refresh_stream_time_base(&output)?;
        if let Some(audio) = audio.as_mut() {
            audio.refresh_stream_time_base(&output)?;
        }
        Ok(Self {
            output,
            video,
            audio,
            wrote_header: true,
        })
    }

    pub fn write_video_frame(&mut self, frame: &CpuVideoFrame) -> Result<()> {
        self.video.send_cpu_frame(&mut self.output, frame)
    }

    pub fn write_gpu_frame(&mut self, frame: &GpuVideoInput<'_>) -> Result<()> {
        self.video.send_gpu_frame(&mut self.output, frame)
    }

    pub fn write_audio_frame(&mut self, frame: &AudioFrame) -> Result<()> {
        let Some(audio) = self.audio.as_mut() else {
            return Err(FfmpegError::new(
                "MuxedEncoder::write_audio_frame",
                "encoder was created without an audio stream",
            ));
        };
        audio.send_audio_frame(&mut self.output, frame)
    }

    pub fn gpu_telemetry(&self) -> &GpuEncodeTelemetry {
        self.video.gpu_telemetry()
    }

    pub fn finish(mut self) -> Result<()> {
        self.video.flush(&mut self.output)?;
        if let Some(audio) = self.audio.as_mut() {
            audio.flush(&mut self.output)?;
        }
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
        match frame {
            #[cfg(feature = "metal")]
            GpuVideoInput::MetalPixelBuffer(frame) => {
                self.gpu_telemetry
                    .record_upload_finished(&upload, started.elapsed());
                let encode_started = Instant::now();
                self.gpu_telemetry.record_encode_started(&upload);
                let result = self.send_metal_pixel_buffer_frame(_output, frame);
                match result {
                    Ok(()) => {
                        self.gpu_telemetry
                            .record_encode_finished(&upload, encode_started.elapsed());
                        Ok(())
                    }
                    Err(error) => {
                        self.gpu_telemetry.record_encode_failed(
                            &upload,
                            encode_started.elapsed(),
                            error.message.clone(),
                        );
                        Err(error)
                    }
                }
            }
            #[cfg(feature = "cuda")]
            GpuVideoInput::Cuda(frame) => {
                self.gpu_telemetry
                    .record_upload_finished(&upload, started.elapsed());
                let encode_started = Instant::now();
                self.gpu_telemetry.record_encode_started(&upload);
                let result = self.send_cuda_frame(_output, frame);
                match result {
                    Ok(()) => {
                        self.gpu_telemetry
                            .record_encode_finished(&upload, encode_started.elapsed());
                        Ok(())
                    }
                    Err(error) => {
                        self.gpu_telemetry.record_encode_failed(
                            &upload,
                            encode_started.elapsed(),
                            error.message.clone(),
                        );
                        Err(error)
                    }
                }
            }
            _ => {
                let error = FfmpegError::new(
                    "VideoEncoder::send_gpu_frame",
                    "GPU texture encode currently supports CVPixelBuffer-backed Metal frames only",
                )
                .with_backend(frame.backend());
                self.gpu_telemetry.record_upload_failed(
                    &upload,
                    started.elapsed(),
                    error.message.clone(),
                );
                Err(error)
            }
        }
    }

    #[cfg(feature = "cuda")]
    fn send_cuda_frame(
        &mut self,
        output: &mut OutputContext,
        frame: &crate::gpu::CudaVideoFrame,
    ) -> Result<()> {
        if self.mode != EncodeMode::GpuTexture(GpuBackend::Cuda) {
            return Err(FfmpegError::new(
                "VideoEncoder::send_cuda_frame",
                "CUDA frames require a CUDA/NVENC hardware texture encoder",
            )
            .with_backend(GpuBackend::Cuda));
        }
        let Some(hw_frames) = self._hw_frames.as_ref() else {
            return Err(FfmpegError::new(
                "VideoEncoder::send_cuda_frame",
                "CUDA/NVENC encoder was created without a CUDA hardware frames context",
            )
            .with_backend(GpuBackend::Cuda));
        };
        let (width, height) = frame.dimensions();
        unsafe {
            if (*self.context).width != width as i32 || (*self.context).height != height as i32 {
                return Err(FfmpegError::new(
                    "VideoEncoder::send_cuda_frame",
                    format!(
                        "CUDA frame dimensions {width}x{height} do not match encoder dimensions {}x{}",
                        (*self.context).width,
                        (*self.context).height,
                    ),
                )
                .with_backend(GpuBackend::Cuda));
            }
        }

        let mut av_frame = AvFrame::new()?;
        unsafe {
            (*av_frame.as_mut_ptr()).format = AV_PIX_FMT_CUDA as i32;
            (*av_frame.as_mut_ptr()).width = width as i32;
            (*av_frame.as_mut_ptr()).height = height as i32;
            (*av_frame.as_mut_ptr()).pts = frame.pts().unwrap_or(self.next_pts);
            (*av_frame.as_mut_ptr()).duration = 1;
            (*av_frame.as_mut_ptr()).hw_frames_ctx = sys::av_buffer_ref(hw_frames.ptr);
            if (*av_frame.as_mut_ptr()).hw_frames_ctx.is_null() {
                return Err(FfmpegError::new(
                    "av_buffer_ref",
                    "failed to reference CUDA hardware frames context for frame",
                )
                .with_backend(GpuBackend::Cuda));
            }

            let data_ptr = frame.device_ptr() as usize as *mut u8;
            (*av_frame.as_mut_ptr()).data[0] = data_ptr;
            (*av_frame.as_mut_ptr()).linesize[0] = frame.pitch() as i32;
            (*av_frame.as_mut_ptr()).buf[0] = sys::av_buffer_create(
                data_ptr,
                1,
                Some(release_external_cuda_frame),
                ptr::null_mut(),
                0,
            );
            if (*av_frame.as_mut_ptr()).buf[0].is_null() {
                return Err(FfmpegError::new(
                    "av_buffer_create",
                    "failed to create external CUDA frame lifetime reference",
                )
                .with_backend(GpuBackend::Cuda));
            }
        }
        self.next_pts = self.next_pts.saturating_add(1);
        self.send_frame(output, av_frame.as_ptr())
    }

    #[cfg(feature = "metal")]
    fn send_metal_pixel_buffer_frame(
        &mut self,
        output: &mut OutputContext,
        frame: &crate::gpu::MetalPixelBufferFrame,
    ) -> Result<()> {
        if self.mode != EncodeMode::GpuTexture(GpuBackend::Metal) {
            return Err(FfmpegError::new(
                "VideoEncoder::send_metal_pixel_buffer_frame",
                "Metal pixel buffers require a Metal hardware texture encoder",
            )
            .with_backend(GpuBackend::Metal));
        }
        let mut av_frame = AvFrame::new()?;
        unsafe {
            (*av_frame.as_mut_ptr()).format = AV_PIX_FMT_VIDEOTOOLBOX as i32;
            (*av_frame.as_mut_ptr()).width = frame.dimensions().0 as i32;
            (*av_frame.as_mut_ptr()).height = frame.dimensions().1 as i32;
            (*av_frame.as_mut_ptr()).pts = frame.pts().unwrap_or(self.next_pts);
            (*av_frame.as_mut_ptr()).duration = 1;

            let pixel_buffer = frame.pixel_buffer_ptr();
            let retained: CFRetained<CVPixelBuffer> = CFRetained::retain(pixel_buffer);
            let retained_ptr: NonNull<CVPixelBuffer> = CFRetained::into_raw(retained);
            let buffer_ref = sys::av_buffer_create(
                retained_ptr.as_ptr().cast::<u8>(),
                1,
                Some(release_cv_pixel_buffer),
                retained_ptr.as_ptr().cast(),
                0,
            );
            if buffer_ref.is_null() {
                let _ = CFRetained::<CVPixelBuffer>::from_raw(retained_ptr);
                return Err(FfmpegError::new(
                    "av_buffer_create",
                    "failed to create CVPixelBuffer lifetime reference",
                )
                .with_backend(GpuBackend::Metal));
            }
            (*av_frame.as_mut_ptr()).data[3] = pixel_buffer.as_ptr().cast::<u8>();
            (*av_frame.as_mut_ptr()).buf[0] = buffer_ref;
        }
        self.next_pts = self.next_pts.saturating_add(1);
        self.send_frame(output, av_frame.as_ptr())
    }
}

#[cfg(feature = "cuda")]
unsafe extern "C" fn release_external_cuda_frame(_opaque: *mut libc::c_void, _data: *mut u8) {}

#[cfg(feature = "metal")]
unsafe extern "C" fn release_cv_pixel_buffer(opaque: *mut libc::c_void, _data: *mut u8) {
    if let Some(pixel_buffer) = NonNull::new(opaque.cast::<CVPixelBuffer>()) {
        let _ = unsafe { CFRetained::<CVPixelBuffer>::from_raw(pixel_buffer) };
    }
}

struct EncoderHwContexts {
    device: AvBufferRef,
    frames: Option<AvBufferRef>,
}

fn create_encoder_hw_contexts(config: &VideoEncoderConfig) -> Result<Option<EncoderHwContexts>> {
    match config.mode {
        EncodeMode::GpuTexture(GpuBackend::Cuda) => {
            let device = AvBufferRef::new_hw_device(AV_HWDEVICE_TYPE_CUDA, GpuBackend::Cuda)?;
            let frames = AvBufferRef::new_hw_frames(
                &device,
                GpuBackend::Cuda,
                AV_PIX_FMT_CUDA,
                AV_PIX_FMT_RGBA,
                config.width,
                config.height,
            )?;
            Ok(Some(EncoderHwContexts {
                device,
                frames: Some(frames),
            }))
        }
        _ => Ok(None),
    }
}

struct AvBufferRef {
    ptr: *mut sys::AVBufferRef,
}

impl AvBufferRef {
    fn new_hw_device(device_type: sys::AVHWDeviceType, backend: GpuBackend) -> Result<Self> {
        let mut ptr = ptr::null_mut();
        unsafe {
            ffi::check(
                sys::av_hwdevice_ctx_create(&mut ptr, device_type, ptr::null(), ptr::null_mut(), 0),
                "av_hwdevice_ctx_create",
            )
            .map_err(|error| error.with_backend(backend))?;
        }
        Ok(Self { ptr })
    }

    fn new_hw_frames(
        device: &Self,
        backend: GpuBackend,
        format: sys::AVPixelFormat,
        sw_format: sys::AVPixelFormat,
        width: u32,
        height: u32,
    ) -> Result<Self> {
        let ptr = unsafe { sys::av_hwframe_ctx_alloc(device.ptr) };
        if ptr.is_null() {
            return Err(FfmpegError::new(
                "av_hwframe_ctx_alloc",
                "failed to allocate encoder hardware frames context",
            )
            .with_backend(backend));
        }
        unsafe {
            let frames = (*ptr).data.cast::<sys::AVHWFramesContext>();
            (*frames).format = format;
            (*frames).sw_format = sw_format;
            (*frames).width = width as i32;
            (*frames).height = height as i32;
            (*frames).initial_pool_size = 8;
            if let Err(error) = ffi::check(sys::av_hwframe_ctx_init(ptr), "av_hwframe_ctx_init")
                .map_err(|error| error.with_backend(backend))
            {
                let mut ptr = ptr;
                sys::av_buffer_unref(&mut ptr);
                return Err(error);
            }
        }
        Ok(Self { ptr })
    }

    fn clone_ref(&self) -> Option<Self> {
        let ptr = unsafe { sys::av_buffer_ref(self.ptr) };
        (!ptr.is_null()).then_some(Self { ptr })
    }
}

impl Drop for AvBufferRef {
    fn drop(&mut self) {
        unsafe { sys::av_buffer_unref(&mut self.ptr) };
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
    pub cuda_frames: u64,
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
            GpuBackend::Cuda => self.cuda_frames = self.cuda_frames.saturating_add(1),
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

        use crate::gpu::{GpuVideoInput, VulkanVideoFrame};

        use super::*;

        let frame = VulkanVideoFrame::new(
            ash::vk::Image::from_raw(1),
            ash::vk::ImageView::from_raw(2),
            ash::vk::DeviceMemory::from_raw(3),
            ash::vk::Format::R8G8B8A8_UNORM,
            ash::vk::Extent3D {
                width: 1280,
                height: 720,
                depth: 1,
            },
        );
        let input = GpuVideoInput::Vulkan(&frame);
        let descriptor = GpuUploadDescriptor::from_frame(&input);
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

    #[cfg(feature = "cuda")]
    #[test]
    fn cuda_telemetry_tracks_upload_attempts() {
        use std::time::Duration;

        use crate::gpu::{CudaVideoFrame, GpuVideoInput};

        use super::*;

        let frame = CudaVideoFrame::from_device_ptr(0xABCD, 1920, 1080, 8192, Some(12));
        let input = GpuVideoInput::Cuda(&frame);
        let descriptor = GpuUploadDescriptor::from_frame(&input);
        let mut telemetry = GpuEncodeTelemetry::default();

        telemetry.record_upload_started(&descriptor);
        telemetry.record_upload_failed(&descriptor, Duration::from_micros(25), "not imported");

        assert_eq!(telemetry.upload_attempts, 1);
        assert_eq!(telemetry.upload_failures, 1);
        assert_eq!(telemetry.cuda_frames, 1);
        assert_eq!(telemetry.last_error.as_deref(), Some("not imported"));
        assert_eq!(telemetry.recent_events.len(), 2);
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

        use crate::gpu::{GpuVideoInput, VulkanVideoFrame};

        use super::*;

        let path = temp_path("gpu_failure", "mp4");
        let Ok(mut encoder) = MuxedEncoder::create(
            path.to_string_lossy().to_string(),
            VideoEncoderConfig::cpu_rgba(16, 16, 30, VideoCodec::H264),
        ) else {
            eprintln!("H.264 encoder unavailable; skipping GPU input mismatch test");
            return;
        };
        let frame = VulkanVideoFrame::new(
            ash::vk::Image::from_raw(1),
            ash::vk::ImageView::from_raw(2),
            ash::vk::DeviceMemory::from_raw(3),
            ash::vk::Format::R8G8B8A8_UNORM,
            ash::vk::Extent3D {
                width: 16,
                height: 16,
                depth: 1,
            },
        );
        let input = GpuVideoInput::Vulkan(&frame);

        let error = encoder
            .write_gpu_frame(&input)
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
        let mut config = VideoEncoderConfig::cpu_rgba(16, 16, 30, VideoCodec::H264);
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
