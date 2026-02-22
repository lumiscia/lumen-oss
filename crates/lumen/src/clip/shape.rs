use std::f32::consts::TAU;

use skia_safe::{Color, Paint, PathBuilder, Point, RRect, Rect};

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
        let rect = Rect::from_xywh(x, y, width, height);
        let max_radius = width.min(height) * 0.5;
        let corner_radii = [
            self.corner_radius[0]
                .resolve_or(&ctx, 0.0)
                .clamp(0.0, max_radius),
            self.corner_radius[1]
                .resolve_or(&ctx, 0.0)
                .clamp(0.0, max_radius),
            self.corner_radius[2]
                .resolve_or(&ctx, 0.0)
                .clamp(0.0, max_radius),
            self.corner_radius[3]
                .resolve_or(&ctx, 0.0)
                .clamp(0.0, max_radius),
        ];

        let mut paint = Paint::default();
        paint.set_anti_alias(true);
        paint.set_color(Color::from_argb(255, 80, 180, 255));

        if corner_radii.iter().any(|radius| *radius > 0.0) {
            let rrect = RRect::new_rect_radii(
                rect,
                &[
                    Point::new(corner_radii[0], corner_radii[0]),
                    Point::new(corner_radii[1], corner_radii[1]),
                    Point::new(corner_radii[2], corner_radii[2]),
                    Point::new(corner_radii[3], corner_radii[3]),
                ],
            );
            renderer_ctx.canvas().draw_rrect(rrect, &paint);
        } else {
            renderer_ctx.canvas().draw_rect(rect, &paint);
        }

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

#[cfg(test)]
mod tests {
    use skia_safe::BlendMode;

    use super::{ShapeClip, ShapeKind};
    use crate::clip::{
        Clip, ClipMeta,
        style::{BaseStyle, RectStyle, StyleProperty, StyleValue, TransformStyle},
    };
    use crate::render::{
        backend::read_surface_rgba,
        context::{FrameContext, RendererContext},
    };
    use crate::time::Rational;

    fn literal<T>(value: T) -> StyleProperty<T> {
        StyleProperty::Value(StyleValue::Literal(value))
    }

    fn base_style() -> BaseStyle {
        BaseStyle {
            visible: literal(true),
            opacity: literal(1.0),
            blend_mode: BlendMode::SrcOver,
            blur: literal(0.0),
            shadow: None,
            transform: TransformStyle {
                translate: [literal(0.0), literal(0.0)],
                scale: [literal(1.0), literal(1.0)],
                rotation: literal(0.0),
                skew: [literal(0.0), literal(0.0)],
                origin: [literal(0.0), literal(0.0)],
            },
            alignment: [literal(0.0), literal(0.0)],
        }
    }

    #[test]
    fn rectangle_corner_radius_rounds_corners() {
        let mut renderer_ctx =
            RendererContext::new(100, 100, Rational::new(30, 1)).expect("renderer context");
        renderer_ctx.clear();

        let clip = ShapeClip {
            meta: ClipMeta {
                id: Some("rect".to_owned()),
                start_frame: 0,
                end_frame: 0,
            },
            kind: ShapeKind::Rectangle(RectStyle {
                base: base_style(),
                width: literal(20.0),
                height: literal(20.0),
                corner_radius: [literal(10.0), literal(10.0), literal(10.0), literal(10.0)],
            }),
        };

        let frame_ctx = FrameContext {
            frame: 0,
            time_seconds: 0.0,
            width: 100,
            height: 100,
            device_scale: 1.0,
        };

        clip.draw(0, &frame_ctx, &mut renderer_ctx)
            .expect("shape should draw");

        let pixels = read_surface_rgba(&mut renderer_ctx).expect("readback");
        let idx = |x: usize, y: usize| (y * 100 + x) * 4;

        let outside_rounded_corner = &pixels[idx(41, 41)..idx(41, 41) + 4];
        let inside_fill = &pixels[idx(45, 45)..idx(45, 45) + 4];

        assert_eq!(outside_rounded_corner[3], 0);
        assert_eq!(inside_fill, &[80, 180, 255, 255]);
    }
}
