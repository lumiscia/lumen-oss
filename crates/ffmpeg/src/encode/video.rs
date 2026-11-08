use std::ptr;

use crate::ffi::{self, AvFrame, CodecContextGuard, SWS_BILINEAR, sys};
use crate::format::Rational;
use crate::gpu::GpuBackend;
use crate::video::{CpuVideoFrame, EncodeMode, PixelFormat};
use crate::{FfmpegError, Result};
use sys::{
    AVColorPrimaries::AVCOL_PRI_BT709,
    AVColorRange::AVCOL_RANGE_MPEG,
    AVColorSpace::AVCOL_SPC_BT709,
    AVColorTransferCharacteristic::AVCOL_TRC_BT709,
    AVPixelFormat::{AV_PIX_FMT_VIDEOTOOLBOX, AV_PIX_FMT_YUV420P},
};

use super::codec::{EncoderFamily, video_encoder_by_name};
use super::common::{refresh_stream_time_base, send_frame_and_write_packets};
use super::hw::{AvBufferRef, EncoderHwContexts, create_encoder_hw_contexts};
use super::output::OutputContext;
use super::quality::{EncoderQuality, apply_quality_options, quality_options};

// swscale range flags: 1 is full (JPEG) range, 0 is limited (MPEG) range.
const SWS_RANGE_FULL: i32 = 1;
const SWS_RANGE_LIMITED: i32 = 0;

#[derive(Debug, Clone)]
pub struct VideoEncoderConfig {
    pub width: u32,
    pub height: u32,
    /// Exact frames per second; use `Rational::from_fps` to convert a floating-point rate.
    pub fps: Rational,
    /// FFmpeg encoder name, such as `libx264` or `h264_vulkan`.
    pub encoder: String,
    pub mode: EncodeMode,
    pub quality: EncoderQuality,
    /// CUDA device the frames sent in `EncodeMode::GpuTexture(GpuBackend::Cuda)` live on; `None`
    /// uses the default device.
    pub cuda_ordinal: Option<i32>,
    /// Device the frames sent in `EncodeMode::GpuTexture(GpuBackend::Vulkan)` live on.
    #[cfg(feature = "vulkan")]
    pub vulkan_device: Option<crate::gpu::VulkanDevice>,
}

impl VideoEncoderConfig {
    pub fn cpu_rgba(
        width: u32,
        height: u32,
        fps: impl Into<Rational>,
        encoder: impl Into<String>,
    ) -> Self {
        Self {
            width,
            height,
            fps: fps.into(),
            encoder: encoder.into(),
            mode: EncodeMode::CpuUpload,
            quality: EncoderQuality::default(),
            cuda_ordinal: None,
            #[cfg(feature = "vulkan")]
            vulkan_device: None,
        }
    }
}

/// A coded frame larger than the requested canvas, cropped back only by container metadata.
/// Players that ignore that metadata display the padding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContainerCrop {
    pub coded_width: u32,
    pub coded_height: u32,
    pub width: u32,
    pub height: u32,
}

/// How frames reach the encoder.
pub(in crate::encode) enum Transport {
    /// RGBA converted into `frame`, then uploaded to `hw_frames` when the encoder takes those.
    Cpu {
        scaler: Scaler,
        frame: AvFrame,
        hw_frames: Option<AvBufferRef>,
    },
    #[cfg(feature = "cuda")]
    Cuda { frames: AvBufferRef },
    #[cfg(feature = "vulkan")]
    Vulkan {
        frames: AvBufferRef,
        device: crate::gpu::VulkanDevice,
    },
    #[cfg(feature = "metal")]
    Metal,
}

pub(in crate::encode) struct Scaler(*mut sys::SwsContext);

impl Drop for Scaler {
    fn drop(&mut self) {
        unsafe { sys::sws_freeContext(self.0) };
    }
}

pub(crate) struct VideoEncoder {
    pub(in crate::encode) width: u32,
    pub(in crate::encode) height: u32,
    container_crop: Option<ContainerCrop>,
    stream_index: usize,
    stream_time_base: sys::AVRational,
    context: *mut sys::AVCodecContext,
    pub(in crate::encode) transport: Transport,
    sent_frame: bool,
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
        if let EncodeMode::GpuTexture(backend) = config.mode
            && !EncoderFamily::of(&config.encoder).takes_textures_from(backend)
        {
            return Err(FfmpegError::new(
                "VideoEncoder::create",
                format!(
                    "encoder `{}` can't take {backend:?} textures",
                    config.encoder
                ),
            )
            .with_backend(backend));
        }
        let codec = video_encoder_by_name(&config.encoder)?;
        let encoder_name = config.encoder.as_str();
        let quality = quality_options(encoder_name, &config.quality, config.mode)?;
        check_container_holds(output, codec, encoder_name)?;
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

