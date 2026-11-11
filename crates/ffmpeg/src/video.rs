use std::cell::UnsafeCell;
use std::collections::HashSet;
use std::ptr;

#[cfg(feature = "metal")]
use std::ptr::NonNull;

use crate::{
    FfmpegError, Result,
    encode::hw::AvBufferRef,
    ffi::{self, AvFrame, SWS_BILINEAR, sys},
    format::{InputContext, Packet, Rational},
    gpu::{GpuBackend, GpuVideoFrame},
};
use sys::{
    AVCodecID::{
        AV_CODEC_ID_AV1, AV_CODEC_ID_H264, AV_CODEC_ID_HEVC, AV_CODEC_ID_PRORES, AV_CODEC_ID_VP8,
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
        AV_PIX_FMT_YUV420P, AV_PIX_FMT_YUV420P10LE, AV_PIX_FMT_YUVJ411P, AV_PIX_FMT_YUVJ420P,
        AV_PIX_FMT_YUVJ422P, AV_PIX_FMT_YUVJ440P, AV_PIX_FMT_YUVJ444P,
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
    Rgba8,
    Bgra8,
    Nv12,
    P010,
    /// Planar 8-bit YUV 4:2:0.
    Yuv420p,
    /// Planar 10-bit YUV 4:2:0, each sample two little-endian bytes holding its value in the low
    /// bits.
    Yuv420p10,
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

/// The `AVColorSpace` of a frame, among the values a YUV to RGB matrix follows from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMatrix {
    Bt709,
    Bt470bg,
    Smpte170m,
    Fcc,
    Smpte240m,
    Bt2020Ncl,
    Bt2020Cl,
    /// Unspecified, or a value that isn't a YUV matrix.
    Other,
}

/// The `AVColorPrimaries` of a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorPrimaries {
    Bt709,
    Bt470bg,
    Smpte170m,
    Smpte240m,
    Bt2020,
    /// DCI-P3, with the DCI white point.
    Smpte431,
    /// Display P3: the P3 primaries with a D65 white point.
    Smpte432,
    Unspecified,
    /// Any other tag, by FFmpeg's name for it.
    Other(&'static str),
}

/// The `AVColorTransferCharacteristic` of a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorTransfer {
    Bt709,
    Smpte170m,
    Bt2020_10,
    Bt2020_12,
    /// IEC 61966-2-1, the sRGB curve.
    Srgb,
    /// SMPTE ST 2084, the PQ curve.
    Smpte2084,
    /// ARIB STD-B67, the HLG curve.
    AribStdB67,
    Unspecified,
    /// Any other tag, by FFmpeg's name for it.
    Other(&'static str),
}

/// Where chroma samples sit relative to luma, the `AVChromaLocation` of a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChromaLocation {
    Unspecified,
    Left,
    Center,
    TopLeft,
    Top,
    BottomLeft,
    Bottom,
}

/// The colour tags FFmpeg read for a decoded frame or a stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColorTags {
    pub matrix: ColorMatrix,
    /// `AVCOL_RANGE_JPEG`, or a JPEG pixel format: samples span the whole code range.
    pub full_range: bool,
    pub primaries: ColorPrimaries,
    pub transfer: ColorTransfer,
    pub chroma_location: ChromaLocation,
}

impl ColorTags {
    /// Tags with only a matrix and range set, and the rest unspecified.
    pub fn new(matrix: ColorMatrix, full_range: bool) -> Self {
        Self {
            matrix,
            full_range,
            primaries: ColorPrimaries::Unspecified,
            transfer: ColorTransfer::Unspecified,
            chroma_location: ChromaLocation::Unspecified,
        }
    }

    pub(crate) fn from_av(
        matrix: sys::AVColorSpace,
        full_range: bool,
        primaries: sys::AVColorPrimaries,
        transfer: sys::AVColorTransferCharacteristic,
        chroma_location: sys::AVChromaLocation,
    ) -> Self {
        use sys::{
            AVChromaLocation as Loc, AVColorPrimaries as Pri, AVColorTransferCharacteristic as Trc,
        };
        let matrix = match matrix {
            AVCOL_SPC_BT709 => ColorMatrix::Bt709,
            AVCOL_SPC_BT470BG => ColorMatrix::Bt470bg,
            AVCOL_SPC_SMPTE170M => ColorMatrix::Smpte170m,
            AVCOL_SPC_FCC => ColorMatrix::Fcc,
            AVCOL_SPC_SMPTE240M => ColorMatrix::Smpte240m,
            AVCOL_SPC_BT2020_NCL => ColorMatrix::Bt2020Ncl,
            AVCOL_SPC_BT2020_CL => ColorMatrix::Bt2020Cl,
            _ => ColorMatrix::Other,
        };
        let primaries = match primaries {
            Pri::AVCOL_PRI_BT709 => ColorPrimaries::Bt709,
            Pri::AVCOL_PRI_BT470BG => ColorPrimaries::Bt470bg,
            Pri::AVCOL_PRI_SMPTE170M => ColorPrimaries::Smpte170m,
            Pri::AVCOL_PRI_SMPTE240M => ColorPrimaries::Smpte240m,
            Pri::AVCOL_PRI_BT2020 => ColorPrimaries::Bt2020,
            Pri::AVCOL_PRI_SMPTE431 => ColorPrimaries::Smpte431,
            Pri::AVCOL_PRI_SMPTE432 => ColorPrimaries::Smpte432,
            Pri::AVCOL_PRI_UNSPECIFIED | Pri::AVCOL_PRI_RESERVED0 | Pri::AVCOL_PRI_RESERVED => {
                ColorPrimaries::Unspecified
            }
            other => {
                ColorPrimaries::Other(static_name(unsafe { sys::av_color_primaries_name(other) }))
            }
        };
        let transfer = match transfer {
            Trc::AVCOL_TRC_BT709 => ColorTransfer::Bt709,
            Trc::AVCOL_TRC_SMPTE170M => ColorTransfer::Smpte170m,
            Trc::AVCOL_TRC_BT2020_10 => ColorTransfer::Bt2020_10,
            Trc::AVCOL_TRC_BT2020_12 => ColorTransfer::Bt2020_12,
            Trc::AVCOL_TRC_IEC61966_2_1 => ColorTransfer::Srgb,
            Trc::AVCOL_TRC_SMPTE2084 => ColorTransfer::Smpte2084,
            Trc::AVCOL_TRC_ARIB_STD_B67 => ColorTransfer::AribStdB67,
            Trc::AVCOL_TRC_UNSPECIFIED | Trc::AVCOL_TRC_RESERVED0 | Trc::AVCOL_TRC_RESERVED => {
                ColorTransfer::Unspecified
            }
            other => {
                ColorTransfer::Other(static_name(unsafe { sys::av_color_transfer_name(other) }))
            }
        };
        let chroma_location = match chroma_location {
            Loc::AVCHROMA_LOC_LEFT => ChromaLocation::Left,
            Loc::AVCHROMA_LOC_CENTER => ChromaLocation::Center,
            Loc::AVCHROMA_LOC_TOPLEFT => ChromaLocation::TopLeft,
            Loc::AVCHROMA_LOC_TOP => ChromaLocation::Top,
            Loc::AVCHROMA_LOC_BOTTOMLEFT => ChromaLocation::BottomLeft,
            Loc::AVCHROMA_LOC_BOTTOM => ChromaLocation::Bottom,
            _ => ChromaLocation::Unspecified,
        };
        Self {
            matrix,
            full_range,
            primaries,
            transfer,
            chroma_location,
        }
    }
}

