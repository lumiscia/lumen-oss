use crate::{
    compile::CompiledTimeline,
    gpu::{FrameProvider, GpuRenderError},
};

pub trait RenderBackend: Send {
    fn render_frame(
        &mut self,
        timeline: &CompiledTimeline,
        frame: u64,
        provider: &mut dyn FrameProvider,
    ) -> Result<Vec<u8>, GpuRenderError>;
}
