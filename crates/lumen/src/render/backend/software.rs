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

#[cfg(test)]
mod tests {
    use super::SoftwareRenderBackend;
    use crate::render::backend::{FrameImage, FrameProvider, RenderBackend};
    use crate::render::context::{FrameContext, RendererContext};
    use crate::time::Rational;

    struct NoopProvider;

    impl FrameProvider for NoopProvider {
        fn image(
            &mut self,
            _source_id: &str,
        ) -> Result<Option<FrameImage>, crate::render::backend::RenderError> {
            Ok(None)
        }

        fn video_frame(
            &mut self,
            _source_id: &str,
            _frame: u64,
        ) -> Result<Option<FrameImage>, crate::render::backend::RenderError> {
            Ok(None)
        }
    }

    #[test]
    fn software_backend_returns_rgba_pixels_with_expected_length() {
        let mut backend = SoftwareRenderBackend;
        let mut renderer_ctx =
            RendererContext::new(2, 2, Rational::new(30, 1)).expect("renderer context");
        renderer_ctx.clear_color = skia_safe::Color::from_argb(255, 1, 2, 3);

        let frame_ctx = FrameContext {
            frame: 0,
            time_seconds: 0.0,
            width: 2,
            height: 2,
            device_scale: 1.0,
        };
        let mut provider = NoopProvider;

        let pixels = backend
            .render_frame(&mut renderer_ctx, &frame_ctx, &mut provider)
            .expect("software render");

        assert_eq!(pixels.len(), 2 * 2 * 4);
        for chunk in pixels.chunks_exact(4) {
            assert_eq!(chunk, &[1, 2, 3, 255]);
        }
    }
}