        let time_base = sys::AVRational {
            num: framerate.den,
            den: framerate.num,
        };
        let hw_contexts = create_encoder_hw_contexts(&config, codec)?;

        let mut container_crop = None;
        unsafe {
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
            (*context).gop_size = gop_size;
            if let Some(contexts) = &hw_contexts {
                (*context).sw_pix_fmt = contexts.sw_format;
                (*context).hw_frames_ctx = sys::av_buffer_ref(contexts.frames.ptr);
                if (*context).hw_frames_ctx.is_null() {
                    return Err(FfmpegError::new(
                        "av_buffer_ref",
                        "failed to reference encoder hardware frames context",
                    )
                    .with_backend(contexts.backend));
                }
            }
            if ((*(*output.ptr).oformat).flags & sys::AVFMT_GLOBALHEADER) != 0 {
                (*context).flags |= sys::AV_CODEC_FLAG_GLOBAL_HEADER as i32;
            }
            apply_quality_options(context, &quality, encoder_name)?;
            ffi::check(
                sys::avcodec_open2(context, codec, ptr::null_mut()),
                "avcodec_open2",
            )
            .map_err(|mut error| {
                // FFmpeg's Vulkan encoders only say why in their log, and return ENOSYS.
                error.message = if encoder_name.ends_with("_vulkan")
                    && error.code == Some(sys::AVERROR(libc::ENOSYS))
                {
                    format!(
                        "the Vulkan device can't run `{encoder_name}`: it has no video encode queue or doesn't support encoding this codec"
                    )
                } else {
                    format!("encoder `{encoder_name}`: {}", error.message)
                };
                error
            })?;
            ffi::check(
                sys::avcodec_parameters_from_context((*stream).codecpar, context),
                "avcodec_parameters_from_context",
            )?;
            if encoder_name == "av1_vulkan" {
                container_crop = crop_coded_padding(context, (*stream).codecpar)?;
            }
            (*stream).time_base = time_base;
        }

        let transport = create_transport(&config, hw_contexts)?;

        Ok(Self {
            width: config.width,
            height: config.height,
            container_crop,
            stream_index: unsafe { (*stream).index as usize },
            stream_time_base: time_base,
            context: context_guard.release(),
            transport,
            sent_frame: false,
        })
    }

    pub fn container_crop(&self) -> Option<ContainerCrop> {
        self.container_crop
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
        input: &CpuVideoFrame,
    ) -> Result<()> {
        #[allow(irrefutable_let_patterns)]
        let Transport::Cpu {
            scaler,
            frame,
            hw_frames,
        } = &mut self.transport
        else {
            return Err(FfmpegError::new(
                "VideoEncoder::send_cpu_frame",
                "hardware texture encoders consume GPU inputs; create a CPU upload encoder to send CPU bytes",
            ));
        };
        let layout = validate_cpu_frame(input, self.width, self.height)?;
        unsafe {
            ffi::check(
                sys::av_frame_make_writable(frame.as_mut_ptr()),
                "av_frame_make_writable",
            )?;
        }
        let src_data = [input.data.as_ptr(), ptr::null(), ptr::null(), ptr::null()];
        let src_stride = [layout.stride, 0, 0, 0];
        unsafe {
            ffi::check(
                sys::sws_scale(
                    scaler.0,
                    src_data.as_ptr(),
                    src_stride.as_ptr(),
                    0,
                    layout.height,
                    (*frame.as_mut_ptr()).data.as_mut_ptr(),
                    (*frame.as_mut_ptr()).linesize.as_mut_ptr(),
                ),
                "sws_scale",
            )?;
        }
        let mut uploaded = hw_frames
            .as_ref()
            .map(|hw_frames| upload_frame(hw_frames, frame))
            .transpose()?;
        let prepared = uploaded.as_mut().unwrap_or(frame).as_mut_ptr();
        self.send_prepared(output, prepared, input.pts)
    }

    pub(in crate::encode) fn send_prepared(
        &mut self,
        output: &mut OutputContext,
        frame: *mut sys::AVFrame,
        pts: Option<i64>,
    ) -> Result<()> {
        let pts = pts.ok_or_else(|| {
            FfmpegError::new(
                "VideoEncoder::send_prepared",
                "every video frame needs a pts, counted in frames of the encoder's fps",
            )
        })?;
        unsafe { (*frame).pts = pts };
        self.send_frame(output, frame)
    }

    /// Drains the encoder. One that never got a frame has nothing to drain, and FFmpeg's Vulkan
    /// encoders read their last frame on a flush, so they would crash on a null pointer.
    pub(in crate::encode) fn flush(&mut self, output: &mut OutputContext) -> Result<()> {
        if !self.sent_frame {
            return Ok(());
        }
        self.send_frame(output, ptr::null())
    }

    fn send_frame(&mut self, output: &mut OutputContext, frame: *const sys::AVFrame) -> Result<()> {
        send_frame_and_write_packets(
            self.context,
            frame,
            output,
            self.stream_index,
            self.stream_time_base,
            1,
        )?;
        self.sent_frame = true;
        Ok(())
    }
}

