use std::sync::Arc;

use thiserror::Error;

use crate::compile::{CompiledTimeline, RuntimeEvalError};

#[cfg(feature = "renderer-skia")]
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
    #[error("frame evaluation failed: {0}")]
    FrameEval(#[from] RuntimeEvalError),
    #[error("renderer initialization failed: {0}")]
    RendererInit(String),
    #[error("render failed: {0}")]
    Failed(String),
    #[error("image payload length does not match dimensions")]
    InvalidImagePayload,
    #[error("pixel size overflow")]
    SizeOverflow,
    #[error("render worker thread panicked")]
    WorkerPanicked,
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

#[derive(Debug, Clone)]
pub enum ProvidedFrame {
    Ready(FrameImage),
    Missing,
    EndOfStream,
}
pub trait FrameProvider: Send {
    fn image(&mut self, source_id: &str) -> Result<ProvidedFrame, ProviderError>;
    fn video_frame(
        &mut self,
        source_id: &str,
        source_frame: u64,
    ) -> Result<ProvidedFrame, ProviderError>;

    fn video_frame_count(&mut self, _source_id: &str) -> Result<Option<u64>, ProviderError> {
        Ok(None)
    }
}

pub trait Renderer: Send {
    fn render_frame(
        &mut self,
        timeline: &CompiledTimeline,
        frame: u64,
        provider: &mut dyn FrameProvider,
    ) -> Result<Vec<u8>, RenderError>;
}

pub fn pixel_len(width: u32, height: u32) -> Result<usize, RenderError> {
    let pixels = (width as usize)
        .checked_mul(height as usize)
        .ok_or(RenderError::SizeOverflow)?;
    pixels.checked_mul(4).ok_or(RenderError::SizeOverflow)
}
