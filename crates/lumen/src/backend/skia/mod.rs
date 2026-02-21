use crate::backend::{FrameProvider, RenderError, Renderer};
use crate::compile::CompiledTimeline;

mod layout;
mod mask;
mod primitives;
mod shadow;

pub struct SkiaRenderer {
    width: u32,
    height: u32,
}

impl SkiaRenderer {
    pub fn new(width: u32, height: u32) -> Result<Self, RenderError> {
        Ok(Self { width, height })
    }
}

impl Renderer for SkiaRenderer {
    fn render_frame(
        &mut self,
        timeline: &CompiledTimeline,
        frame: u64,
        _provider: &mut dyn FrameProvider,
    ) -> Result<Vec<u8>, RenderError> {
        if frame >= timeline.total_frames() {
            return Err(RenderError::FrameOutOfRange {
                frame,
                total_frames: timeline.total_frames(),
            });
        }

        let len = (self.width as usize)
            .checked_mul(self.height as usize)
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or_else(|| RenderError::Failed("pixel size overflow".to_string()))?;
        Ok(vec![0; len])
    }
}