/// A name FFmpeg returns from its static tables, or `"unknown"` for none.
fn static_name(name: *const std::ffi::c_char) -> &'static str {
    if name.is_null() {
        return "unknown";
    }
    // SAFETY: FFmpeg's colour names are string literals in libavutil, which lives as long as the
    // process.
    unsafe { std::ffi::CStr::from_ptr(name) }
        .to_str()
        .unwrap_or("unknown")
}

/// A frame in CPU memory. RGBA rows are `stride` bytes apart. A YUV 4:2:0 frame (`Nv12`, `P010`,
/// `Yuv420p`, `Yuv420p10`) holds `height` luma rows `stride` bytes apart, then `height.div_ceil(2)`
/// chroma rows: interleaved Cb, Cr rows `stride` bytes apart, or for the planar formats the Cb
/// plane and then the Cr plane, rows `stride / 2` bytes apart.
#[derive(Debug, Clone)]
pub struct CpuVideoFrame {
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub pixel_format: PixelFormat,
    pub pts: Option<i64>,
    pub data: Vec<u8>,
}

/// A decoded video frame. Sources that store YUV come out as planes with the stream's colour tags, so
/// the renderer picks the colour conversion; everything else comes out as RGBA.
#[derive(Debug, Clone)]
pub enum DecodedVideoFrame {
    Rgba {
        frame: CpuVideoFrame,
        /// The pixels carry alpha that the renderer may still need to premultiply.
        has_alpha: bool,
    },
    Yuv(YuvVideoFrame),
}

