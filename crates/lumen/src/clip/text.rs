use skia_safe::{Color, Paint, paint::Style as PaintStyle};

use crate::clip::{Clip, ClipGeometry, ClipMeta, style::TextStyle};
use crate::render::backend::RenderError;
use crate::render::context::{FrameContext, RendererContext};

#[derive(Debug, Clone)]
pub struct TextClip {
    pub meta: ClipMeta,
    pub geometry: ClipGeometry,
    pub content: String,
    pub style: TextStyle,
}

impl Clip for TextClip {
    fn meta(&self) -> &ClipMeta {
        &self.meta
    }

    fn draw(
        &self,
        frame: u32,
        frame_ctx: &FrameContext,
        renderer_ctx: &mut RendererContext,
    ) -> Result<(), RenderError> {
        if !self.contains_frame(frame) {
            return Ok(());
        }

        self.style
            .base
            .draw(frame, frame_ctx, renderer_ctx, |renderer_ctx, _resolved| {
                let geometry = self.geometry.resolve_with_defaults(
                    frame,
                    frame_ctx.width as f32 * 0.15,
                    frame_ctx.height as f32 * 0.15,
                    ((self.content.len() as f32) * 8.0).max(32.0),
                    24.0,
                    0.0,
                    0.0,
                );
                let bounds = geometry.rect();

                let mut box_paint = Paint::default();
                box_paint.set_anti_alias(true);
                box_paint.set_color(Color::from_argb(160, 255, 255, 255));
                renderer_ctx.canvas().draw_rect(bounds, &box_paint);

                let mut baseline_paint = Paint::default();
                baseline_paint.set_style(PaintStyle::Stroke);
                baseline_paint.set_stroke_width(2.0);
                baseline_paint.set_color(Color::from_argb(255, 60, 60, 60));
                renderer_ctx.canvas().draw_line(
                    (geometry.left() + 4.0, geometry.top() + 18.0),
                    (
                        geometry.left() + geometry.width - 4.0,
                        geometry.top() + 18.0,
                    ),
                    &baseline_paint,
                );

                Ok(())
            })
    }
}
