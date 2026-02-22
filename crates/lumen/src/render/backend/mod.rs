use thiserror::Error;

use crate::render::context::{FrameContext, RendererContext};

#[derive(Debug, Clone)]
pub struct FrameImage {
    pub width: u32,
    pub height: u32,
    pub pixels_rgba: Vec<u8>,
}

pub trait FrameProvider {
    fn image(&mut self, source_id: &str) -> Result<Option<FrameImage>, RenderError>;
    fn video_frame(
        &mut self,
        source_id: &str,
        frame: u64,
    ) -> Result<Option<FrameImage>, RenderError>;
}

pub trait RenderBackend {
    fn render_frame(
        &mut self,
        renderer_ctx: &RendererContext,
        frame_ctx: &FrameContext,
        provider: &mut dyn FrameProvider,
    ) -> Result<Vec<u8>, RenderError>;
}

#[derive(Debug, Error)]
pub enum RenderError {
    #[error("unsupported render operation: {0}")]
    Unsupported(&'static str),
    #[error("missing media source: {0}")]
    MissingSource(String),
    #[error("render backend not initialized")]
    NotInitialized,
}

pub fn pixel_len(width: u32, height: u32) -> Result<usize, RenderError> {
    let len = (width as u64)
        .saturating_mul(height as u64)
        .saturating_mul(4);

    usize::try_from(len).map_err(|_| RenderError::Unsupported("frame size overflow"))
}
