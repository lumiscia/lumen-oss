use std::ptr;

use crate::ffi::{self, AvFrame, AvPacket, sys};
use crate::gpu::GpuBackend;
use crate::video::{CpuVideoFrame, EncodeMode, PixelFormat, VideoCodec};
use crate::{FfmpegError, Result};
#[cfg(target_os = "linux")]
use sys::SWS_BILINEAR;
#[cfg(not(target_os = "linux"))]
use sys::SwsFlags::SWS_BILINEAR;
use sys::{
    AVMediaType::AVMEDIA_TYPE_VIDEO,
    AVPixelFormat::{AV_PIX_FMT_CUDA, AV_PIX_FMT_VIDEOTOOLBOX, AV_PIX_FMT_YUV420P},
};

use super::codec::find_encoder;
use super::common::refresh_stream_time_base;
use super::hw::{AvBufferRef, create_encoder_hw_contexts};
use super::output::OutputContext;
use super::telemetry::GpuEncodeTelemetry;

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

pub struct VideoEncoder {
    stream_index: usize,
    stream_time_base: sys::AVRational,
    pub(in crate::encode) context: *mut sys::AVCodecContext,
    frame: AvFrame,
    scaler: *mut sys::SwsContext,
    pub(in crate::encode) hw_device: Option<AvBufferRef>,
    pub(in crate::encode) hw_frames: Option<AvBufferRef>,
    pub(in crate::encode) next_pts: i64,
    pub(in crate::encode) mode: EncodeMode,
    pub(in crate::encode) gpu_telemetry: GpuEncodeTelemetry,
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
                EncodeMode::GpuTexture(GpuBackend::Cuda) => AV_PIX_FMT_CUDA,
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
            hw_device: hw_contexts
                .as_ref()
                .and_then(|contexts| contexts.device.clone_ref()),
            hw_frames: hw_contexts.and_then(|contexts| contexts.frames),
            next_pts: 0,
            mode: config.mode,
            gpu_telemetry: GpuEncodeTelemetry::default(),
        })
    }

    pub fn gpu_telemetry(&self) -> &GpuEncodeTelemetry {
        &self.gpu_telemetry
    }

    pub(in crate::encode) fn refresh_stream_time_base(
        &mut self,
        output: &OutputContext,
    ) -> Result<()> {
        self.stream_time_base = refresh_stream_time_base(
            output,
            self.stream_index,
            "VideoEncoder::refresh_stream_time_base",
        )?;
        Ok(())
    }

    pub(in crate::encode) fn send_cpu_frame(
        &mut self,
        output: &mut OutputContext,
        frame: &CpuVideoFrame,
    ) -> Result<()> {
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

    pub(in crate::encode) fn flush(&mut self, output: &mut OutputContext) -> Result<()> {
        self.send_frame(output, ptr::null())
    }

    pub(in crate::encode) fn send_frame(
        &mut self,
        output: &mut OutputContext,
        frame: *const sys::AVFrame,
    ) -> Result<()> {
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
                .map_err(|error| error.with_path(output.path().to_string()))?;
            }
        }
        Ok(())
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
