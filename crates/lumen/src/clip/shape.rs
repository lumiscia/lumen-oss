use std::f32::consts::TAU;

use skia_safe::{Color, Paint, PathBuilder, Rect};

use crate::clip::style::{
    BaseStyle, EllipseStyle, PolygonStyle, RectStyle, resolve_style_value_or,
};
use crate::clip::{Clip, ClipMeta, draw_with_base_style};
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

        draw_with_base_style(
            self.kind.base_style(),
            frame_ctx,
            renderer_ctx,
            |renderer_ctx, _resolved| {
                match &self.kind {
                    ShapeKind::Rectangle(style) => draw_rectangle(style, frame_ctx, renderer_ctx),
                    ShapeKind::Ellipse(style) => draw_ellipse(style, frame_ctx, renderer_ctx),
                    ShapeKind::Polygon(style) => draw_polygon(style, frame_ctx, renderer_ctx),
                }

                Ok(())
            },
        )
    }
}

fn draw_rectangle(style: &RectStyle, frame_ctx: &FrameContext, renderer_ctx: &mut RendererContext) {
    let width = resolve_style_value_or(&style.width, frame_ctx.width as f32 * 0.25).max(1.0);
    let height = resolve_style_value_or(&style.height, frame_ctx.height as f32 * 0.25).max(1.0);

    let x = (frame_ctx.width as f32 - width) * 0.5;
    let y = (frame_ctx.height as f32 - height) * 0.5;

    let mut paint = Paint::default();
    paint.set_anti_alias(true);
    paint.set_color(Color::from_argb(255, 80, 180, 255));

    renderer_ctx
        .canvas()
        .draw_rect(Rect::from_xywh(x, y, width, height), &paint);
}

fn draw_ellipse(
    style: &EllipseStyle,
    frame_ctx: &FrameContext,
    renderer_ctx: &mut RendererContext,
) {
    let width = resolve_style_value_or(&style.width, frame_ctx.width as f32 * 0.25).max(1.0);
    let height = resolve_style_value_or(&style.height, frame_ctx.height as f32 * 0.25).max(1.0);

    let x = (frame_ctx.width as f32 - width) * 0.5;
    let y = (frame_ctx.height as f32 - height) * 0.5;

    let mut paint = Paint::default();
    paint.set_anti_alias(true);
    paint.set_color(Color::from_argb(255, 255, 140, 90));

    renderer_ctx
        .canvas()
        .draw_oval(Rect::from_xywh(x, y, width, height), &paint);
}

fn draw_polygon(
    style: &PolygonStyle,
    frame_ctx: &FrameContext,
    renderer_ctx: &mut RendererContext,
) {
    let width = resolve_style_value_or(&style.width, frame_ctx.width as f32 * 0.25).max(1.0);
    let height = resolve_style_value_or(&style.height, frame_ctx.height as f32 * 0.25).max(1.0);
    let sides = resolve_style_value_or(&style.sides, 5).max(3);

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
}
