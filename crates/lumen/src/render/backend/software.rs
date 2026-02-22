use crate::render::backend::{FrameProvider, RenderBackend, RenderError, read_surface_rgba};
use crate::render::context::{FrameContext, RendererContext};

#[derive(Debug, Default)]
pub struct SoftwareRenderBackend;

impl RenderBackend for SoftwareRenderBackend {
    fn render_frame(
        &mut self,
        renderer_ctx: &mut RendererContext,
        _frame_ctx: &FrameContext,
        _provider: &mut dyn FrameProvider,
    ) -> Result<Vec<u8>, RenderError> {
        renderer_ctx.clear();
        read_surface_rgba(renderer_ctx)
    }
}
