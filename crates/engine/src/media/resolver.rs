use std::sync::Arc;
#[cfg(feature = "ffmpeg")]
use std::sync::OnceLock;

use crate::error::MediaError;
use crate::media::YuvFrame;

#[derive(Debug, Clone)]
pub struct CpuMediaFrame {
    /// RGBA8, premultiplied unless the frame is a `MediaFrame::RawRgba`.
    pub rgba: Arc<Vec<u8>>,
    pub width: u32,
    pub height: u32,
    pub row_bytes: usize,
}

/// An image's straight colour in linear light with BT.709 primaries, as half floats: four per
/// pixel, rows tightly packed from the top. Colour is never negative and may be above 1, where
/// the source's gamut is wider than BT.709; alpha is within `0..=1`.
#[derive(Debug, Clone)]
pub struct LinearMediaFrame {
    pub rgba: Vec<half::f16>,
    pub width: u32,
    pub height: u32,
}

/// The matrix taking linear RGB in a source's primaries to linear BT.709 RGB, by rows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GamutMatrix(pub [[f32; 3]; 3]);

#[derive(Debug, Clone)]
pub enum MediaFrame {
    CpuRgba(Arc<CpuMediaFrame>),
    /// RGBA8 as decoded. An image's colour is straight; a video's is multiplied by alpha only if
    /// the node reading it says so.
    RawRgba(Arc<CpuMediaFrame>),
    /// An image's straight RGBA8, sRGB-encoded like `RawRgba`'s but in other primaries, which
    /// `gamut` takes to BT.709 once decoded. Colour outside BT.709 is clipped at zero.
    WideRgba {
        frame: Arc<CpuMediaFrame>,
        gamut: GamutMatrix,
    },
    /// An image whose depth or transfer an 8-bit sRGB texture would lose, already in the working
    /// space's light and primaries.
    LinearRgba(Arc<LinearMediaFrame>),
    /// Planes the renderer converts to RGBA on the GPU.
    Yuv(Arc<YuvFrame>),
    #[cfg(feature = "ffmpeg")]
    GpuVideo(Arc<GpuVideoMediaFrame>),
}

#[cfg(feature = "ffmpeg")]
pub(crate) type CpuRetry = Box<dyn Fn(&str) -> Result<MediaFrame, MediaError> + Send + Sync>;

/// Where a source's pixels come from. Hardware and software decode differ by a few levels per
/// channel, so whichever happens first settles it for the whole render: a GPU frame reaching a
/// texture, or the source moving to the CPU. Frames that were only decoded or bound settle nothing.
#[cfg(feature = "ffmpeg")]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum PixelOrigin {
    Gpu,
    Cpu,
}

#[cfg(feature = "ffmpeg")]
pub(crate) type SettledOrigin = Arc<OnceLock<PixelOrigin>>;

#[cfg(feature = "ffmpeg")]
pub struct GpuVideoMediaFrame {
    pub frame: lumen_ffmpeg::GpuVideoFrame,
    pub(crate) cpu_retry: CpuRetry,
    pub(crate) origin: SettledOrigin,
}

#[cfg(feature = "ffmpeg")]
impl std::fmt::Debug for GpuVideoMediaFrame {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GpuVideoMediaFrame")
            .field("frame", &self.frame)
            .finish_non_exhaustive()
    }
}

#[cfg(feature = "ffmpeg")]
impl GpuVideoMediaFrame {
    pub fn dimensions(&self) -> (u32, u32) {
        self.frame.dimensions()
    }

    /// Returns whether this frame can reach a texture, and from then on the source stays on the GPU.
    /// It can't once the source has moved to the CPU.
    pub(crate) fn settle_on_gpu(&self) -> bool {
        *self.origin.get_or_init(|| PixelOrigin::Gpu) == PixelOrigin::Gpu
    }

    /// Moves this frame's source to the CPU for good and decodes the frame there, for when the
    /// renderer couldn't use the GPU frame because of `reason`. Fails once a GPU frame of the source
    /// has reached a texture, as a move would change its pixels partway through. The frame it
    /// returns is never a GPU frame.
    pub fn decode_on_cpu(&self, reason: &str) -> Result<MediaFrame, MediaError> {
        match (self.cpu_retry)(reason)? {
            frame @ (MediaFrame::CpuRgba(_)
            | MediaFrame::RawRgba(_)
            | MediaFrame::WideRgba { .. }
            | MediaFrame::LinearRgba(_)
            | MediaFrame::Yuv(_)) => Ok(frame),
            MediaFrame::GpuVideo(_) => Err(MediaError::decode(
                "",
                "a source moved to the CPU returned a GPU frame",
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ImageMetadata {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct VideoMetadata {
    pub width: u32,
    pub height: u32,
    pub frame_count: u32,
    pub fps: f32,
}

pub trait ImageResolver: Send + Sync {
    fn metadata(&self) -> ImageMetadata;

    fn frame(&self) -> Result<MediaFrame, MediaError>;

    /// Why the image's embedded colour profile is read as sRGB in its place, which a render warns
    /// of.
    fn color_warning(&self) -> Option<&str> {
        None
    }
}

pub trait VideoFrameResolver: Send + Sync {
    fn metadata(&self) -> VideoMetadata;

    fn enqueue_frame(&self, _frame: u32) -> Result<(), MediaError> {
        Ok(())
    }

    fn frame(&self, frame: u32) -> Result<MediaFrame, MediaError>;

    fn retain_frames(&self, _frames: &[u32]) {}

    /// What the source's colour tags ask for that it is read without, which a render warns of.
    fn color_warning(&self) -> Option<&str> {
        None
    }
}

impl<T: ImageResolver + ?Sized> ImageResolver for Arc<T> {
    fn metadata(&self) -> ImageMetadata {
        (**self).metadata()
    }

    fn frame(&self) -> Result<MediaFrame, MediaError> {
        (**self).frame()
    }

    fn color_warning(&self) -> Option<&str> {
        (**self).color_warning()
    }
}

impl<T: VideoFrameResolver + ?Sized> VideoFrameResolver for Arc<T> {
    fn metadata(&self) -> VideoMetadata {
        (**self).metadata()
    }

    fn enqueue_frame(&self, frame: u32) -> Result<(), MediaError> {
        (**self).enqueue_frame(frame)
    }

    fn frame(&self, frame: u32) -> Result<MediaFrame, MediaError> {
        (**self).frame(frame)
    }

    fn retain_frames(&self, frames: &[u32]) {
        (**self).retain_frames(frames);
    }

    fn color_warning(&self) -> Option<&str> {
        (**self).color_warning()
    }
}

/// The files of a font family's faces.
pub trait FontResolver: Send + Sync {
    fn data(&self) -> Result<Vec<Vec<u8>>, MediaError>;
}

/// A 3D model file.
pub trait ModelResolver: Send + Sync {
    fn data(&self) -> Result<Vec<u8>, MediaError>;
}

/// An environment map file.
pub trait EnvironmentResolver: Send + Sync {
    fn data(&self) -> Result<Vec<u8>, MediaError>;
}

/// The largest `.cube` file a host should read: a table of the largest size, written with six
/// decimals, is about 58 MB.
pub const MAX_LUT_BYTES: usize = 64 << 20;

/// A `.cube` colour lookup table file.
pub trait LutResolver: Send + Sync {
    fn data(&self) -> Result<Vec<u8>, MediaError>;
}
