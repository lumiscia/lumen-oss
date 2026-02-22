use std::f32::consts::TAU;

use skia_safe::{Color, Paint, PathBuilder, Rect};

use crate::clip::style::{BaseStyle, EllipseStyle, PolygonStyle, RectStyle, StyleContext};
use crate::clip::{Clip, ClipMeta};
use crate::render::backend::RenderError;
use crate::render::context::{FrameContext, RendererContext};

#[derive(Debug, Clone)]
pub enum ShapeKind {
    Rectangle(RectStyle),
    Ellipse(EllipseStyle),
    Polygon(PolygonStyle),
}

impl ShapeKind {
    fn base_style(&self) -> &BaseStyle {
        match self {
            Self::Rectangle(style) => &style.base,
            Self::Ellipse(style) => &style.base,
            Self::Polygon(style) => &style.base,
        }
    }

    fn draw(
        &self,
        frame: u32,
        frame_ctx: &FrameContext,
        renderer_ctx: &mut RendererContext,
    ) -> Result<(), RenderError> {
        match self {
            Self::Rectangle(style) => style.draw(frame, frame_ctx, renderer_ctx),
            Self::Ellipse(style) => style.draw(frame, frame_ctx, renderer_ctx),
            Self::Polygon(style) => style.draw(frame, frame_ctx, renderer_ctx),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ShapeClip {
    pub meta: ClipMeta,
    pub kind: ShapeKind,
}

impl Clip for ShapeClip {
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

        self.kind
            .base_style()
            .draw(frame, frame_ctx, renderer_ctx, |renderer_ctx, _resolved| {
                self.kind.draw(frame, frame_ctx, renderer_ctx)
            })
    }
}

impl RectStyle {
    fn draw(
        &self,
        frame: u32,
        frame_ctx: &FrameContext,
        renderer_ctx: &mut RendererContext,
    ) -> Result<(), RenderError> {
        let ctx = StyleContext::new(frame);
        let width = self
            .width
            .resolve_or(&ctx, frame_ctx.width as f32 * 0.25)
            .max(1.0);
        let height = self
            .height
            .resolve_or(&ctx, frame_ctx.height as f32 * 0.25)
            .max(1.0);

        let x = (frame_ctx.width as f32 - width) * 0.5;
        let y = (frame_ctx.height as f32 - height) * 0.5;

        let mut paint = Paint::default();
        paint.set_anti_alias(true);
        paint.set_color(Color::from_argb(255, 80, 180, 255));

        renderer_ctx
            .canvas()
            .draw_rect(Rect::from_xywh(x, y, width, height), &paint);

        Ok(())
    }
}

impl EllipseStyle {
    fn draw(
        &self,
        frame: u32,
        frame_ctx: &FrameContext,
        renderer_ctx: &mut RendererContext,
    ) -> Result<(), RenderError> {
        let ctx = StyleContext::new(frame);
        let width = self
            .width
            .resolve_or(&ctx, frame_ctx.width as f32 * 0.25)
            .max(1.0);
        let height = self
            .height
            .resolve_or(&ctx, frame_ctx.height as f32 * 0.25)
            .max(1.0);

        let x = (frame_ctx.width as f32 - width) * 0.5;
        let y = (frame_ctx.height as f32 - height) * 0.5;

        let mut paint = Paint::default();
        paint.set_anti_alias(true);
        paint.set_color(Color::from_argb(255, 255, 140, 90));

        renderer_ctx
            .canvas()
            .draw_oval(Rect::from_xywh(x, y, width, height), &paint);

        Ok(())
    }
}

impl PolygonStyle {
    fn draw(
        &self,
        frame: u32,
        frame_ctx: &FrameContext,
        renderer_ctx: &mut RendererContext,
    ) -> Result<(), RenderError> {
        let ctx = StyleContext::new(frame);
        let width = self
            .width
            .resolve_or(&ctx, frame_ctx.width as f32 * 0.25)
            .max(1.0);
        let height = self
            .height
            .resolve_or(&ctx, frame_ctx.height as f32 * 0.25)
            .max(1.0);
        let sides = self.sides.resolve_or(&ctx, 5).max(3);

        let cx = frame_ctx.width as f32 * 0.5;
        let cy = frame_ctx.height as f32 * 0.5;
        let rx = width * 0.5;
        let ry = height * 0.5;

        let mut builder = PathBuilder::new();
        for index in 0..sides {
            let angle = (index as f32 / sides as f32) * TAU - std::f32::consts::FRAC_PI_2;
            let x = cx + angle.cos() * rx;
            let y = cy + angle.sin() * ry;
            if index == 0 {
                builder.move_to((x, y));
            } else {
                builder.line_to((x, y));
            }
        }
        builder.close();
        let path = builder.detach();

        let mut paint = Paint::default();
        paint.set_anti_alias(true);
        paint.set_color(Color::from_argb(255, 140, 255, 120));

        renderer_ctx.canvas().draw_path(&path, &paint);

        Ok(())
    }
}
