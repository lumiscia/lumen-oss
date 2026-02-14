use std::sync::Arc;

use thiserror::Error;

use crate::compile::{CompileError, CompiledTimeline};

#[cfg(feature = "renderer-skia")]
pub mod skia;

#[cfg(feature = "renderer-vello")]
pub mod vello;

#[derive(Debug, Error)]
pub enum RenderError {
    #[error("failed to acquire a compatible GPU device")]
    MissingDevice,
    #[error("failed to initialize renderer: {0}")]
    RendererInit(String),
    #[error("frame {frame} is out of range for total frames {total_frames}")]
    FrameOutOfRange { frame: u64, total_frames: u64 },
    #[error("missing operation index {0}")]
    MissingOperation(usize),
    #[error("scene compile error: {0}")]
    Compile(#[from] CompileError),
    #[error("media provider error: {0}")]
    Provider(#[from] ProviderError),
    #[error("failed to map GPU buffer")]
    BufferMap,
    #[error("failed to read GPU buffer")]
    BufferRead,
    #[error("texture dimensions overflowed")]
    SizeOverflow,
    #[error("image payload length did not match dimensions")]
    InvalidImagePayload,
    #[error("text render error: {0}")]
    Text(String),
    #[error("surface creation failed: {0}")]
    SurfaceCreation(String),
    #[error("unsupported render feature: {0}")]
    Unsupported(String),
}

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("missing source `{0}`")]
    MissingSource(String),
    #[error("decode failed: {0}")]
    Decode(String),
    #[error("source failed: {0}")]
    Source(String),
}

#[derive(Debug, Clone)]
pub struct FrameImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<Vec<u8>>,
}

impl FrameImage {
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Result<Self, RenderError> {
        let expected = pixel_len(width, height)?;
        if rgba.len() != expected {
            return Err(RenderError::InvalidImagePayload);
        }

        Ok(Self {
            width,
            height,
            rgba: Arc::new(rgba),
        })
    }
}

pub trait FrameProvider: Send {
    fn image(&mut self, _source_id: &str) -> Result<Option<FrameImage>, ProviderError> {
        Ok(None)
    }

    fn video_frame(
        &mut self,
        _source_id: &str,
        _source_frame: u64,
    ) -> Result<Option<FrameImage>, ProviderError> {
        Ok(None)
    }
}

#[derive(Default)]
pub struct NoopFrameProvider;

impl FrameProvider for NoopFrameProvider {}

pub trait RenderBackend: Send {
    fn render_frame(
        &mut self,
        timeline: &CompiledTimeline,
        frame: u64,
        provider: &mut dyn FrameProvider,
    ) -> Result<Vec<u8>, RenderError>;
}

pub fn pixel_len(width: u32, height: u32) -> Result<usize, RenderError> {
    let pixel_count = (width as usize)
        .checked_mul(height as usize)
        .ok_or(RenderError::SizeOverflow)?;
    pixel_count.checked_mul(4).ok_or(RenderError::SizeOverflow)
}
