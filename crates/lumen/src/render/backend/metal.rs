use crate::render::backend::{
    FrameProvider, RenderBackend, RenderError, software::SoftwareRenderBackend,
};
use crate::render::context::{FrameContext, RendererContext};

#[derive(Debug, Default)]
pub struct MetalRenderBackend {
    software_fallback: SoftwareRenderBackend,
}

impl RenderBackend for MetalRenderBackend {
    fn render_frame(
        &mut self,
        renderer_ctx: &mut RendererContext,
        frame_ctx: &FrameContext,
        provider: &mut dyn FrameProvider,
    ) -> Result<Vec<u8>, RenderError> {
        self.software_fallback
            .render_frame(renderer_ctx, frame_ctx, provider)
    }
}
