use std::ptr;

#[cfg(feature = "metal")]
use std::ptr::NonNull;

use crate::{
    FfmpegError, Result,
    ffi::{self, AvFrame, sys},
    format::{InputContext, Packet, Rational},
    gpu::{GpuBackend, GpuVideoFrame},
};
#[cfg(target_os = "linux")]
use sys::SWS_BILINEAR;
#[cfg(not(target_os = "linux"))]
use sys::SwsFlags::SWS_BILINEAR;
use sys::{
    AVCodecID::{
        AV_CODEC_ID_AV1, AV_CODEC_ID_H264, AV_CODEC_ID_HEVC, AV_CODEC_ID_NONE, AV_CODEC_ID_PRORES,
        AV_CODEC_ID_VP9,
    },
    AVHWDeviceType::{
        AV_HWDEVICE_TYPE_CUDA, AV_HWDEVICE_TYPE_VIDEOTOOLBOX, AV_HWDEVICE_TYPE_VULKAN,
    },
    AVPixelFormat::{
        AV_PIX_FMT_BGRA, AV_PIX_FMT_CUDA, AV_PIX_FMT_NONE, AV_PIX_FMT_NV12, AV_PIX_FMT_P010BE,
        AV_PIX_FMT_P010LE, AV_PIX_FMT_RGBA, AV_PIX_FMT_VIDEOTOOLBOX, AV_PIX_FMT_VULKAN,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodec {
    H264,
    Hevc,
    Av1,
    Vp9,
    ProRes,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    Cuda,
    Rgba8,
    Bgra8,
    Nv12,
    P010,
    Vulkan,
    Metal,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeMode {
    Cpu,
    Gpu(GpuBackend),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodeMode {
    CpuUpload,
    GpuTexture(GpuBackend),
}

#[derive(Debug, Clone)]
pub struct CpuVideoFrame {
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub pixel_format: PixelFormat,
    pub pts: Option<i64>,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Copy)]
pub struct VideoDecoderConfig {
    pub stream_index: usize,
    pub mode: DecodeMode,
}

impl Default for VideoDecoderConfig {
    fn default() -> Self {
        Self {
            stream_index: 0,
            mode: DecodeMode::Cpu,
        }
    }
}

pub struct VideoDecoder {
    stream_index: usize,
    mode: DecodeMode,
    codec: VideoCodec,
    time_base: Rational,
    context: *mut sys::AVCodecContext,
    scaler: *mut sys::SwsContext,
    hw_device: Option<HwDeviceContext>,
}

unsafe impl Send for VideoDecoder {}

impl VideoCodec {
    pub(crate) fn from_av_codec_id(codec_id: sys::AVCodecID) -> Self {
        match codec_id {
            AV_CODEC_ID_H264 => Self::H264,
            AV_CODEC_ID_HEVC => Self::Hevc,
            AV_CODEC_ID_AV1 => Self::Av1,
            AV_CODEC_ID_VP9 => Self::Vp9,
            AV_CODEC_ID_PRORES => Self::ProRes,
            _ => Self::Unknown,
        }
    }

    pub(crate) fn to_av_codec_id(self) -> sys::AVCodecID {
        match self {
            Self::H264 => AV_CODEC_ID_H264,
            Self::Hevc => AV_CODEC_ID_HEVC,
            Self::Av1 => AV_CODEC_ID_AV1,
            Self::Vp9 => AV_CODEC_ID_VP9,
            Self::ProRes => AV_CODEC_ID_PRORES,
            Self::Unknown => AV_CODEC_ID_NONE,
        }
    }
}

impl PixelFormat {
    pub(crate) fn from_av_pixel_format(format: sys::AVPixelFormat) -> Self {
        match format {
            AV_PIX_FMT_RGBA => Self::Rgba8,
            AV_PIX_FMT_CUDA => Self::Cuda,
            AV_PIX_FMT_BGRA => Self::Bgra8,
            AV_PIX_FMT_NV12 => Self::Nv12,
            AV_PIX_FMT_P010LE | AV_PIX_FMT_P010BE => Self::P010,
            AV_PIX_FMT_VULKAN => Self::Vulkan,
            AV_PIX_FMT_VIDEOTOOLBOX => Self::Metal,
            _ => Self::Unknown,
        }
    }

    pub(crate) fn to_av_pixel_format(self) -> sys::AVPixelFormat {
        match self {
            Self::Rgba8 => AV_PIX_FMT_RGBA,
            Self::Cuda => AV_PIX_FMT_CUDA,
            Self::Bgra8 => AV_PIX_FMT_BGRA,
            Self::Nv12 => AV_PIX_FMT_NV12,
            Self::P010 => AV_PIX_FMT_P010LE,
            Self::Vulkan => AV_PIX_FMT_VULKAN,
            Self::Metal => AV_PIX_FMT_VIDEOTOOLBOX,
            Self::Unknown => AV_PIX_FMT_NONE,
        }
    }
}

impl VideoDecoder {
    pub fn open(input: &InputContext, config: VideoDecoderConfig) -> Result<Self> {
        let parameters = input.stream_parameters(config.stream_index)?;
        let codec_id = unsafe { (*parameters).codec_id };
        let stable_codec = VideoCodec::from_av_codec_id(codec_id);
        let codec = find_decoder(codec_id)?;
        if let DecodeMode::Gpu(backend) = config.mode {
            ensure_hardware_decoder(codec, stable_codec, backend)?;
        }
        let context = unsafe { sys::avcodec_alloc_context3(codec) };
        if context.is_null() {
            return Err(FfmpegError::new(
                "avcodec_alloc_context3",
                "failed to allocate video decoder context",
            ));
        }

        let mut decoder = Self {
            stream_index: config.stream_index,
            mode: config.mode,
            codec: stable_codec,
            time_base: input.stream_time_base(config.stream_index)?.into(),
            context,
            scaler: ptr::null_mut(),
            hw_device: None,
        };

        unsafe {
            ffi::check(
                sys::avcodec_parameters_to_context(decoder.context, parameters),
                "avcodec_parameters_to_context",
            )
            .map_err(|error| {
                error
                    .with_codec(stable_codec)
                    .with_stream_index(config.stream_index)
            })?;
            (*decoder.context).thread_count = 0;
            (*decoder.context).thread_type = sys::FF_THREAD_FRAME | sys::FF_THREAD_SLICE;
        }

        if let DecodeMode::Gpu(backend) = config.mode {
            decoder.hw_device = Some(HwDeviceContext::create(backend)?);
            unsafe {
                (*decoder.context).hw_device_ctx =
                    sys::av_buffer_ref(decoder.hw_device.as_ref().unwrap().ptr);
                (*decoder.context).get_format = match backend {
                    GpuBackend::Metal => Some(force_metal_pixel_format),
                    GpuBackend::Vulkan => Some(force_vulkan_pixel_format),
                    GpuBackend::Cuda => Some(force_cuda_pixel_format),
                };
            }
        }

        unsafe {
            ffi::check(
                sys::avcodec_open2(decoder.context, codec, ptr::null_mut()),
                "avcodec_open2",
            )
            .map_err(|error| {
                error
                    .with_codec(stable_codec)
                    .with_stream_index(config.stream_index)
            })?;
        }

        Ok(decoder)
    }

    pub fn codec(&self) -> VideoCodec {
        self.codec
    }

    pub fn stream_index(&self) -> usize {
        self.stream_index
    }

    pub fn time_base(&self) -> Rational {
        self.time_base
    }

    pub fn send_packet(&mut self, packet: &Packet) -> Result<()> {
        if packet.stream_index() != self.stream_index {
            return Ok(());
        }
        unsafe {
            ffi::check(
                sys::avcodec_send_packet(self.context, packet.inner.as_ptr()),
                "avcodec_send_packet",
            )
            .map_err(|error| {
                error
                    .with_codec(self.codec)
                    .with_stream_index(self.stream_index)
            })
        }
    }

    pub fn send_eof(&mut self) -> Result<()> {
        unsafe {
            ffi::check(
                sys::avcodec_send_packet(self.context, ptr::null()),
                "avcodec_send_packet",
            )
        }
    }

    pub fn flush(&mut self) {
        unsafe { sys::avcodec_flush_buffers(self.context) };
    }

    pub fn receive_cpu_frame(&mut self) -> Result<Option<CpuVideoFrame>> {
        if let DecodeMode::Gpu(backend) = self.mode {
            return Err(FfmpegError::new(
                "receive_cpu_frame",
                "hardware decoders produce GPU frames; create a CPU decoder to receive CPU bytes",
            )
            .with_backend(backend)
            .with_codec(self.codec)
            .with_stream_index(self.stream_index));
        }
        let mut frame = AvFrame::new()?;
        match self.receive_frame(&mut frame)? {
            ReceiveStatus::Again => Ok(None),
            ReceiveStatus::Frame => self.frame_to_rgba(&frame).map(Some),
        }
    }

    pub fn receive_gpu_frame(&mut self) -> Result<Option<GpuVideoFrame>> {
        if self.mode == DecodeMode::Cpu {
            return Err(FfmpegError::new(
                "receive_gpu_frame",
                "CPU decoders produce CPU frames; create a hardware decoder to receive GPU textures",
            )
            .with_codec(self.codec)
            .with_stream_index(self.stream_index));
        }
        let mut frame = AvFrame::new()?;
        match self.receive_frame(&mut frame)? {
            ReceiveStatus::Again => Ok(None),
            ReceiveStatus::Frame => self.frame_to_gpu(&frame).map(Some),
        }
    }

    fn receive_frame(&mut self, frame: &mut AvFrame) -> Result<ReceiveStatus> {
        let result = unsafe { sys::avcodec_receive_frame(self.context, frame.as_mut_ptr()) };
        if result == sys::AVERROR(libc::EAGAIN) || result == sys::AVERROR_EOF {
            return Ok(ReceiveStatus::Again);
        }
        if result < 0 {
            return Err(ffi::error_from_code("avcodec_receive_frame", result)
                .with_codec(self.codec)
                .with_stream_index(self.stream_index));
        }
        Ok(ReceiveStatus::Frame)
    }

    fn frame_to_rgba(&mut self, frame: &AvFrame) -> Result<CpuVideoFrame> {
        let width = frame.width();
        let height = frame.height();
        let stride = width as usize * 4;
        let mut data = vec![0; stride.saturating_mul(height as usize)];
        let src_format = match frame.format() {
            AV_PIX_FMT_CUDA | AV_PIX_FMT_VULKAN | AV_PIX_FMT_VIDEOTOOLBOX => {
                let mut cpu_frame = AvFrame::new()?;
                unsafe {
                    ffi::check(
                        sys::av_hwframe_transfer_data(cpu_frame.as_mut_ptr(), frame.as_ptr(), 0),
                        "av_hwframe_transfer_data",
                    )?;
                }
                return self.frame_to_rgba(&cpu_frame);
            }
            other => other,
        };

        if self.scaler.is_null() {
            self.scaler = unsafe {
                sys::sws_getContext(
                    width as i32,
                    height as i32,
                    src_format,
                    width as i32,
                    height as i32,
                    AV_PIX_FMT_RGBA,
                    SWS_BILINEAR as i32,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null(),
                )
            };
            if self.scaler.is_null() {
                return Err(FfmpegError::new(
                    "sws_getContext",
                    "failed to create RGBA conversion context",
                ));
            }
        }

        let mut dst_data = [
            data.as_mut_ptr(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
        ];
        let mut dst_stride = [stride as i32, 0, 0, 0];
        unsafe {
            sys::sws_scale(
                self.scaler,
                (*frame.as_ptr()).data.as_ptr() as *const *const u8,
                (*frame.as_ptr()).linesize.as_ptr(),
                0,
                height as i32,
                dst_data.as_mut_ptr(),
                dst_stride.as_mut_ptr(),
            );
        }

        Ok(CpuVideoFrame {
            width,
            height,
            stride,
            pixel_format: PixelFormat::Rgba8,
            pts: frame.pts(),
            data,
        })
    }

    fn frame_to_gpu(&self, frame: &AvFrame) -> Result<GpuVideoFrame> {
        match (self.mode, frame.format()) {
            #[cfg(feature = "cuda")]
            (DecodeMode::Gpu(GpuBackend::Cuda), AV_PIX_FMT_CUDA) => {
                let _ = frame;
                Err(FfmpegError::new(
                    "receive_gpu_frame",
                    "CUDA decode produced AV_PIX_FMT_CUDA, but CUDA frame wrapping is not implemented yet",
                )
                .with_backend(GpuBackend::Cuda))
            }
            #[cfg(not(feature = "cuda"))]
            (DecodeMode::Gpu(GpuBackend::Cuda), AV_PIX_FMT_CUDA) => Err(FfmpegError::new(
                "receive_gpu_frame",
                "crate was built without the cuda feature",
            )
            .with_backend(GpuBackend::Cuda)),
            #[cfg(feature = "metal")]
            (DecodeMode::Gpu(GpuBackend::Metal), AV_PIX_FMT_VIDEOTOOLBOX) => {
                self.frame_to_metal(frame)
            }
            #[cfg(not(feature = "metal"))]
            (DecodeMode::Gpu(GpuBackend::Metal), AV_PIX_FMT_VIDEOTOOLBOX) => Err(FfmpegError::new(
                "receive_gpu_frame",
                "crate was built without the metal feature",
            )
            .with_backend(GpuBackend::Metal)),
            #[cfg(feature = "vulkan")]
            (DecodeMode::Gpu(GpuBackend::Vulkan), AV_PIX_FMT_VULKAN) => {
                let _ = frame;
                Err(FfmpegError::new(
                    "receive_gpu_frame",
                    "ffmpeg-sys-next does not expose AVVkFrame, so decoded Vulkan image handles cannot be exported safely yet",
                )
                .with_backend(GpuBackend::Vulkan))
            }
            #[cfg(not(feature = "vulkan"))]
            (DecodeMode::Gpu(GpuBackend::Vulkan), AV_PIX_FMT_VULKAN) => Err(FfmpegError::new(
                "receive_gpu_frame",
                "crate was built without the vulkan feature",
            )
            .with_backend(GpuBackend::Vulkan)),
            _ => Err(FfmpegError::new(
                "receive_gpu_frame",
                "decoder did not produce a hardware texture frame",
            )),
        }
    }

    #[cfg(feature = "metal")]
    fn frame_to_metal(&self, frame: &AvFrame) -> Result<GpuVideoFrame> {
        let pixel_buffer = NonNull::new(frame.data(3).cast()).ok_or_else(|| {
            FfmpegError::new(
                "receive_gpu_frame",
                "VideoToolbox frame did not contain a CVPixelBuffer",
            )
            .with_backend(GpuBackend::Metal)
        })?;
        Ok(GpuVideoFrame::Metal(unsafe {
            crate::gpu::MetalDecodedFrame::retain_from_video_toolbox_frame(
                pixel_buffer,
                frame.pts(),
            )
        }))
    }
}

unsafe extern "C" fn force_metal_pixel_format(
    _context: *mut sys::AVCodecContext,
    formats: *const sys::AVPixelFormat,
) -> sys::AVPixelFormat {
    force_pixel_format(formats, AV_PIX_FMT_VIDEOTOOLBOX)
}

unsafe extern "C" fn force_vulkan_pixel_format(
    _context: *mut sys::AVCodecContext,
    formats: *const sys::AVPixelFormat,
) -> sys::AVPixelFormat {
    force_pixel_format(formats, AV_PIX_FMT_VULKAN)
}

unsafe extern "C" fn force_cuda_pixel_format(
    _context: *mut sys::AVCodecContext,
    formats: *const sys::AVPixelFormat,
) -> sys::AVPixelFormat {
    force_pixel_format(formats, AV_PIX_FMT_CUDA)
}

fn force_pixel_format(
    formats: *const sys::AVPixelFormat,
    desired: sys::AVPixelFormat,
) -> sys::AVPixelFormat {
    let mut current = formats;
    while unsafe { *current } != AV_PIX_FMT_NONE {
        let format = unsafe { *current };
        if format == desired {
            return format;
        }
        current = unsafe { current.add(1) };
    }
    AV_PIX_FMT_NONE
}

impl Drop for VideoDecoder {
    fn drop(&mut self) {
        unsafe {
            if !self.scaler.is_null() {
                sys::sws_freeContext(self.scaler);
            }
            sys::avcodec_free_context(&mut self.context);
        }
    }
}

enum ReceiveStatus {
    Again,
    Frame,
}

struct HwDeviceContext {
    ptr: *mut sys::AVBufferRef,
}

impl HwDeviceContext {
    fn create(backend: GpuBackend) -> Result<Self> {
        let device_type = hw_device_type(backend);
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
}

fn hw_device_type(backend: GpuBackend) -> sys::AVHWDeviceType {
    match backend {
        GpuBackend::Metal => AV_HWDEVICE_TYPE_VIDEOTOOLBOX,
        GpuBackend::Vulkan => AV_HWDEVICE_TYPE_VULKAN,
        GpuBackend::Cuda => AV_HWDEVICE_TYPE_CUDA,
    }
}

fn hw_pixel_format(backend: GpuBackend) -> sys::AVPixelFormat {
    match backend {
        GpuBackend::Metal => AV_PIX_FMT_VIDEOTOOLBOX,
        GpuBackend::Vulkan => AV_PIX_FMT_VULKAN,
        GpuBackend::Cuda => AV_PIX_FMT_CUDA,
    }
}

impl Drop for HwDeviceContext {
    fn drop(&mut self) {
        unsafe { sys::av_buffer_unref(&mut self.ptr) };
    }
}

fn find_decoder(codec_id: sys::AVCodecID) -> Result<*const sys::AVCodec> {
    let decoder = unsafe { sys::avcodec_find_decoder(codec_id) };
    if decoder.is_null() {
        Err(FfmpegError::new(
            "avcodec_find_decoder",
            format!(
                "no decoder found for {}",
                crate::format::codec_name(codec_id)
            ),
        ))
    } else {
        Ok(decoder)
    }
}

fn ensure_hardware_decoder(
    decoder: *const sys::AVCodec,
    codec: VideoCodec,
    backend: GpuBackend,
) -> Result<()> {
    let device_type = hw_device_type(backend);
    let pixel_format = hw_pixel_format(backend);
    let mut index = 0;
    loop {
        let config = unsafe { sys::avcodec_get_hw_config(decoder, index) };
        if config.is_null() {
            break;
        }
        let supports_device_context = unsafe {
            ((*config).methods & sys::AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX as i32) != 0
        };
        let matches_backend =
            unsafe { (*config).device_type == device_type && (*config).pix_fmt == pixel_format };
        if supports_device_context && matches_backend {
            return Ok(());
        }
        index += 1;
    }

    Err(FfmpegError::new(
        "VideoDecoder::open",
        format!("{backend:?} hardware decode is unavailable for {codec:?}"),
    )
    .with_backend(backend)
    .with_codec(codec))
}