impl DecodedVideoFrame {
    pub fn pts(&self) -> Option<i64> {
        match self {
            Self::Rgba { frame, .. } => frame.pts,
            Self::Yuv(frame) => frame.pts,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YuvSubsampling {
    /// Chroma at half width and half height.
    Yuv420,
    /// Chroma at half width.
    Yuv422,
    Yuv444,
}

/// A decoded frame as YUV planes, whose colour is exactly what the source stored.
///
/// `data` holds the planes back to back with no row padding: luma, then chroma, then alpha when
/// there is any. Samples are one byte at depth 8 and two little-endian bytes above it. Chroma is
/// either one plane of Cb, Cr pairs or a Cb plane followed by a Cr plane, and alpha is full size.
#[derive(Debug, Clone)]
pub struct YuvVideoFrame {
    /// The coded size, before `rotation`.
    pub width: u32,
    pub height: u32,
    pub pts: Option<i64>,
    pub subsampling: YuvSubsampling,
    /// Significant bits per sample.
    pub depth: u8,
    /// A two-byte sample's value sits in its high bits, as in P010.
    pub msb_aligned: bool,
    pub interleaved_chroma: bool,
    pub has_alpha: bool,
    pub color: ColorTags,
    /// Clockwise degrees the frame turns to show upright: 0, 90, 180 or 270.
    pub rotation: u32,
    pub data: Vec<u8>,
}

impl YuvVideoFrame {
    /// The Y, Cb and Cr code values at luma position `(x, y)` in coded orientation, with chroma from
    /// the block covering that pixel. Values span `depth` bits whatever the sample's alignment.
    ///
    /// # Panics
    ///
    /// If `(x, y)` is outside the frame.
    pub fn sample(&self, x: u32, y: u32) -> [u16; 3] {
        assert!(
            x < self.width && y < self.height,
            "sample outside the frame"
        );
        let (x, y) = (x as usize, y as usize);
        let (width, height) = (self.width as usize, self.height as usize);
        let bytes = if self.depth > 8 { 2 } else { 1 };
        let (shift_x, shift_y) = match self.subsampling {
            YuvSubsampling::Yuv420 => (1, 1),
            YuvSubsampling::Yuv422 => (1, 0),
            YuvSubsampling::Yuv444 => (0, 0),
        };
        let chroma_width = width.div_ceil(1 << shift_x);
        let chroma_size = chroma_width * height.div_ceil(1 << shift_y);
        let value = |offset: usize| {
            let raw = if bytes == 1 {
                u16::from(self.data[offset])
            } else {
                u16::from_le_bytes([self.data[offset], self.data[offset + 1]])
            };
            if self.msb_aligned {
                raw >> (16 - self.depth)
            } else {
                raw
            }
        };
        let chroma = (y >> shift_y) * chroma_width + (x >> shift_x);
        let chroma_start = width * height * bytes;
        let (cb, cr) = if self.interleaved_chroma {
            let offset = chroma_start + chroma * 2 * bytes;
            (offset, offset + bytes)
        } else {
            let offset = chroma_start + chroma * bytes;
            (offset, offset + chroma_size * bytes)
        };
        [value((y * width + x) * bytes), value(cb), value(cr)]
    }
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
    #[cfg(feature = "vulkan")]
    vulkan_device: Option<crate::gpu::VulkanDevice>,
    /// Reached through the codec context's `opaque` by [`force_vulkan_pixel_format`].
    vulkan_frames: Box<UnsafeCell<Option<VulkanFrames>>>,
    /// Timestamps of the packets an edit list discards, for decoders where `drops_by_timestamp`.
    discarded_pts: HashSet<i64>,
    drops_alpha: bool,
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
}

impl PixelFormat {
    pub(crate) fn from_av_pixel_format(format: sys::AVPixelFormat) -> Self {
        match format {
            AV_PIX_FMT_RGBA => Self::Rgba8,
            AV_PIX_FMT_BGRA => Self::Bgra8,
            AV_PIX_FMT_NV12 => Self::Nv12,
            AV_PIX_FMT_P010LE | AV_PIX_FMT_P010BE => Self::P010,
            AV_PIX_FMT_YUV420P => Self::Yuv420p,
            AV_PIX_FMT_YUV420P10LE => Self::Yuv420p10,
            AV_PIX_FMT_VULKAN => Self::Vulkan,
            AV_PIX_FMT_VIDEOTOOLBOX => Self::Metal,
            _ => Self::Unknown,
        }
    }

    pub(crate) fn to_av_pixel_format(self) -> sys::AVPixelFormat {
        match self {
            Self::Rgba8 => AV_PIX_FMT_RGBA,
            Self::Bgra8 => AV_PIX_FMT_BGRA,
            Self::Nv12 => AV_PIX_FMT_NV12,
            Self::P010 => AV_PIX_FMT_P010LE,
            Self::Yuv420p => AV_PIX_FMT_YUV420P,
            Self::Yuv420p10 => AV_PIX_FMT_YUV420P10LE,
            Self::Vulkan => AV_PIX_FMT_VULKAN,
            Self::Metal => AV_PIX_FMT_VIDEOTOOLBOX,
            Self::Unknown => AV_PIX_FMT_NONE,
        }
    }
}

impl VideoDecoder {
    pub fn open(input: &InputContext, config: VideoDecoderConfig) -> Result<Self> {
        let hw_device = match config.mode {
            DecodeMode::Cpu => None,
            DecodeMode::Gpu(GpuBackend::Cuda) => {
                return Err(FfmpegError::new(
                    "VideoDecoder::open",
                    "CUDA hardware decode is not supported",
                )
                .with_backend(GpuBackend::Cuda));
            }
            DecodeMode::Gpu(backend) => Some(HwDeviceContext::create(backend)?),
        };
        Self::open_with_device(input, config, hw_device)
    }

    /// Opens a decoder whose frames [`receive_gpu_frame`](Self::receive_gpu_frame) returns on
    /// `device`, for a renderer on that device to read. Fails for FFmpeg older than 8.0 and codecs
    /// other than H.264, HEVC and AV1. Whether the device decodes the stream's profile is only known
    /// once FFmpeg sets up decoding for the first frame, which fails if it doesn't.
    #[cfg(feature = "vulkan")]
    pub fn open_on_vulkan_device(
        input: &InputContext,
        stream_index: usize,
        device: &crate::gpu::VulkanDevice,
    ) -> Result<Self> {
        let parameters = input.stream_parameters(stream_index)?;
        let codec_id = unsafe { (*parameters).codec_id };
        ensure_vulkan_texture_decode(
            VideoCodec::from_av_codec_id(codec_id),
            unsafe { sys::avcodec_version() } >> 16,
        )?;
        let hw_device = device.hw_device()?;
        let mut decoder = Self::open_with_device(
            input,
            VideoDecoderConfig {
                stream_index,
                mode: DecodeMode::Gpu(GpuBackend::Vulkan),
            },
            Some(HwDeviceContext::from_ref(&hw_device)?),
        )?;
        decoder.vulkan_device = Some(device.clone());
        Ok(decoder)
    }

    fn open_with_device(
        input: &InputContext,
        config: VideoDecoderConfig,
        hw_device: Option<HwDeviceContext>,
    ) -> Result<Self> {
        let parameters = input.stream_parameters(config.stream_index)?;
        let codec_id = unsafe { (*parameters).codec_id };
        let stable_codec = VideoCodec::from_av_codec_id(codec_id);
        let alpha_layer = input.has_alpha_layer(config.stream_index);
        let alpha_decoder = if alpha_layer && config.mode == DecodeMode::Cpu {
            find_alpha_layer_decoder(codec_id)
        } else {
            None
        };
        let codec = match (alpha_decoder, config.mode) {
            (Some(decoder), _) => decoder,
            (None, DecodeMode::Cpu) => find_decoder(codec_id)?,
            (None, DecodeMode::Gpu(backend)) => {
                find_hardware_decoder(codec_id, stable_codec, backend)?
            }
        };
        let context = unsafe { sys::avcodec_alloc_context3(codec) };
        if context.is_null() {
            return Err(FfmpegError::new(
                "avcodec_alloc_context3",
                "failed to allocate video decoder context",
            ));
        }

        let decoder = Self {
            stream_index: config.stream_index,
            mode: config.mode,
            codec: stable_codec,
            time_base: input.stream_time_base(config.stream_index)?.into(),
            rotation: crate::format::display_rotation(parameters),
            context,
            scaler: ptr::null_mut(),
            hw_device,
            #[cfg(feature = "vulkan")]
            vulkan_device: None,
            vulkan_frames: Box::default(),
            discarded_pts: HashSet::new(),
            drops_alpha: alpha_layer && alpha_decoder.is_none(),
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

        if let (DecodeMode::Gpu(backend), Some(hw_device)) = (config.mode, &decoder.hw_device) {
            unsafe {
                (*decoder.context).hw_device_ctx = sys::av_buffer_ref(hw_device.ptr);
                (*decoder.context).opaque =
                    ptr::from_ref(&*decoder.vulkan_frames).cast_mut().cast();
                (*decoder.context).get_format = match backend {
                    GpuBackend::Metal => Some(force_metal_pixel_format),
                    GpuBackend::Vulkan => Some(force_vulkan_pixel_format),
                    GpuBackend::Cuda => None,
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

    /// Whether the stream has an alpha layer this decoder ignores, so its frames come out opaque.
    pub fn drops_alpha(&self) -> bool {
        self.drops_alpha
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
        let undiscarded = if self.drops_by_timestamp() && packet.inner.is_discarded() {
            self.discarded_pts.extend(packet.pts());
            Some(packet.inner.clone_without_discard()?)
        } else {
            None
        };
        let packet = undiscarded.as_ref().unwrap_or(&packet.inner);
        unsafe {
            ffi::check(
                sys::avcodec_send_packet(self.context, packet.as_ptr()),
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

    /// Forgets all input and output, so the next frame received comes from the next packet sent.
    ///
    /// FFmpeg's HEVC decoder doesn't empty its output queue on a flush. An IDR frame releases every
    /// earlier frame into that queue at once, and only frame threads, which hardware decoders and
    /// single-CPU hosts don't use, move them somewhere the flush clears.
    pub fn flush(&mut self) -> Result<()> {
        unsafe { sys::avcodec_flush_buffers(self.context) };
        self.discarded_pts.clear();
        let mut frame = AvFrame::new()?;
        // These frames are being thrown away, so an error about them doesn't matter either.
        while matches!(self.receive_frame(&mut frame), Ok(ReceiveStatus::Frame)) {}
        Ok(())
    }

    /// FFmpeg's native AV1 decoder keeps a packet's discard flag on every frame that packet
    /// decodes, so a hidden frame from an edit list's preroll is dropped when a kept packet shows
    /// it. Hardware AV1 therefore decodes with the flag cleared and drops frames by timestamp.
    fn drops_by_timestamp(&self) -> bool {
        self.codec == VideoCodec::Av1 && matches!(self.mode, DecodeMode::Gpu(_))
    }

    /// Receives the next decoded frame in the form that keeps the source's colour: YUV sources as
    /// planes in coded orientation, with their colour tags and the stream's rotation, and the rest
    /// as RGBA8 in display orientation. Hardware decoders transfer the frame to software memory
    /// first.
    ///
    /// Formats [`YuvVideoFrame`] can't carry as they are come out as the nearest layout it can, by
    /// a swscale conversion between YUV formats that leaves the colour alone.
    pub fn receive_video_frame(&mut self) -> Result<Option<DecodedVideoFrame>> {
        let mut frame = AvFrame::new()?;
        match self.receive_frame(&mut frame)? {
            ReceiveStatus::Again => Ok(None),
            ReceiveStatus::Frame => {
                let frame = software_frame(frame)?;
                match yuv_source(frame.format()) {
                    Some(source) => self
                        .frame_to_yuv(&frame, source)
                        .map(DecodedVideoFrame::Yuv),
                    None => self
                        .frame_to_rgba(&frame)
                        .map(|rgba| DecodedVideoFrame::Rgba {
                            has_alpha: crate::format::pixel_format_has_alpha(frame.format()),
                            frame: rgba,
                        }),
                }
                .map(Some)
            }
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
        loop {
            let result = unsafe { sys::avcodec_receive_frame(self.context, frame.as_mut_ptr()) };
            if result == sys::AVERROR(libc::EAGAIN) || result == sys::AVERROR_EOF {
                return Ok(ReceiveStatus::Again);
            }
            if result < 0 {
                return Err(ffi::error_from_code("avcodec_receive_frame", result)
                    .with_codec(self.codec)
                    .with_stream_index(self.stream_index));
            }
            if !frame
                .pts()
                .is_some_and(|pts| self.discarded_pts.remove(&pts))
            {
                return Ok(ReceiveStatus::Frame);
            }
        }
    }

    /// Converts a frame that isn't YUV, as `yuv_source` decides, to RGBA8 in display orientation.
    fn frame_to_rgba(&mut self, frame: &AvFrame) -> Result<CpuVideoFrame> {
        let (width, height) = checked_size(frame, "receive_video_frame")?;
        let (av_width, av_height) = (width as i32, height as i32);
        let stride = (width as usize).checked_mul(4).ok_or_else(|| {
            FfmpegError::new("receive_video_frame", "decoded frame row is too large")
        })?;
        // swscale's x86 YUV-to-RGBA kernels convert whole groups of pixels and drop the tail of a row
        // that has no padding, so it converts into rows padded to 64 bytes and packs them afterwards.
        let padded_stride = stride.div_ceil(64) * 64;
        let av_stride = i32::try_from(padded_stride).map_err(|_| {
            FfmpegError::new("receive_video_frame", "decoded frame row is too large")
        })?;
        let len = padded_stride
            .checked_mul(height as usize)
            .ok_or_else(|| FfmpegError::new("receive_video_frame", "decoded frame is too large"))?;
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
        // Applied per frame because recreating the converter resets it, and the tag can change
        // between frames. swscale returns early when nothing changed.
        if is_gray(frame.format()) {
            set_gray_range(self.scaler, is_full_range(frame))?;
        }

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

    fn frame_to_yuv(&mut self, frame: &AvFrame, source: YuvSource) -> Result<YuvVideoFrame> {
        let (width, height) = checked_size(frame, "receive_video_frame")?;
        let reshaped;
        let (planes, shape) = match source {
            YuvSource::Native(shape) => (frame, shape),
            YuvSource::Reshape(target, shape) => {
                reshaped = self.reshape_yuv(frame, target, width, height)?;
                (&reshaped, shape)
            }
        };
        Ok(YuvVideoFrame {
            width,
            height,
            pts: frame.pts(),
            subsampling: shape.subsampling,
            depth: shape.depth,
            msb_aligned: shape.msb_aligned,
            interleaved_chroma: shape.interleaved,
            has_alpha: shape.alpha,
            color: color_tags(frame),
            rotation: self.rotation,
            data: pack_planes(planes, &shape, width, height)?,
        })
    }

    /// Converts between two YUV formats of the same size. swscale's default colour details leave
    /// the samples' range and matrix alone.
    fn reshape_yuv(
        &mut self,
        frame: &AvFrame,
        target: sys::AVPixelFormat,
        width: u32,
        height: u32,
    ) -> Result<AvFrame> {
        let (av_width, av_height) = (width as i32, height as i32);
        self.scaler = unsafe {
            sys::sws_getCachedContext(
                self.scaler,
                av_width,
                av_height,
                frame.format(),
                av_width,
                av_height,
                target,
                SWS_BILINEAR,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null(),
            )
        };
        if self.scaler.is_null() {
            return Err(FfmpegError::new(
                "sws_getCachedContext",
                "failed to create YUV conversion context",
            ));
        }
        let mut converted = AvFrame::with_buffers(target, av_width, av_height)?;
        unsafe {
            let output = converted.as_mut_ptr();
            ffi::check(
                sys::sws_scale(
                    self.scaler,
                    (*frame.as_ptr()).data.as_ptr() as *const *const u8,
                    (*frame.as_ptr()).linesize.as_ptr(),
                    0,
                    av_height,
                    (*output).data.as_ptr(),
                    (*output).linesize.as_ptr(),
                ),
                "sws_scale",
            )?;
        }
        Ok(converted)
    }

    fn frame_to_gpu(&self, frame: AvFrame) -> Result<GpuVideoFrame> {
        match (self.mode, frame.format()) {
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
            (DecodeMode::Gpu(GpuBackend::Vulkan), AV_PIX_FMT_VULKAN) => self.frame_to_vulkan(frame),
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
                color_tags(frame),
                self.rotation,
            )
        }))
    }

    #[cfg(feature = "vulkan")]
    fn frame_to_vulkan(&self, frame: AvFrame) -> Result<GpuVideoFrame> {
        let Some(device) = &self.vulkan_device else {
            return Err(FfmpegError::new(
                "receive_gpu_frame",
                "this decoder has its own Vulkan device; use open_on_vulkan_device to decode on a renderer's",
            )
            .with_backend(GpuBackend::Vulkan));
        };
        let format = frame.hw_sw_format().unwrap_or(AV_PIX_FMT_NONE);
        let (subsampling, depth) = two_plane_layout(format).ok_or_else(|| {
            FfmpegError::new(
                "receive_gpu_frame",
                format!("Vulkan decoded {format:?} frames, which aren't two-plane YCbCr"),
            )
            .with_backend(GpuBackend::Vulkan)
        })?;
        let color = color_tags(&frame);
        crate::gpu::VulkanDecodedFrame::new(
            frame,
            device.clone(),
            subsampling,
            depth,
            color,
            self.rotation,
        )
        .map(GpuVideoFrame::Vulkan)
    }
}

/// The frame itself, or for a hardware frame its contents transferred to software memory.
fn software_frame(frame: AvFrame) -> Result<AvFrame> {
    if !matches!(frame.format(), AV_PIX_FMT_VULKAN | AV_PIX_FMT_VIDEOTOOLBOX) {
        return Ok(frame);
    }
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
    Ok(cpu_frame)
}

/// The frame's size, if FFmpeg can address a frame that big.
fn checked_size(frame: &AvFrame, operation: &'static str) -> Result<(u32, u32)> {
    let (width, height) = (frame.width(), frame.height());
    if width == 0 || height == 0 {
        return Err(FfmpegError::new(
            operation,
            "decoded frame has zero width or height",
        ));
    }
    if i32::try_from(width).is_err() || i32::try_from(height).is_err() {
        return Err(FfmpegError::new(
            operation,
            "decoded frame dimensions exceed FFmpeg's supported integer range",
        ));
    }
    Ok((width, height))
}

/// How a pixel format's planes are laid out, for the formats [`YuvVideoFrame`] carries.
#[derive(Debug, Clone, Copy)]
struct YuvShape {
    subsampling: YuvSubsampling,
    depth: u8,
    msb_aligned: bool,
    interleaved: bool,
    alpha: bool,
}

enum YuvSource {
    /// The frame's planes are read as they are.
    Native(YuvShape),
    /// The frame converts to this YUV format first, which has this layout.
    Reshape(sys::AVPixelFormat, YuvShape),
}

/// How to take a frame of `format` as YUV, or `None` if it isn't a YUV format.
fn yuv_source(format: sys::AVPixelFormat) -> Option<YuvSource> {
    let descriptor = unsafe { sys::av_pix_fmt_desc_get(format).as_ref()? };
    let not_yuv = [
        sys::AV_PIX_FMT_FLAG_RGB,
        sys::AV_PIX_FMT_FLAG_PAL,
        sys::AV_PIX_FMT_FLAG_BITSTREAM,
        sys::AV_PIX_FMT_FLAG_HWACCEL,
        sys::AV_PIX_FMT_FLAG_BAYER,
    ]
    .into_iter()
    .fold(0, |flags, flag| flags | flag as u64);
    if descriptor.nb_components < 3
        || descriptor.flags & not_yuv != 0
        || matches!(
            format,
            sys::AVPixelFormat::AV_PIX_FMT_XYZ12LE | sys::AVPixelFormat::AV_PIX_FMT_XYZ12BE
        )
    {
        return None;
    }
    Some(match native_shape(descriptor) {
        Some(shape) => YuvSource::Native(shape),
        None => {
            let target = reshape_target(descriptor, is_full_range_format(format));
            let shape = native_shape(unsafe { sys::av_pix_fmt_desc_get(target).as_ref()? })?;
            YuvSource::Reshape(target, shape)
        }
    })
}

pub(crate) fn is_full_range_format(format: sys::AVPixelFormat) -> bool {
    matches!(
        format,
        AV_PIX_FMT_YUVJ411P
            | AV_PIX_FMT_YUVJ420P
            | AV_PIX_FMT_YUVJ422P
            | AV_PIX_FMT_YUVJ440P
            | AV_PIX_FMT_YUVJ444P
    )
}

fn subsampling_of(descriptor: &sys::AVPixFmtDescriptor) -> Option<YuvSubsampling> {
    match (descriptor.log2_chroma_w, descriptor.log2_chroma_h) {
        (0, 0) => Some(YuvSubsampling::Yuv444),
        (1, 0) => Some(YuvSubsampling::Yuv422),
        (1, 1) => Some(YuvSubsampling::Yuv420),
        _ => None,
    }
}

/// The layout of a planar little-endian format whose samples sit in the low bits, or in the high
/// bits of 16 as in P010, with chroma in two planes or one of pairs in Cb, Cr order.
fn native_shape(descriptor: &sys::AVPixFmtDescriptor) -> Option<YuvShape> {
    let flags = descriptor.flags;
    let flag = |flag: u32| flags & u64::from(flag) != 0;
    if flag(sys::AV_PIX_FMT_FLAG_BE as u32) || !flag(sys::AV_PIX_FMT_FLAG_PLANAR as u32) {
        return None;
    }
    let alpha = flag(sys::AV_PIX_FMT_FLAG_ALPHA as u32);
    let components = &descriptor.comp[..usize::from(descriptor.nb_components)];
    if components.len() != 3 + usize::from(alpha) {
        return None;
    }
    let (depth, shift) = (components[0].depth, components[0].shift);
    if !(8..=16).contains(&depth)
        || components
            .iter()
            .any(|component| component.depth != depth || component.shift != shift)
    {
        return None;
    }
    let msb_aligned = match shift {
        0 => false,
        shift if depth > 8 && shift == 16 - depth => true,
        _ => return None,
    };
    let bytes = if depth > 8 { 2 } else { 1 };
    let interleaved = components[1].plane == components[2].plane;
    let chroma_step = if interleaved { 2 * bytes } else { bytes };
    let layout_matches = components[0].plane == 0
        && components[0].step == bytes
        && components[0].offset == 0
        && components[1].plane == 1
        && components[1].step == chroma_step
        && components[2].step == chroma_step
        && components[1].offset == 0
        && components[2].offset == if interleaved { bytes } else { 0 }
        && (!alpha
            || (components[3].plane == if interleaved { 2 } else { 3 }
                && components[3].step == bytes
                && components[3].offset == 0));
    layout_matches.then_some(YuvShape {
        subsampling: subsampling_of(descriptor)?,
        depth: depth as u8,
        msb_aligned,
        interleaved,
        alpha,
    })
}

/// The chroma subsampling and depth of a little-endian format with a luma plane and a plane of
/// Cb, Cr pairs, whose samples are a byte or two bytes holding the value in the high bits, as in
/// NV12, P010 and NV24.
#[cfg(feature = "vulkan")]
fn two_plane_layout(format: sys::AVPixelFormat) -> Option<(YuvSubsampling, u8)> {
    let shape = native_shape(unsafe { sys::av_pix_fmt_desc_get(format).as_ref()? })?;
    let samples_fit = shape.depth == 8 || shape.depth == 16 || shape.msb_aligned;
    (shape.interleaved && !shape.alpha && samples_fit).then_some((shape.subsampling, shape.depth))
}

/// The planar little-endian format nearest to a YUV format `native_shape` rejects, without losing
/// chroma or depth. Full-range 8-bit sources keep to the full-range formats, as swscale tells the
/// ranges apart by format.
fn reshape_target(descriptor: &sys::AVPixFmtDescriptor, full_range: bool) -> sys::AVPixelFormat {
    use sys::AVPixelFormat::*;
    let alpha = descriptor.flags & u64::from(sys::AV_PIX_FMT_FLAG_ALPHA as u32) != 0;
    let subsampling = match (
        descriptor.log2_chroma_w.min(1),
        descriptor.log2_chroma_h.min(1),
    ) {
        (1, 1) => YuvSubsampling::Yuv420,
        (1, 0) => YuvSubsampling::Yuv422,
        _ => YuvSubsampling::Yuv444,
    };
    let depth = match descriptor.comp[0].depth {
        ..=8 => 8,
        9..=10 => 10,
        11..=12 if !alpha => 12,
        _ => 16,
    };
    match (alpha, depth, subsampling) {
        (false, 8, YuvSubsampling::Yuv420) if full_range => AV_PIX_FMT_YUVJ420P,
        (false, 8, YuvSubsampling::Yuv422) if full_range => AV_PIX_FMT_YUVJ422P,
        (false, 8, YuvSubsampling::Yuv444) if full_range => AV_PIX_FMT_YUVJ444P,
        (false, 8, YuvSubsampling::Yuv420) => AV_PIX_FMT_YUV420P,
        (false, 8, YuvSubsampling::Yuv422) => AV_PIX_FMT_YUV422P,
        (false, 8, YuvSubsampling::Yuv444) => AV_PIX_FMT_YUV444P,
        (false, 10, YuvSubsampling::Yuv420) => AV_PIX_FMT_YUV420P10LE,
        (false, 10, YuvSubsampling::Yuv422) => AV_PIX_FMT_YUV422P10LE,
        (false, 10, YuvSubsampling::Yuv444) => AV_PIX_FMT_YUV444P10LE,
        (false, 12, YuvSubsampling::Yuv420) => AV_PIX_FMT_YUV420P12LE,
        (false, 12, YuvSubsampling::Yuv422) => AV_PIX_FMT_YUV422P12LE,
        (false, 12, YuvSubsampling::Yuv444) => AV_PIX_FMT_YUV444P12LE,
        (false, _, YuvSubsampling::Yuv420) => AV_PIX_FMT_YUV420P16LE,
        (false, _, YuvSubsampling::Yuv422) => AV_PIX_FMT_YUV422P16LE,
        (false, _, YuvSubsampling::Yuv444) => AV_PIX_FMT_YUV444P16LE,
        (true, 8, YuvSubsampling::Yuv420) => AV_PIX_FMT_YUVA420P,
        (true, 8, YuvSubsampling::Yuv422) => AV_PIX_FMT_YUVA422P,
        (true, 8, YuvSubsampling::Yuv444) => AV_PIX_FMT_YUVA444P,
        (true, 10, YuvSubsampling::Yuv420) => AV_PIX_FMT_YUVA420P10LE,
        (true, 10, YuvSubsampling::Yuv422) => AV_PIX_FMT_YUVA422P10LE,
        (true, 10, YuvSubsampling::Yuv444) => AV_PIX_FMT_YUVA444P10LE,
        (true, _, YuvSubsampling::Yuv420) => AV_PIX_FMT_YUVA420P16LE,
        (true, _, YuvSubsampling::Yuv422) => AV_PIX_FMT_YUVA422P16LE,
        (true, _, YuvSubsampling::Yuv444) => AV_PIX_FMT_YUVA444P16LE,
    }
}

/// Copies a frame's planes into one tightly packed buffer, in the order [`YuvVideoFrame`] states.
fn pack_planes(frame: &AvFrame, shape: &YuvShape, width: u32, height: u32) -> Result<Vec<u8>> {
    let (width, height) = (width as usize, height as usize);
    let sample = if shape.depth > 8 { 2 } else { 1 };
    let (chroma_width, chroma_height) = match shape.subsampling {
        YuvSubsampling::Yuv420 => (width.div_ceil(2), height.div_ceil(2)),
        YuvSubsampling::Yuv422 => (width.div_ceil(2), height),
        YuvSubsampling::Yuv444 => (width, height),
    };
    // Each plane's index in the frame, row length in bytes and row count.
    let mut planes = vec![(0, width * sample, height)];
    if shape.interleaved {
        planes.push((1, chroma_width * 2 * sample, chroma_height));
    } else {
        planes.push((1, chroma_width * sample, chroma_height));
        planes.push((2, chroma_width * sample, chroma_height));
    }
    if shape.alpha {
        planes.push((
            if shape.interleaved { 2 } else { 3 },
            width * sample,
            height,
        ));
    }
    let mut data = Vec::with_capacity(planes.iter().map(|(_, row, rows)| row * rows).sum());
    for (plane, row_bytes, rows) in planes {
        let (base, linesize) = unsafe {
            (
                (*frame.as_ptr()).data[plane],
                (*frame.as_ptr()).linesize[plane] as isize,
            )
        };
        if base.is_null() {
            return Err(FfmpegError::new(
                "receive_video_frame",
                "decoded frame is missing a plane",
            ));
        }
        for row in 0..rows {
            data.extend_from_slice(unsafe {
                std::slice::from_raw_parts(base.offset(row as isize * linesize), row_bytes)
            });
        }
    }
    Ok(data)
}

/// Whether a format has a luma channel and no chroma, which swscale reads as full range unless told
/// otherwise.
fn is_gray(format: sys::AVPixelFormat) -> bool {
    let Some(descriptor) = (unsafe { sys::av_pix_fmt_desc_get(format).as_ref() }) else {
        return false;
    };
    let not_gray = (sys::AV_PIX_FMT_FLAG_PAL | sys::AV_PIX_FMT_FLAG_BITSTREAM) as u64;
    descriptor.nb_components <= 2 && descriptor.flags & not_gray == 0
}

/// Tells swscale the range of a gray source. The destination is always full-range RGBA.
fn set_gray_range(scaler: *mut sys::SwsContext, full_range: bool) -> Result<()> {
    const DEFAULT_BRIGHTNESS: i32 = 0;
    const DEFAULT_CONTRAST: i32 = 1 << 16;
    const DEFAULT_SATURATION: i32 = 1 << 16;

    unsafe {
        // Gray has no chroma, so the matrix is never read.
        let coefficients = sys::sws_getCoefficients(sys::SWS_CS_DEFAULT);
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

fn is_full_range(frame: &AvFrame) -> bool {
    frame.color_range() == AVCOL_RANGE_JPEG || is_full_range_format(frame.format())
}

fn color_tags(frame: &AvFrame) -> ColorTags {
    ColorTags::from_av(
        frame.colorspace(),
        is_full_range(frame),
        frame.color_primaries(),
        frame.color_trc(),
        frame.chroma_location(),
    )
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
    context: *mut sys::AVCodecContext,
    formats: *const sys::AVPixelFormat,
) -> sys::AVPixelFormat {
    let format = force_pixel_format(formats, AV_PIX_FMT_VULKAN);
    if format == AV_PIX_FMT_VULKAN && !unsafe { share_vulkan_frames(context) } {
        return AV_PIX_FMT_NONE;
    }
    format
}

/// The frames context a Vulkan decoder keeps across FFmpeg's rebuilds of its hardware decoder.
struct VulkanFrames {
    params: VulkanFramesParams,
    frames: AvBufferRef,
}

/// What decides whether frames can serve a rebuilt decoder: their size and format, and the Vulkan
/// video profile they were created for.
#[derive(PartialEq)]
struct VulkanFramesParams {
    width: i32,
    height: i32,
    sw_format: sys::AVPixelFormat,
    profile: i32,
    progressive: bool,
}

/// Gives the decoder the frames context it already has, unless the stream's parameters changed.
///
/// H.264 rebuilds its hardware decoder on every flush, and FFmpeg's default gives each rebuild a new
/// frames context. Every decoded frame keeps its context alive, along with the context's command
/// pools and all its images, so a cache of frames from around a seek would hold one context per
/// frame. NVIDIA's driver holds a file descriptor for each fence and timeline semaphore in them.
///
/// `avcodec_get_hw_frames_parameters` runs on every call because it also sets up the Vulkan hwaccel,
/// and FFmpeg's documentation allows using it only to compare parameters.
unsafe fn share_vulkan_frames(context: *mut sys::AVCodecContext) -> bool {
    unsafe {
        let kept = &mut *(*context).opaque.cast::<Option<VulkanFrames>>();
        let mut frames = AvBufferRef {
            ptr: ptr::null_mut(),
        };
        if sys::avcodec_get_hw_frames_parameters(
            context,
            (*context).hw_device_ctx,
            AV_PIX_FMT_VULKAN,
            &mut frames.ptr,
        ) < 0
        {
            return false;
        }
        let frames_context = (*frames.ptr).data.cast::<sys::AVHWFramesContext>();
        let params = VulkanFramesParams {
            width: (*frames_context).width,
            height: (*frames_context).height,
            sw_format: (*frames_context).sw_format,
            profile: (*context).profile,
            progressive: matches!(
                (*context).field_order,
                sys::AVFieldOrder::AV_FIELD_UNKNOWN | sys::AVFieldOrder::AV_FIELD_PROGRESSIVE
            ),
        };
        if kept.as_ref().is_none_or(|kept| kept.params != params) {
            if sys::av_hwframe_ctx_init(frames.ptr) < 0 {
                return false;
            }
            *kept = Some(VulkanFrames { params, frames });
        }
        let Some(kept) = kept else {
            return false;
        };
        (*context).hw_frames_ctx = sys::av_buffer_ref(kept.frames.ptr);
        !(*context).hw_frames_ctx.is_null()
    }
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

    #[cfg(feature = "vulkan")]
    fn from_ref(device: &crate::encode::hw::AvBufferRef) -> Result<Self> {
        let ptr = unsafe { sys::av_buffer_ref(device.ptr) };
        if ptr.is_null() {
            return Err(
                FfmpegError::new("av_buffer_ref", "failed to reference the Vulkan device")
                    .with_backend(GpuBackend::Vulkan),
            );
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

/// libvpx, for a VP8 or VP9 stream with an alpha layer. FFmpeg's native decoders ignore that layer.
fn find_alpha_layer_decoder(codec_id: sys::AVCodecID) -> Option<*const sys::AVCodec> {
    let name = match codec_id {
        AV_CODEC_ID_VP9 => c"libvpx-vp9",
        AV_CODEC_ID_VP8 => c"libvpx",
        _ => return None,
    };
    let decoder = unsafe { sys::avcodec_find_decoder_by_name(name.as_ptr()) };
    (!decoder.is_null()).then_some(decoder)
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

/// Finds a decoder for `codec_id` that can decode on `backend`. The default decoder may not: AV1
/// defaults to libdav1d where FFmpeg was built with it, and only the native decoder has hardware
/// acceleration.
fn find_hardware_decoder(
    codec_id: sys::AVCodecID,
    codec: VideoCodec,
    backend: GpuBackend,
) -> Result<*const sys::AVCodec> {
    let mut iterator = ptr::null_mut();
    loop {
        let decoder = unsafe { sys::av_codec_iterate(&mut iterator) };
        if decoder.is_null() {
            return Err(FfmpegError::new(
                "VideoDecoder::open",
                format!("{backend:?} hardware decode is unavailable for {codec:?}"),
            )
            .with_backend(backend)
            .with_codec(codec));
        }
        let matches_codec =
            unsafe { (*decoder).id == codec_id && sys::av_codec_is_decoder(decoder) != 0 };
        if matches_codec && decoder_supports_backend(decoder, backend) {
            return Ok(decoder);
        }
    }
}

fn decoder_supports_backend(decoder: *const sys::AVCodec, backend: GpuBackend) -> bool {
    let device_type = hw_device_type(backend);
    let pixel_format = hw_pixel_format(backend);
    let mut index = 0;
    loop {
        let config = unsafe { sys::avcodec_get_hw_config(decoder, index) };
        if config.is_null() {
            return false;
        }
        let supports_device_context = unsafe {
            ((*config).methods & sys::AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX as i32) != 0
        };
        let matches_backend =
            unsafe { (*config).device_type == device_type && (*config).pix_fmt == pixel_format };
        if supports_device_context && matches_backend {
            return true;
        }
        index += 1;
    }
}

/// libavcodec 62 is FFmpeg 8.0. VP9 stays out: on NVIDIA it decodes wrongly from the second inter
/// frame.
#[cfg(feature = "vulkan")]
fn ensure_vulkan_texture_decode(codec: VideoCodec, avcodec_major: u32) -> Result<()> {
    let refusal = if avcodec_major < 62 {
        format!(
            "Vulkan decode needs FFmpeg 8.0 or newer, but this FFmpeg has libavcodec {avcodec_major}"
        )
    } else if !matches!(codec, VideoCodec::H264 | VideoCodec::Hevc | VideoCodec::Av1) {
        format!("Vulkan decode doesn't cover {codec:?}")
    } else {
        return Ok(());
    };
    Err(
        FfmpegError::new("VideoDecoder::open_on_vulkan_device", refusal)
            .with_backend(GpuBackend::Vulkan)
            .with_codec(codec),
    )
}

#[cfg(test)]
mod hardware_tests {
    use std::process::Command;

    use super::*;

    /// The frames context of the first frame decoded after the input is read from the start.
    fn first_frame_context(input: &mut InputContext, decoder: &mut VideoDecoder) -> Option<usize> {
        input.seek_stream(0, 0).expect("seek to the start");
        decoder.flush().unwrap();
        let mut frame = AvFrame::new().expect("frame");
        while let Some(packet) = input.read_packet().expect("read packet") {
            decoder.send_packet(&packet).ok()?;
            if matches!(
                decoder.receive_frame(&mut frame).ok()?,
                ReceiveStatus::Frame
            ) {
                return Some(unsafe { (*(*frame.as_ptr()).hw_frames_ctx).data } as usize);
            }
        }
        None
    }

    #[test]
    fn seeks_on_a_vulkan_decoder_keep_one_frames_context() {
        let path = std::env::temp_dir().join(format!(
            "lumen_ffmpeg_frames_context_{}.mp4",
            std::process::id()
        ));
        let encoded = Command::new("ffmpeg")
            .args(["-y", "-hide_banner", "-loglevel", "error", "-f", "lavfi"])
            .args(["-i", "testsrc=s=64x64:r=30:d=1"])
            .args(["-c:v", "libx264", "-pix_fmt", "yuv420p"])
            .arg(&path)
            .status();
        if !encoded.is_ok_and(|status| status.success()) {
            eprintln!("no ffmpeg CLI with libx264; skipping");
            return;
        }
        let mut input = InputContext::open(path.to_string_lossy().to_string()).expect("open clip");
        let config = VideoDecoderConfig {
            stream_index: 0,
            mode: DecodeMode::Gpu(GpuBackend::Vulkan),
        };
        let Ok(mut decoder) = VideoDecoder::open(&input, config) else {
            eprintln!("no Vulkan decoder; skipping");
            return;
        };
        let Some(first) = first_frame_context(&mut input, &mut decoder) else {
            eprintln!("this device doesn't decode H.264 on Vulkan; skipping");
            return;
        };
        for _ in 0..3 {
            assert_eq!(
                first_frame_context(&mut input, &mut decoder),
                Some(first),
                "a seek gave the decoder a new frames context"
            );
        }
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod yuv_source_tests {
    use super::*;
    use sys::AVPixelFormat::*;

    fn native(format: sys::AVPixelFormat) -> YuvShape {
        match yuv_source(format) {
            Some(YuvSource::Native(shape)) => shape,
            _ => panic!("{format:?} should be read as it is"),
        }
    }

    fn reshaped(format: sys::AVPixelFormat) -> sys::AVPixelFormat {
        match yuv_source(format) {
            Some(YuvSource::Reshape(target, _)) => target,
            _ => panic!("{format:?} should be reshaped"),
        }
    }

    #[test]
    fn planar_little_endian_formats_pass_through() {
        let nv12 = native(AV_PIX_FMT_NV12);
        assert!(nv12.interleaved && !nv12.alpha && nv12.depth == 8);
        assert_eq!(nv12.subsampling, YuvSubsampling::Yuv420);
        let planar = native(AV_PIX_FMT_YUVJ422P);
        assert!(!planar.interleaved && planar.subsampling == YuvSubsampling::Yuv422);

        let p010 = native(AV_PIX_FMT_P010LE);
        assert!(p010.interleaved && p010.msb_aligned && p010.depth == 10);
        assert!(!native(AV_PIX_FMT_P016LE).msb_aligned);
        let ten_bit = native(AV_PIX_FMT_YUV420P10LE);
        assert!(!ten_bit.msb_aligned && ten_bit.depth == 10);

        let alpha = native(AV_PIX_FMT_YUVA444P10LE);
        assert!(alpha.alpha && alpha.subsampling == YuvSubsampling::Yuv444);
        assert!(native(AV_PIX_FMT_NV24).interleaved);
    }

    #[test]
    fn other_yuv_formats_reshape_to_the_nearest_planar_one() {
        assert_eq!(reshaped(AV_PIX_FMT_NV21), AV_PIX_FMT_YUV420P);
        assert_eq!(reshaped(AV_PIX_FMT_YUYV422), AV_PIX_FMT_YUV422P);
        assert_eq!(reshaped(AV_PIX_FMT_UYVY422), AV_PIX_FMT_YUV422P);
        assert_eq!(reshaped(AV_PIX_FMT_YUV411P), AV_PIX_FMT_YUV422P);
        assert_eq!(reshaped(AV_PIX_FMT_YUV410P), AV_PIX_FMT_YUV420P);
        assert_eq!(reshaped(AV_PIX_FMT_YUV440P), AV_PIX_FMT_YUV444P);
        assert_eq!(reshaped(AV_PIX_FMT_YUVJ440P), AV_PIX_FMT_YUVJ444P);
        assert_eq!(reshaped(AV_PIX_FMT_YUVJ411P), AV_PIX_FMT_YUVJ422P);
        assert_eq!(reshaped(AV_PIX_FMT_YUV420P10BE), AV_PIX_FMT_YUV420P10LE);
        assert_eq!(reshaped(AV_PIX_FMT_YUV444P9BE), AV_PIX_FMT_YUV444P10LE);
        assert_eq!(reshaped(AV_PIX_FMT_YUV444P14BE), AV_PIX_FMT_YUV444P16LE);
        assert_eq!(reshaped(AV_PIX_FMT_YUVA444P12BE), AV_PIX_FMT_YUVA444P16LE);
        assert_eq!(reshaped(AV_PIX_FMT_P010BE), AV_PIX_FMT_YUV420P10LE);
    }

    #[test]
    fn rgb_gray_and_palette_formats_stay_rgba() {
        for format in [
            AV_PIX_FMT_RGBA,
            AV_PIX_FMT_BGR24,
            AV_PIX_FMT_GBRP,
            AV_PIX_FMT_GRAY8,
            AV_PIX_FMT_YA8,
            AV_PIX_FMT_PAL8,
            AV_PIX_FMT_XYZ12LE,
        ] {
            assert!(yuv_source(format).is_none(), "{format:?}");
        }
    }
}

#[cfg(test)]
mod sample_tests {
    use super::*;

    fn frame(
        (width, height): (u32, u32),
        subsampling: YuvSubsampling,
        depth: u8,
        msb_aligned: bool,
        interleaved_chroma: bool,
        data: Vec<u8>,
    ) -> YuvVideoFrame {
        YuvVideoFrame {
            width,
            height,
            pts: None,
            subsampling,
            depth,
            msb_aligned,
            interleaved_chroma,
            has_alpha: false,
            color: ColorTags::new(ColorMatrix::Other, false),
            rotation: 0,
            data,
        }
    }

    #[test]
    fn samples_read_each_plane_layout() {
        let planar = frame(
            (4, 2),
            YuvSubsampling::Yuv420,
            8,
            false,
            false,
            [0, 1, 2, 3, 4, 5, 6, 7, 100, 101, 200, 201].to_vec(),
        );
        assert_eq!(planar.sample(0, 0), [0, 100, 200]);
        assert_eq!(planar.sample(3, 1), [7, 101, 201]);

        let p010 = frame(
            (2, 2),
            YuvSubsampling::Yuv420,
            10,
            true,
            true,
            [1_u16, 2, 3, 4, 512, 1023]
                .into_iter()
                .flat_map(|code| (code << 6).to_le_bytes())
                .collect(),
        );
        assert_eq!(p010.sample(1, 1), [4, 512, 1023]);
    }
}

#[cfg(all(test, feature = "vulkan"))]
mod tests {
    use super::*;

    #[test]
    fn vulkan_texture_decode_covers_h264_hevc_and_av1_from_ffmpeg_8() {
        for codec in [VideoCodec::H264, VideoCodec::Hevc, VideoCodec::Av1] {
            assert!(ensure_vulkan_texture_decode(codec, 62).is_ok());
            assert!(ensure_vulkan_texture_decode(codec, 63).is_ok());
            assert!(ensure_vulkan_texture_decode(codec, 61).is_err());
        }
        for codec in [VideoCodec::Vp9, VideoCodec::ProRes, VideoCodec::Unknown] {
            assert!(ensure_vulkan_texture_decode(codec, 62).is_err());
        }
    }

    #[test]
    fn vulkan_frames_import_as_two_plane_ycbcr_of_any_depth_and_subsampling() {
        use YuvSubsampling::{Yuv420, Yuv422, Yuv444};
        use sys::AVPixelFormat::*;

        for (format, layout) in [
            (AV_PIX_FMT_NV12, (Yuv420, 8)),
            (AV_PIX_FMT_P010LE, (Yuv420, 10)),
            (AV_PIX_FMT_P012LE, (Yuv420, 12)),
            (AV_PIX_FMT_P016LE, (Yuv420, 16)),
            (AV_PIX_FMT_NV16, (Yuv422, 8)),
            (AV_PIX_FMT_P210LE, (Yuv422, 10)),
            (AV_PIX_FMT_NV24, (Yuv444, 8)),
            (AV_PIX_FMT_P410LE, (Yuv444, 10)),
        ] {
            assert_eq!(two_plane_layout(format), Some(layout), "{format:?}");
        }
        for format in [
            AV_PIX_FMT_YUV420P,
            AV_PIX_FMT_YUV420P10LE,
            AV_PIX_FMT_YUYV422,
            AV_PIX_FMT_NV21,
            AV_PIX_FMT_P010BE,
            AV_PIX_FMT_NV20LE,
            AV_PIX_FMT_RGBA,
            AV_PIX_FMT_VULKAN,
            AV_PIX_FMT_NONE,
        ] {
            assert_eq!(two_plane_layout(format), None, "{format:?}");
        }
    }
}
