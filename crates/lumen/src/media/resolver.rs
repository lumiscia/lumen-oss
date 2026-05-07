use std::sync::Arc;

use crate::error::MediaError;

#[derive(Debug, Clone)]
pub struct CpuMediaFrame {
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
    ExternalTexture(ExternalTextureFrame),
}

#[cfg(feature = "ffmpeg")]
#[derive(Debug)]
pub struct GpuVideoMediaFrame {
    pub frame: lumen_ffmpeg::GpuVideoFrame,
}

#[cfg(feature = "ffmpeg")]
impl GpuVideoMediaFrame {
    pub fn dimensions(&self) -> (u32, u32) {
        self.frame.dimensions()
    }
}

#[derive(Debug, Clone)]
pub struct ExternalTextureFrame {
    pub width: u32,
    pub height: u32,
    pub format: lumen_gpu::wgpu::TextureFormat,
    pub handle: ExternalTextureHandle,
}

#[derive(Debug, Clone)]
pub enum ExternalTextureHandle {
    WgpuTexture(Arc<lumen_gpu::wgpu::Texture>),
    Platform(String),
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