fn create_transport(
    config: &VideoEncoderConfig,
    hw_contexts: Option<EncoderHwContexts>,
) -> Result<Transport> {
    match (config.mode, hw_contexts) {
        (EncodeMode::CpuUpload, hw_contexts) => {
            // CPU frames are converted to this format, then uploaded when the encoder takes hardware frames.
            let format = hw_contexts
                .as_ref()
                .map_or(AV_PIX_FMT_YUV420P, |contexts| contexts.sw_format);
            let (width, height) = (config.width as i32, config.height as i32);
            let mut frame = AvFrame::new()?;
            unsafe {
                (*frame.as_mut_ptr()).format = format as i32;
                (*frame.as_mut_ptr()).width = width;
                (*frame.as_mut_ptr()).height = height;
                ffi::check(
                    sys::av_frame_get_buffer(frame.as_mut_ptr(), 32),
                    "av_frame_get_buffer",
                )?;
            }
            Ok(Transport::Cpu {
                scaler: rgba_scaler(width, height, format)?,
                frame,
                hw_frames: hw_contexts.map(|contexts| contexts.frames),
            })
        }
        #[cfg(feature = "cuda")]
        (EncodeMode::GpuTexture(GpuBackend::Cuda), Some(contexts)) => Ok(Transport::Cuda {
            frames: contexts.frames,
        }),
        #[cfg(feature = "vulkan")]
        (EncodeMode::GpuTexture(GpuBackend::Vulkan), Some(contexts)) => Ok(Transport::Vulkan {
            frames: contexts.frames,
            device: config
                .vulkan_device
                .clone()
                .ok_or_else(super::hw::missing_vulkan_device)?,
        }),
        #[cfg(feature = "metal")]
        (EncodeMode::GpuTexture(GpuBackend::Metal), None) => Ok(Transport::Metal),
        (EncodeMode::GpuTexture(backend), _) => Err(FfmpegError::new(
            "VideoEncoder::create",
            format!("{backend:?} texture encode has no frames context"),
        )
        .with_backend(backend)),
    }
}

