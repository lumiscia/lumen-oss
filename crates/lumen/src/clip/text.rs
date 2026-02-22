use skia_safe::{Color, Paint, Rect, paint::Style as PaintStyle};

use crate::clip::{Clip, ClipMeta, draw_with_base_style, style::TextStyle};
use crate::render::backend::RenderError;
use crate::render::context::{FrameContext, RendererContext};

#[derive(Debug, Clone)]
pub struct TextClip {
    pub meta: ClipMeta,
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

        draw_with_base_style(
            &self.style.base,
            frame_ctx,
            renderer_ctx,
            |renderer_ctx, _resolved| {
                let width = ((self.content.len() as f32) * 8.0).max(32.0);
                let x = frame_ctx.width as f32 * 0.15;
                let y = frame_ctx.height as f32 * 0.15;

                let mut box_paint = Paint::default();
                box_paint.set_anti_alias(true);
                box_paint.set_color(Color::from_argb(160, 255, 255, 255));
                renderer_ctx
                    .canvas()
                    .draw_rect(Rect::from_xywh(x, y, width, 24.0), &box_paint);

                let mut baseline_paint = Paint::default();
                baseline_paint.set_style(PaintStyle::Stroke);
                baseline_paint.set_stroke_width(2.0);
                baseline_paint.set_color(Color::from_argb(255, 60, 60, 60));
                renderer_ctx.canvas().draw_line(
                    (x + 4.0, y + 18.0),
                    (x + width - 4.0, y + 18.0),
                    &baseline_paint,
                );

                Ok(())
            },
        )
    }
}
