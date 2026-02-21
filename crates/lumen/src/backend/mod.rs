use std::sync::Arc;

use thiserror::Error;

use crate::compile::CompiledTimeline;

pub mod skia;

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("missing source `{0}`")]
    MissingSource(String),
    #[error("provider failed: {0}")]
    Failed(String),
}

#[derive(Debug, Error)]
pub enum RenderError {
    #[error("frame {frame} is out of range for total frames {total_frames}")]
    FrameOutOfRange { frame: u64, total_frames: u64 },
    #[error("provider error: {0}")]
    Provider(#[from] ProviderError),
    #[error("renderer initialization failed: {0}")]
    RendererInit(String),
    #[error("render failed: {0}")]
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct FrameImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<Vec<u8>>,
}

impl FrameImage {
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Result<Self, RenderError> {
        let expected = (width as usize)
            .checked_mul(height as usize)
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or_else(|| RenderError::Failed("pixel size overflow".to_string()))?;
        if rgba.len() != expected {
            return Err(RenderError::Failed(
                "image payload length does not match dimensions".to_string(),
            ));
        }
        Ok(Self {
            width,
            height,
            rgba: Arc::new(rgba),
        })
    }
}

pub trait FrameProvider: Send {
    fn image(&mut self, source_id: &str) -> Result<Option<FrameImage>, ProviderError>;
    fn video_frame(
        &mut self,
        source_id: &str,
        source_frame: u64,
    ) -> Result<Option<FrameImage>, ProviderError>;
}

pub trait Renderer: Send {
    fn render_frame(
        &mut self,
        timeline: &CompiledTimeline,
        frame: u64,
        provider: &mut dyn FrameProvider,
    ) -> Result<Vec<u8>, RenderError>;
}