/// Full-range RGB in, BT.709 limited-range YUV out, matching the stream's colour tags.
fn rgba_scaler(width: i32, height: i32, format: sys::AVPixelFormat) -> Result<Scaler> {
    let scaler = unsafe {
        sys::sws_getContext(
            width,
            height,
            PixelFormat::Rgba8.to_av_pixel_format(),
            width,
            height,
            format,
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
    let scaler = Scaler(scaler);
    let coefficients = unsafe { sys::sws_getCoefficients(sys::SWS_CS_ITU709) };
    let configured = unsafe {
        sys::sws_setColorspaceDetails(
            scaler.0,
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
        return Err(FfmpegError::new(
            "sws_setColorspaceDetails",
            "failed to configure BT.709 color conversion",
        ));
    }
    Ok(scaler)
}

fn upload_frame(hw_frames: &AvBufferRef, frame: &AvFrame) -> Result<AvFrame> {
    let mut uploaded = AvFrame::new()?;
    unsafe {
        ffi::check(
            sys::av_hwframe_get_buffer(hw_frames.ptr, uploaded.as_mut_ptr(), 0),
            "av_hwframe_get_buffer",
        )?;
        ffi::check(
            sys::av_hwframe_transfer_data(uploaded.as_mut_ptr(), frame.as_ptr(), 0),
            "av_hwframe_transfer_data",
        )?;
    }
    Ok(uploaded)
}

/// `av1_vulkan` pads frames to the hardware's coded alignment and AV1 has no crop window, so the
/// stream declares the coded size and the container crops back to the requested one.
#[cfg(ffmpeg_frame_cropping)]
unsafe fn crop_coded_padding(
    context: *const sys::AVCodecContext,
    params: *mut sys::AVCodecParameters,
) -> Result<Option<ContainerCrop>> {
    unsafe {
        let (width, height) = ((*context).width, (*context).height);
        let (coded_width, coded_height) = ((*context).coded_width, (*context).coded_height);
        if coded_width == width && coded_height == height {
            return Ok(None);
        }
        let crop: Vec<u8> = [0, coded_height - height, 0, coded_width - width]
            .into_iter()
            .flat_map(|side| side.to_le_bytes())
            .collect();
        let side_data = sys::av_packet_side_data_new(
            &mut (*params).coded_side_data,
            &mut (*params).nb_coded_side_data,
            sys::AVPacketSideDataType::AV_PKT_DATA_FRAME_CROPPING,
            crop.len(),
            0,
        );
        if side_data.is_null() {
            return Err(FfmpegError::new(
                "av_packet_side_data_new",
                "failed to allocate stream cropping side data",
            ));
        }
        ptr::copy_nonoverlapping(crop.as_ptr(), (*side_data).data, crop.len());
        (*params).width = coded_width;
        (*params).height = coded_height;
        Ok(Some(ContainerCrop {
            coded_width: coded_width as u32,
            coded_height: coded_height as u32,
            width: width as u32,
            height: height as u32,
        }))
    }
}

#[cfg(not(ffmpeg_frame_cropping))]
unsafe fn crop_coded_padding(
    _context: *const sys::AVCodecContext,
    _params: *mut sys::AVCodecParameters,
) -> Result<Option<ContainerCrop>> {
    Ok(None)
}

/// Errors when the output's container can't hold what `codec` writes, such as H.264 in `.webm`.
fn check_container_holds(
    output: &OutputContext,
    codec: *const sys::AVCodec,
    encoder: &str,
) -> Result<()> {
    unsafe {
        let format = (*output.ptr).oformat;
        if sys::avformat_query_codec(format, (*codec).id, sys::FF_COMPLIANCE_NORMAL) == 0 {
            let container = std::ffi::CStr::from_ptr((*format).name).to_string_lossy();
            return Err(FfmpegError::new(
                "VideoEncoder::create",
                format!(
                    "the {container} container can't hold the video `{encoder}` encodes; use a codec it supports or another container"
                ),
            )
            .with_path(output.path()));
        }
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

impl Drop for VideoEncoder {
    fn drop(&mut self) {
        unsafe { sys::avcodec_free_context(&mut self.context) };
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
    fn malformed_rgba_frames_are_rejected() {
        type Corrupt = fn(&mut CpuVideoFrame);
        let cases: [(&str, Corrupt, u32, &str); 5] = [
            (
                "non-RGBA pixel format",
                |frame| frame.pixel_format = PixelFormat::Bgra8,
                16,
                "require RGBA8",
            ),
            (
                "mismatched dimensions",
                |_| {},
                8,
                "do not match encoder dimensions",
            ),
            (
                "short stride",
                |frame| frame.stride -= 1,
                16,
                "smaller than the required",
            ),
            (
                "undersized buffer",
                |frame| {
                    frame.data.pop();
                },
                16,
                "layout requires at least",
            ),
            (
                "stride beyond i32",
                |frame| frame.stride = i32::MAX as usize + 1,
                16,
                "stride exceeds FFmpeg",
            ),
        ];
        for (name, corrupt, expected_width, expected_message) in cases {
            let mut frame = rgba_frame(16, 9);
            corrupt(&mut frame);

            let error = validate_cpu_frame(&frame, expected_width, 9).unwrap_err();
            assert!(error.message.contains(expected_message), "{name}: {error}");
        }
    }
}
