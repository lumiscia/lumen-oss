use std::sync::Arc;
#[cfg(feature = "ffmpeg")]
use std::sync::OnceLock;

use crate::error::MediaError;

#[derive(Debug, Clone)]
pub struct CpuMediaFrame {
    /// Premultiplied RGBA8, which is what the renderer's textures hold.
    pub rgba: Arc<Vec<u8>>,
    pub width: u32,
    pub height: u32,
    pub row_bytes: usize,
}

#[derive(Debug, Clone)]
pub enum MediaFrame {
    CpuRgba(Arc<CpuMediaFrame>),
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

    /// Stops this frame's source from drawing any more GPU frames, unless it already has.
    pub(crate) fn settle_on_cpu(&self) {
        self.origin.get_or_init(|| PixelOrigin::Cpu);
    }

    /// Moves this frame's source to the CPU for good and decodes the frame there, for when the
    /// renderer couldn't use the GPU frame because of `reason`. Fails once a GPU frame of the source
    /// has reached a texture, as a move would change its pixels partway through.
    pub fn decode_on_cpu(&self, reason: &str) -> Result<Arc<CpuMediaFrame>, MediaError> {
        match (self.cpu_retry)(reason)? {
            MediaFrame::CpuRgba(frame) => Ok(frame),
            MediaFrame::GpuVideo(_) => Err(MediaError::Decode {
                media_source: String::new(),
                details: "a source moved to the CPU returned a GPU frame".to_string(),
            }),
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
    fn id(&self) -> &str;

    fn metadata(&self) -> ImageMetadata;

    fn frame(&self) -> Result<MediaFrame, MediaError>;
}

pub trait VideoFrameResolver: Send + Sync {
    fn id(&self) -> &str;

    fn metadata(&self) -> VideoMetadata;

    fn enqueue_frame(&self, _frame: u32) -> Result<(), MediaError> {
        Ok(())
    }

    fn frame(&self, frame: u32) -> Result<MediaFrame, MediaError>;

    fn retain_frames(&self, _frames: &[u32]) {}
}

pub trait FontResolver: Send + Sync {
    fn id(&self) -> &str;

    fn data(&self) -> Result<Vec<Vec<u8>>, MediaError>;
}
