use std::{ffi::CStr, ptr};

use crate::ffi::{self, AvFrame, AvPacket, SWS_BILINEAR, sys};
use crate::format::Rational;
use crate::gpu::GpuBackend;
use crate::video::{CpuVideoFrame, EncodeMode, PixelFormat, VideoCodec};
use crate::{FfmpegError, Result};
use sys::{
    AVColorPrimaries::AVCOL_PRI_BT709,
    AVColorRange::AVCOL_RANGE_MPEG,
    AVColorSpace::AVCOL_SPC_BT709,
    AVColorTransferCharacteristic::AVCOL_TRC_BT709,
    AVMediaType::AVMEDIA_TYPE_VIDEO,
    AVPixelFormat::{AV_PIX_FMT_VIDEOTOOLBOX, AV_PIX_FMT_YUV420P},
};

use super::codec::find_encoder;
use super::common::refresh_stream_time_base;
use super::hw::{AvBufferRef, create_encoder_hw_contexts};
use super::output::OutputContext;
use super::quality::{QualityOptions, RateControl, quality_options};
use super::telemetry::GpuEncodeTelemetry;

// swscale range flags: 1 is full (JPEG) range, 0 is limited (MPEG) range.
const SWS_RANGE_FULL: i32 = 1;
const SWS_RANGE_LIMITED: i32 = 0;

#[derive(Debug, Clone)]
pub struct VideoEncoderConfig {
    pub width: u32,
    pub height: u32,
    /// Exact frames per second; use `Rational::from_fps` to convert a floating-point rate.
    pub fps: Rational,
    pub codec: VideoCodec,
    pub encoder_name: Option<String>,
    pub mode: EncodeMode,
    pub bit_rate: i64,
    /// Overrides `bit_rate` when set.
    pub rate_control: Option<RateControl>,
    /// Encoder preset: `ultrafast`..`placebo` for libx264, `p1`..`p7` for NVENC.
    pub preset: Option<String>,
    /// Device the frames sent in `EncodeMode::GpuTexture(GpuBackend::Vulkan)` live on.
    #[cfg(feature = "vulkan")]
    pub vulkan_device: Option<crate::gpu::VulkanDevice>,
}

impl VideoEncoderConfig {
    pub fn cpu_rgba(width: u32, height: u32, fps: impl Into<Rational>, codec: VideoCodec) -> Self {
        Self {
            width,
            height,
            fps: fps.into(),
            codec,
            encoder_name: None,
            mode: EncodeMode::CpuUpload,
            bit_rate: 8_000_000,
            rate_control: None,
            preset: None,
            #[cfg(feature = "vulkan")]
            vulkan_device: None,
        }
    }

    pub fn h264_videotoolbox(width: u32, height: u32, fps: impl Into<Rational>) -> Self {
        Self {
            encoder_name: Some("h264_videotoolbox".to_string()),
            ..Self::cpu_rgba(width, height, fps, VideoCodec::H264)
        }
    }
}

pub struct VideoEncoder {
    pub(in crate::encode) width: u32,
    pub(in crate::encode) height: u32,
    stream_index: usize,
    stream_time_base: sys::AVRational,
    pub(in crate::encode) context: *mut sys::AVCodecContext,
    frame: AvFrame,
    scaler: *mut sys::SwsContext,
    #[allow(dead_code)]
    pub(in crate::encode) hw_device: Option<AvBufferRef>,
    pub(in crate::encode) hw_frames: Option<AvBufferRef>,
    pub(in crate::encode) next_pts: i64,
    pub(in crate::encode) mode: EncodeMode,
    pub(in crate::encode) gpu_telemetry: GpuEncodeTelemetry,
    #[cfg(feature = "vulkan")]
    pub(in crate::encode) vulkan_device: Option<crate::gpu::VulkanDevice>,
}

unsafe impl Send for VideoEncoder {}

