use std::ptr;

#[cfg(feature = "metal")]
use std::ptr::NonNull;

use crate::{
    FfmpegError, Result,
    ffi::{self, AvFrame, SWS_BILINEAR, sys},
    format::{InputContext, Packet, Rational},
    gpu::{GpuBackend, GpuVideoFrame},
};
use sys::{
    AVCodecID::{
        AV_CODEC_ID_AV1, AV_CODEC_ID_H264, AV_CODEC_ID_HEVC, AV_CODEC_ID_NONE, AV_CODEC_ID_PRORES,
        AV_CODEC_ID_VP9,
    },
    AVColorRange::AVCOL_RANGE_JPEG,
    AVColorSpace::{
        AVCOL_SPC_BT470BG, AVCOL_SPC_BT709, AVCOL_SPC_BT2020_CL, AVCOL_SPC_BT2020_NCL,
        AVCOL_SPC_FCC, AVCOL_SPC_SMPTE170M, AVCOL_SPC_SMPTE240M,
    },
    AVHWDeviceType::{
        AV_HWDEVICE_TYPE_CUDA, AV_HWDEVICE_TYPE_VIDEOTOOLBOX, AV_HWDEVICE_TYPE_VULKAN,
    },
    AVPixelFormat::{
        AV_PIX_FMT_BGRA, AV_PIX_FMT_CUDA, AV_PIX_FMT_NONE, AV_PIX_FMT_NV12, AV_PIX_FMT_P010BE,
        AV_PIX_FMT_P010LE, AV_PIX_FMT_RGBA, AV_PIX_FMT_VIDEOTOOLBOX, AV_PIX_FMT_VULKAN,
        AV_PIX_FMT_YUVJ411P, AV_PIX_FMT_YUVJ420P, AV_PIX_FMT_YUVJ422P, AV_PIX_FMT_YUVJ440P,
        AV_PIX_FMT_YUVJ444P,
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
    rotation: u32,
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
            rotation: crate::format::display_rotation(parameters),
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
            if matches!(config.mode, DecodeMode::Gpu(_)) {
                (*decoder.context).thread_count = 1;
                (*decoder.context).thread_type = 0;
            } else {
                (*decoder.context).thread_count = 0;
                (*decoder.context).thread_type = sys::FF_THREAD_FRAME | sys::FF_THREAD_SLICE;
            }
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
        self.receive_rgba_frame()
    }

    /// Receives the next decoded frame as RGBA8.
    ///
    /// CPU decoders convert their native software frame with swscale. Hardware decoders first
    /// transfer the decoded frame into software memory, then use the same RGBA8 conversion path.
    /// This keeps callers independent from FFmpeg's platform-specific GPU frame handles while still
    /// allowing decode itself to use VideoToolbox/NVDEC/Vulkan when available. Frames come out in
    /// display orientation, with the stream's rotation applied.
    pub fn receive_rgba_frame(&mut self) -> Result<Option<CpuVideoFrame>> {
        let mut frame = AvFrame::new()?;
        match self.receive_frame(&mut frame)? {
            ReceiveStatus::Again => Ok(None),
            ReceiveStatus::Frame => self.frame_to_rgba(&frame).map(Some),
        }
    }

    /// Receives the next decoded frame as a GPU texture, in coded orientation.
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
            ReceiveStatus::Frame => self.frame_to_gpu(frame).map(Some),
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
        if matches!(
            frame.format(),
            AV_PIX_FMT_CUDA | AV_PIX_FMT_VULKAN | AV_PIX_FMT_VIDEOTOOLBOX
        ) {
            let mut cpu_frame = AvFrame::new()?;
            unsafe {
                ffi::check(
                    sys::av_hwframe_transfer_data(cpu_frame.as_mut_ptr(), frame.as_ptr(), 0),
                    "av_hwframe_transfer_data",
                )?;
                ffi::check(
                    sys::av_frame_copy_props(cpu_frame.as_mut_ptr(), frame.as_ptr()),
                    "av_frame_copy_props",
                )?;
            }
            return self.frame_to_rgba(&cpu_frame);
        }

        let width = frame.width();
        let height = frame.height();
        if width == 0 || height == 0 {
            return Err(FfmpegError::new(
                "receive_rgba_frame",
                "decoded frame has zero width or height",
            ));
        }
        let (Ok(av_width), Ok(av_height)) = (i32::try_from(width), i32::try_from(height)) else {
            return Err(FfmpegError::new(
                "receive_rgba_frame",
                "decoded frame dimensions exceed FFmpeg's supported integer range",
            ));
        };
        let stride = (width as usize).checked_mul(4).ok_or_else(|| {
            FfmpegError::new("receive_rgba_frame", "decoded frame row is too large")
        })?;
        // swscale's x86 YUV-to-RGBA kernels convert whole groups of pixels and drop the tail of a row
        // that has no padding, so it converts into rows padded to 64 bytes and packs them afterwards.
        let padded_stride = stride.div_ceil(64) * 64;
        let av_stride = i32::try_from(padded_stride).map_err(|_| {
            FfmpegError::new("receive_rgba_frame", "decoded frame row is too large")
        })?;
        let len = padded_stride
            .checked_mul(height as usize)
            .ok_or_else(|| FfmpegError::new("receive_rgba_frame", "decoded frame is too large"))?;
        let mut data = vec![0; len];

        // Rebuilds the converter whenever the frame's geometry or pixel format changes.
        self.scaler = unsafe {
            sys::sws_getCachedContext(
                self.scaler,
                av_width,
                av_height,
                frame.format(),
                av_width,
                av_height,
                AV_PIX_FMT_RGBA,
                SWS_BILINEAR,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null(),
            )
        };
        if self.scaler.is_null() {
            return Err(FfmpegError::new(
                "sws_getCachedContext",
                "failed to create RGBA conversion context",
            ));
        }
        // Applied per frame because recreating the converter resets these, and tags can change
        // between frames. swscale returns early when nothing changed.
        set_source_colorspace(self.scaler, frame)?;

        let mut dst_data = [
            data.as_mut_ptr(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
        ];
        let mut dst_stride = [av_stride, 0, 0, 0];
        unsafe {
            ffi::check(
                sys::sws_scale(
                    self.scaler,
                    (*frame.as_ptr()).data.as_ptr() as *const *const u8,
                    (*frame.as_ptr()).linesize.as_ptr(),
                    0,
                    av_height,
                    dst_data.as_mut_ptr(),
                    dst_stride.as_mut_ptr(),
                ),
                "sws_scale",
            )?;
        }

        if padded_stride != stride {
            for row in 1..height as usize {
                data.copy_within(
                    row * padded_stride..row * padded_stride + stride,
                    row * stride,
                );
            }
            data.truncate(stride * height as usize);
        }

        let (width, height, data) = rotate_rgba(width, height, data, self.rotation);
        Ok(CpuVideoFrame {
            width,
            height,
            stride: width as usize * 4,
            pixel_format: PixelFormat::Rgba8,
            pts: frame.pts(),
            data,
        })
    }

    fn frame_to_gpu(&self, frame: AvFrame) -> Result<GpuVideoFrame> {
        match (self.mode, frame.format()) {
            #[cfg(feature = "cuda")]
            (DecodeMode::Gpu(GpuBackend::Cuda), AV_PIX_FMT_CUDA) => self.frame_to_cuda(frame),
            #[cfg(not(feature = "cuda"))]
            (DecodeMode::Gpu(GpuBackend::Cuda), AV_PIX_FMT_CUDA) => Err(FfmpegError::new(
                "receive_gpu_frame",
                "crate was built without the cuda feature",
            )
            .with_backend(GpuBackend::Cuda)),
            #[cfg(feature = "metal")]
            (DecodeMode::Gpu(GpuBackend::Metal), AV_PIX_FMT_VIDEOTOOLBOX) => {
                self.frame_to_metal(&frame)
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

    #[cfg(feature = "cuda")]
    fn frame_to_cuda(&self, frame: AvFrame) -> Result<GpuVideoFrame> {
        let device_ptr = frame.data(0) as u64;
        let pitch = frame.line_size(0);
        let width = frame.width();
        let height = frame.height();
        let pts = frame.pts();
        if device_ptr == 0 || pitch <= 0 {
            return Err(FfmpegError::new(
                "receive_gpu_frame",
                "CUDA decoded frame did not contain a valid device pointer and pitch",
            )
            .with_backend(GpuBackend::Cuda));
        }
        let pixel_format = frame
            .hw_sw_format()
            .map(PixelFormat::from_av_pixel_format)
            .unwrap_or(PixelFormat::Unknown);
        // The renderer's CUDA converter only reads NV12. Failing here moves the source to the CPU.
        if pixel_format != PixelFormat::Nv12 {
            return Err(FfmpegError::new(
                "receive_gpu_frame",
                format!("CUDA decoded {pixel_format:?} frames, only NV12 converts to RGBA"),
            )
            .with_backend(GpuBackend::Cuda));
        }
        Ok(GpuVideoFrame::Cuda(
            crate::gpu::CudaDecodedFrame::from_av_frame(
                frame,
                device_ptr,
                width,
                height,
                pitch as u64,
                pixel_format,
                pts,
            ),
        ))
    }
}

/// Tells swscale how to read the frame's YUV. Its default is limited-range BT.601. The destination is
/// always full-range RGBA.
fn set_source_colorspace(scaler: *mut sys::SwsContext, frame: &AvFrame) -> Result<()> {
    const DEFAULT_BRIGHTNESS: i32 = 0;
    const DEFAULT_CONTRAST: i32 = 1 << 16;
    const DEFAULT_SATURATION: i32 = 1 << 16;

    let full_range = frame.color_range() == AVCOL_RANGE_JPEG
        || matches!(
            frame.format(),
            AV_PIX_FMT_YUVJ411P
                | AV_PIX_FMT_YUVJ420P
                | AV_PIX_FMT_YUVJ422P
                | AV_PIX_FMT_YUVJ440P
                | AV_PIX_FMT_YUVJ444P
        );
    unsafe {
        let coefficients = sys::sws_getCoefficients(source_matrix(frame));
        ffi::check(
            sys::sws_setColorspaceDetails(
                scaler,
                coefficients,
                i32::from(full_range),
                coefficients,
                1,
                DEFAULT_BRIGHTNESS,
                DEFAULT_CONTRAST,
                DEFAULT_SATURATION,
            ),
            "sws_setColorspaceDetails",
        )
    }
}

/// Picks swscale's `SWS_CS_*` index, whose values match `AVColorSpace`.
fn source_matrix(frame: &AvFrame) -> i32 {
    match frame.colorspace() {
        colorspace @ (AVCOL_SPC_BT709 | AVCOL_SPC_FCC | AVCOL_SPC_BT470BG | AVCOL_SPC_SMPTE170M
        | AVCOL_SPC_SMPTE240M | AVCOL_SPC_BT2020_NCL) => colorspace as i32,
        AVCOL_SPC_BT2020_CL => AVCOL_SPC_BT2020_NCL as i32,
        // Heuristic for untagged video, not a fact about the source: HD is usually BT.709 and SD
        // usually BT.601.
        _ if frame.height() >= 720 => AVCOL_SPC_BT709 as i32,
        _ => AVCOL_SPC_BT470BG as i32,
    }
}

/// Rotates tightly packed RGBA clockwise, returning the new width, height and pixels.
fn rotate_rgba(width: u32, height: u32, data: Vec<u8>, rotation: u32) -> (u32, u32, Vec<u8>) {
    if rotation == 0 {
        return (width, height, data);
    }
    let (w, h) = (width as usize, height as usize);
    let (out_width, out_height) = if rotation == 180 { (w, h) } else { (h, w) };
    let mut out = vec![0; data.len()];
    for (index, pixel) in data.as_chunks::<4>().0.iter().enumerate() {
        let (x, y) = (index % w, index / w);
        let (out_x, out_y) = match rotation {
            90 => (h - 1 - y, x),
            180 => (w - 1 - x, h - 1 - y),
            _ => (y, w - 1 - x),
        };
        out[(out_y * out_width + out_x) * 4..][..4].copy_from_slice(pixel);
    }
    (out_width as u32, out_height as u32, out)
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