impl VideoEncoder {
    pub fn create(output: &mut OutputContext, config: VideoEncoderConfig) -> Result<Self> {
        if config.width == 0
            || config.height == 0
            || config.fps.numerator <= 0
            || config.fps.denominator <= 0
        {
            return Err(FfmpegError::new(
                "VideoEncoder::create",
                "width, height, and fps must be greater than zero",
            ));
        }
        let width = i32::try_from(config.width).map_err(|_| {
            FfmpegError::new(
                "VideoEncoder::create",
                "width exceeds FFmpeg's supported integer range",
            )
        })?;
        let height = i32::try_from(config.height).map_err(|_| {
            FfmpegError::new(
                "VideoEncoder::create",
                "height exceeds FFmpeg's supported integer range",
            )
        })?;
        let framerate = sys::AVRational {
            num: config.fps.numerator,
            den: config.fps.denominator,
        };
        // Two seconds of frames, rounded to the nearest whole frame.
        let gop_frames = (i64::from(framerate.num) * 2 + i64::from(framerate.den) / 2)
            / i64::from(framerate.den);
        let gop_size = i32::try_from(gop_frames.max(1)).map_err(|_| {
            FfmpegError::new(
                "VideoEncoder::create",
                "fps is too large to calculate the encoder GOP size",
            )
        })?;
        #[cfg(feature = "vulkan")]
        if config.mode == EncodeMode::GpuTexture(GpuBackend::Vulkan) {
            if !config.width.is_multiple_of(2) || !config.height.is_multiple_of(2) {
                return Err(FfmpegError::new(
                    "VideoEncoder::create",
                    format!(
                        "Vulkan texture encode needs even dimensions for NV12, got {}x{}",
                        config.width, config.height,
                    ),
                )
                .with_backend(GpuBackend::Vulkan));
            }
            if config.vulkan_device.is_none() {
                return Err(super::hw::missing_vulkan_device());
            }
        }
        let codec = find_encoder(&config)?;
        if codec.is_null() {
            return Err(FfmpegError::new(
                "avcodec_find_encoder",
                "requested encoder is unavailable",
            )
            .with_codec(config.codec));
        }
        let encoder_name = unsafe { CStr::from_ptr((*codec).name) }.to_string_lossy();
        let quality = quality_options(
            &encoder_name,
            config
                .rate_control
                .unwrap_or_else(|| RateControl::Bitrate(config.bit_rate.max(0) as u64)),
            config.preset.as_deref(),
        )
        .map_err(|error| error.with_codec(config.codec))?;
        let stream = unsafe { sys::avformat_new_stream(output.ptr, ptr::null()) };
        if stream.is_null() {
            return Err(FfmpegError::new(
                "avformat_new_stream",
                "failed to allocate output stream",
            ));
        }
        let mut context_guard = CodecContextGuard(unsafe { sys::avcodec_alloc_context3(codec) });
        let context = context_guard.0;
        if context.is_null() {
            return Err(FfmpegError::new(
                "avcodec_alloc_context3",
                "failed to allocate encoder context",
            ));
        }

        apply_private_options(context, &quality, &encoder_name)?;

        let time_base = sys::AVRational {
            num: framerate.den,
            den: framerate.num,
        };
        let hw_contexts = create_encoder_hw_contexts(&config, codec)?;
        // CPU frames are converted to this format, then uploaded when the encoder takes hardware frames.
        let cpu_format = match &hw_contexts {
            Some(contexts) if config.mode == EncodeMode::CpuUpload => contexts.sw_format,
            _ => AV_PIX_FMT_YUV420P,
        };

        unsafe {
            (*context).codec_id = config.codec.to_av_codec_id();
            (*context).codec_type = AVMEDIA_TYPE_VIDEO;
            (*context).width = width;
            (*context).height = height;
            (*context).time_base = time_base;
            (*context).framerate = framerate;
            (*context).pix_fmt = match (&hw_contexts, config.mode) {
                (Some(contexts), _) => contexts.hw_format,
                (None, EncodeMode::GpuTexture(GpuBackend::Metal)) => AV_PIX_FMT_VIDEOTOOLBOX,
                (None, _) => AV_PIX_FMT_YUV420P,
            };
            (*context).color_primaries = AVCOL_PRI_BT709;
            (*context).color_trc = AVCOL_TRC_BT709;
            (*context).colorspace = AVCOL_SPC_BT709;
            (*context).color_range = AVCOL_RANGE_MPEG;
            (*context).bit_rate = quality.bit_rate;
            if let Some(global_quality) = quality.global_quality {
                (*context).global_quality = global_quality;
            }
            (*context).gop_size = gop_size;
            if let Some(contexts) = &hw_contexts {
                (*context).sw_pix_fmt = contexts.sw_format;
                if let Some(frames) = &contexts.frames {
                    (*context).hw_frames_ctx = sys::av_buffer_ref(frames.ptr);
                    if (*context).hw_frames_ctx.is_null() {
                        return Err(FfmpegError::new(
                            "av_buffer_ref",
                            "failed to reference encoder hardware frames context",
                        )
                        .with_backend(contexts.backend));
                    }
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
                (*frame.as_mut_ptr()).format = cpu_format as i32;
                (*frame.as_mut_ptr()).width = width;
                (*frame.as_mut_ptr()).height = height;
                ffi::check(
                    sys::av_frame_get_buffer(frame.as_mut_ptr(), 32),
                    "av_frame_get_buffer",
                )?;
            }

            let scaler = unsafe {
                sys::sws_getContext(
                    width,
                    height,
                    PixelFormat::Rgba8.to_av_pixel_format(),
                    width,
                    height,
                    cpu_format,
                    SWS_BILINEAR,
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
            // Full-range RGB in, BT.709 limited-range YUV out, matching the stream's colour tags.
            let coefficients = unsafe { sys::sws_getCoefficients(sys::SWS_CS_ITU709) };
            let configured = unsafe {
                sys::sws_setColorspaceDetails(
                    scaler,
                    coefficients,
                    SWS_RANGE_FULL,
                    coefficients,
                    SWS_RANGE_LIMITED,
                    0,
                    1 << 16,
                    1 << 16,
                )
            };
            if configured < 0 {
                unsafe { sys::sws_freeContext(scaler) };
                return Err(FfmpegError::new(
                    "sws_setColorspaceDetails",
                    "failed to configure BT.709 color conversion",
                ));
            }
            scaler
        } else {
            ptr::null_mut()
        };

        #[cfg(feature = "vulkan")]
        let vulkan_device = config
            .vulkan_device
            .clone()
            .filter(|_| config.mode == EncodeMode::GpuTexture(GpuBackend::Vulkan));

        Ok(Self {
            width: config.width,
            height: config.height,
            stream_index: unsafe { (*stream).index as usize },
            stream_time_base: time_base,
            context: context_guard.release(),
            frame,
            scaler,
            hw_device: hw_contexts
                .as_ref()
                .and_then(|contexts| contexts.device.clone_ref()),
            hw_frames: hw_contexts.and_then(|contexts| contexts.frames),
            next_pts: 0,
            mode: config.mode,
            gpu_telemetry: GpuEncodeTelemetry::default(),
            #[cfg(feature = "vulkan")]
            vulkan_device,
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
        let layout = validate_cpu_frame(frame, self.width, self.height)?;
        unsafe {
            ffi::check(
                sys::av_frame_make_writable(self.frame.as_mut_ptr()),
                "av_frame_make_writable",
            )?;
        }
        let src_data = [frame.data.as_ptr(), ptr::null(), ptr::null(), ptr::null()];
        let src_stride = [layout.stride, 0, 0, 0];
        unsafe {
            ffi::check(
                sys::sws_scale(
                    self.scaler,
                    src_data.as_ptr(),
                    src_stride.as_ptr(),
                    0,
                    layout.height,
                    (*self.frame.as_mut_ptr()).data.as_mut_ptr(),
                    (*self.frame.as_mut_ptr()).linesize.as_mut_ptr(),
                ),
                "sws_scale",
            )?;
            (*self.frame.as_mut_ptr()).pts = frame.pts.unwrap_or(self.next_pts);
        }
        self.next_pts = self.next_pts.saturating_add(1);
        let Some(hw_frames) = self.hw_frames.as_ref() else {
            return self.send_frame(output, self.frame.as_ptr());
        };
        let mut hw_frame = AvFrame::new()?;
        unsafe {
            ffi::check(
                sys::av_hwframe_get_buffer(hw_frames.ptr, hw_frame.as_mut_ptr(), 0),
                "av_hwframe_get_buffer",
            )?;
            ffi::check(
                sys::av_hwframe_transfer_data(hw_frame.as_mut_ptr(), self.frame.as_ptr(), 0),
                "av_hwframe_transfer_data",
            )?;
            (*hw_frame.as_mut_ptr()).pts = (*self.frame.as_ptr()).pts;
        }
        self.send_frame(output, hw_frame.as_ptr())
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

fn apply_private_options(
    context: *mut sys::AVCodecContext,
    quality: &QualityOptions,
    encoder_name: &str,
) -> Result<()> {
    for (name, value) in &quality.options {
        let c_name = ffi::cstring("av_opt_set", name)?;
        let c_value = ffi::cstring("av_opt_set", value)?;
        let code =
            unsafe { sys::av_opt_set((*context).priv_data, c_name.as_ptr(), c_value.as_ptr(), 0) };
        ffi::check(code, "av_opt_set").map_err(|mut error| {
            error.message = format!(
                "failed to set `{name}={value}` on encoder `{encoder_name}`: {}",
                error.message
            );
            error
        })?;
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ValidatedCpuFrameLayout {
    stride: i32,
    height: i32,
}

fn validate_cpu_frame(
    frame: &CpuVideoFrame,
    expected_width: u32,
    expected_height: u32,
) -> Result<ValidatedCpuFrameLayout> {
    const OPERATION: &str = "VideoEncoder::send_cpu_frame";

    if frame.pixel_format != PixelFormat::Rgba8 {
        return Err(FfmpegError::new(
            OPERATION,
            "CPU upload encoders require RGBA8 input frames",
        ));
    }
    if frame.width != expected_width || frame.height != expected_height {
        return Err(FfmpegError::new(
            OPERATION,
            format!(
                "frame dimensions {}x{} do not match encoder dimensions {expected_width}x{expected_height}",
                frame.width, frame.height
            ),
        ));
    }

    let width = usize::try_from(frame.width).map_err(|_| {
        FfmpegError::new(OPERATION, "frame width exceeds the platform integer range")
    })?;
    let row_bytes = width.checked_mul(4).ok_or_else(|| {
        FfmpegError::new(
            OPERATION,
            "frame row byte length exceeds the platform integer range",
        )
    })?;
    if frame.stride < row_bytes {
        return Err(FfmpegError::new(
            OPERATION,
            format!(
                "frame stride {} is smaller than the required RGBA8 row length {row_bytes}",
                frame.stride
            ),
        ));
    }

    let stride = i32::try_from(frame.stride).map_err(|_| {
        FfmpegError::new(
            OPERATION,
            "frame stride exceeds FFmpeg's supported integer range",
        )
    })?;
    let height = i32::try_from(frame.height).map_err(|_| {
        FfmpegError::new(
            OPERATION,
            "frame height exceeds FFmpeg's supported integer range",
        )
    })?;
    let required_len = frame
        .stride
        .checked_mul(frame.height as usize)
        .ok_or_else(|| {
            FfmpegError::new(
                OPERATION,
                "frame buffer length exceeds the platform integer range",
            )
        })?;
    if frame.data.len() < required_len {
        return Err(FfmpegError::new(
            OPERATION,
            format!(
                "frame buffer contains {} bytes but its layout requires at least {required_len}",
                frame.data.len()
            ),
        ));
    }

    Ok(ValidatedCpuFrameLayout { stride, height })
}

/// Frees the codec context on early returns until ownership moves into the encoder.
struct CodecContextGuard(*mut sys::AVCodecContext);

impl CodecContextGuard {
    fn release(&mut self) -> *mut sys::AVCodecContext {
        std::mem::replace(&mut self.0, ptr::null_mut())
    }
}

impl Drop for CodecContextGuard {
    fn drop(&mut self) {
        unsafe { sys::avcodec_free_context(&mut self.0) };
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

#[cfg(test)]
mod tests {
    use super::*;

    fn rgba_frame(width: u32, height: u32) -> CpuVideoFrame {
        let stride = width as usize * 4;
        CpuVideoFrame {
            width,
            height,
            stride,
            pixel_format: PixelFormat::Rgba8,
            pts: None,
            data: vec![0; stride * height as usize],
        }
    }

    #[test]
    fn accepts_valid_rgba_frame_layout() {
        let frame = rgba_frame(16, 9);

        assert_eq!(
            validate_cpu_frame(&frame, 16, 9).unwrap(),
            ValidatedCpuFrameLayout {
                stride: 64,
                height: 9,
            }
        );
    }

    #[test]
    fn rejects_non_rgba_frame() {
        let mut frame = rgba_frame(16, 9);
        frame.pixel_format = PixelFormat::Bgra8;

        let error = validate_cpu_frame(&frame, 16, 9).unwrap_err();
        assert!(error.message.contains("require RGBA8"));
    }

    #[test]
    fn rejects_mismatched_dimensions() {
        let frame = rgba_frame(16, 9);

        let error = validate_cpu_frame(&frame, 8, 9).unwrap_err();
        assert!(error.message.contains("do not match encoder dimensions"));
    }

    #[test]
    fn rejects_short_stride() {
        let mut frame = rgba_frame(16, 9);
        frame.stride -= 1;

        let error = validate_cpu_frame(&frame, 16, 9).unwrap_err();
        assert!(error.message.contains("smaller than the required"));
    }

    #[test]
    fn rejects_undersized_buffer() {
        let mut frame = rgba_frame(16, 9);
        frame.data.pop();

        let error = validate_cpu_frame(&frame, 16, 9).unwrap_err();
        assert!(error.message.contains("layout requires at least"));
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn rejects_stride_outside_ffmpeg_integer_range() {
        let mut frame = rgba_frame(1, 1);
        frame.stride = i32::MAX as usize + 1;

        let error = validate_cpu_frame(&frame, 1, 1).unwrap_err();
        assert!(error.message.contains("stride exceeds FFmpeg"));
    }
}
