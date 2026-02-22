use std::f32::consts::TAU;

use skia_safe::{Color, Paint, PathBuilder, PathEffect, Point, RRect, paint::Style as PaintStyle};

use crate::clip::style::{
    BaseStyle, EllipseStyle, Fill, PolygonStyle, RectStyle, Stroke, StyleContext,
};
use crate::clip::{Clip, ClipGeometry, ClipMeta, ResolvedClipGeometry};
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
        geometry: &ResolvedClipGeometry,
    ) -> Result<(), RenderError> {
        match self {
            Self::Rectangle(style) => style.draw(frame, frame_ctx, renderer_ctx, geometry),
            Self::Ellipse(style) => style.draw(frame, frame_ctx, renderer_ctx, geometry),
            Self::Polygon(style) => style.draw(frame, frame_ctx, renderer_ctx, geometry),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ShapeClip {
    pub meta: ClipMeta,
    pub geometry: ClipGeometry,
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

        let geometry = self.geometry.resolve_with_defaults(
            frame,
            frame_ctx.width as f32 * 0.5,
            frame_ctx.height as f32 * 0.5,
            frame_ctx.width as f32 * 0.25,
            frame_ctx.height as f32 * 0.25,
            0.5,
            0.5,
        );

        self.kind
            .base_style()
            .draw(frame, frame_ctx, renderer_ctx, |renderer_ctx, _resolved| {
                self.kind.draw(frame, frame_ctx, renderer_ctx, &geometry)
            })
    }
}

impl RectStyle {
    fn draw(
        &self,
        frame: u32,
        _frame_ctx: &FrameContext,
        renderer_ctx: &mut RendererContext,
        geometry: &ResolvedClipGeometry,
    ) -> Result<(), RenderError> {
        let ctx = StyleContext::new(frame);
        let width = self.width.resolve_or(&ctx, geometry.width).max(1.0);
        let height = self.height.resolve_or(&ctx, geometry.height).max(1.0);
        let rect = geometry.rect_for(width, height);
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
        let rrect = corner_radii.iter().any(|radius| *radius > 0.0).then(|| {
            RRect::new_rect_radii(
                rect,
                &[
                    Point::new(corner_radii[0], corner_radii[0]),
                    Point::new(corner_radii[1], corner_radii[1]),
                    Point::new(corner_radii[2], corner_radii[2]),
                    Point::new(corner_radii[3], corner_radii[3]),
                ],
            )
        });

        let default_fill = (self.fill.is_none() && self.stroke.is_none())
            .then_some(Color::from_argb(255, 80, 180, 255));
        let fill_paint = self.fill.as_ref().map(|f| f.to_paint(&ctx)).or_else(|| {
            let color = default_fill?;
            let mut paint = Paint::default();
            paint.set_anti_alias(true);
            paint.set_color(color);
            Some(paint)
        });
        let stroke_paint = self.stroke.as_ref().and_then(|s| s.to_paint(&ctx));

        if let Some(fill_paint) = &fill_paint {
            if let Some(rrect) = rrect {
                renderer_ctx.canvas().draw_rrect(rrect, fill_paint);
            } else {
                renderer_ctx.canvas().draw_rect(rect, fill_paint);
            }
        }

        if let Some(stroke_paint) = &stroke_paint {
            if let Some(rrect) = rrect {
                renderer_ctx.canvas().draw_rrect(rrect, stroke_paint);
            } else {
                renderer_ctx.canvas().draw_rect(rect, stroke_paint);
            }
        }

        Ok(())
    }
}

impl EllipseStyle {
    fn draw(
        &self,
        frame: u32,
        _frame_ctx: &FrameContext,
        renderer_ctx: &mut RendererContext,
        geometry: &ResolvedClipGeometry,
    ) -> Result<(), RenderError> {
        let ctx = StyleContext::new(frame);
        let width = self.width.resolve_or(&ctx, geometry.width).max(1.0);
        let height = self.height.resolve_or(&ctx, geometry.height).max(1.0);
        let bounds = geometry.rect_for(width, height);

        let default_fill = (self.fill.is_none() && self.stroke.is_none())
            .then_some(Color::from_argb(255, 255, 140, 90));
        let fill_paint = self.fill.as_ref().map(|f| f.to_paint(&ctx)).or_else(|| {
            let color = default_fill?;
            let mut paint = Paint::default();
            paint.set_anti_alias(true);
            paint.set_color(color);
            Some(paint)
        });
        if let Some(fill_paint) = fill_paint {
            renderer_ctx.canvas().draw_oval(bounds, &fill_paint);
        }
        if let Some(stroke_paint) = self.stroke.as_ref().and_then(|s| s.to_paint(&ctx)) {
            renderer_ctx.canvas().draw_oval(bounds, &stroke_paint);
        }

        Ok(())
    }
}

impl PolygonStyle {
    fn draw(
        &self,
        frame: u32,
        _frame_ctx: &FrameContext,
        renderer_ctx: &mut RendererContext,
        geometry: &ResolvedClipGeometry,
    ) -> Result<(), RenderError> {
        let ctx = StyleContext::new(frame);
        let width = self.width.resolve_or(&ctx, geometry.width).max(1.0);
        let height = self.height.resolve_or(&ctx, geometry.height).max(1.0);
        let sides = self.sides.resolve_or(&ctx, 5).max(3);

        let cx = geometry.x;
        let cy = geometry.y;
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

        let default_fill = (self.fill.is_none() && self.stroke.is_none())
            .then_some(Color::from_argb(255, 140, 255, 120));
        let fill_paint = self.fill.as_ref().map(|f| f.to_paint(&ctx)).or_else(|| {
            let color = default_fill?;
            let mut paint = Paint::default();
            paint.set_anti_alias(true);
            paint.set_color(color);
            Some(paint)
        });
        if let Some(fill_paint) = fill_paint {
            renderer_ctx.canvas().draw_path(&path, &fill_paint);
        }
        if let Some(stroke_paint) = self.stroke.as_ref().and_then(|s| s.to_paint(&ctx)) {
            renderer_ctx.canvas().draw_path(&path, &stroke_paint);
        }

        Ok(())
    }
}

impl Fill {
    fn to_paint(&self, ctx: &StyleContext<'_>) -> Paint {
        let mut paint = Paint::default();
        paint.set_anti_alias(true);
        match self {
            Fill::Solid { color } => {
                let [r, g, b, a] = resolve_rgba(color, ctx);
                paint.set_color(Color::from_argb(a, r, g, b));
            }
        }
        paint
    }
}

impl Stroke {
    fn to_paint(&self, ctx: &StyleContext<'_>) -> Option<Paint> {
        let width = self.width.resolve_or(ctx, 1.0).max(0.0);
        if width <= 0.0 {
            return None;
        }

        let [r, g, b, a] = resolve_rgba(&self.color, ctx);
        let mut paint = Paint::default();
        paint.set_anti_alias(true);
        paint.set_style(PaintStyle::Stroke);
        paint.set_color(Color::from_argb(a, r, g, b));
        paint.set_stroke_width(width);
        paint.set_stroke_cap(self.line_cap);
        paint.set_stroke_join(self.line_join);

        if let Some(pattern) = &self.dash_pattern {
            let intervals = pattern
                .iter()
                .copied()
                .filter(|value| value.is_finite() && *value > 0.0)
                .collect::<Vec<_>>();
            if intervals.len() >= 2 {
                paint.set_path_effect(PathEffect::dash(intervals.as_slice(), 0.0));
            }
        }

        Some(paint)
    }
}

fn resolve_rgba(
    color: &[crate::clip::style::StyleProperty<u8>; 4],
    ctx: &StyleContext<'_>,
) -> [u8; 4] {
    [
        color[0].resolve_or(ctx, 0),
        color[1].resolve_or(ctx, 0),
        color[2].resolve_or(ctx, 0),
        color[3].resolve_or(ctx, 255),
    ]
}

#[cfg(test)]
mod tests {
    use skia_safe::{BlendMode, paint};

    use super::{ShapeClip, ShapeKind};
    use crate::clip::{
        Clip, ClipGeometry, ClipMeta,
        style::{BaseStyle, Fill, RectStyle, Stroke, StyleProperty, StyleValue, TransformStyle},
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
            clip_radius: [literal(0.0), literal(0.0), literal(0.0), literal(0.0)],
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
            geometry: ClipGeometry::default(),
            kind: ShapeKind::Rectangle(RectStyle {
                base: base_style(),
                width: literal(20.0),
                height: literal(20.0),
                corner_radius: [literal(10.0), literal(10.0), literal(10.0), literal(10.0)],
                fill: None,
                stroke: None,
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

    #[test]
    fn rectangle_fill_color_overrides_default() {
        let mut renderer_ctx =
            RendererContext::new(100, 100, Rational::new(30, 1)).expect("renderer context");
        renderer_ctx.clear();

        let clip = ShapeClip {
            meta: ClipMeta {
                id: Some("rect".to_owned()),
                start_frame: 0,
                end_frame: 0,
            },
            geometry: ClipGeometry::default(),
            kind: ShapeKind::Rectangle(RectStyle {
                base: base_style(),
                width: literal(20.0),
                height: literal(20.0),
                corner_radius: [literal(0.0), literal(0.0), literal(0.0), literal(0.0)],
                fill: Some(Fill::Solid {
                    color: [literal(1), literal(2), literal(3), literal(255)],
                }),
                stroke: None,
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
        assert_eq!(&pixels[idx(50, 50)..idx(50, 50) + 4], &[1, 2, 3, 255]);
    }

    #[test]
    fn rectangle_stroke_only_leaves_center_transparent() {
        let mut renderer_ctx =
            RendererContext::new(100, 100, Rational::new(30, 1)).expect("renderer context");
        renderer_ctx.clear();

        let clip = ShapeClip {
            meta: ClipMeta {
                id: Some("rect".to_owned()),
                start_frame: 0,
                end_frame: 0,
            },
            geometry: ClipGeometry::default(),
            kind: ShapeKind::Rectangle(RectStyle {
                base: base_style(),
                width: literal(20.0),
                height: literal(20.0),
                corner_radius: [literal(0.0), literal(0.0), literal(0.0), literal(0.0)],
                fill: None,
                stroke: Some(Stroke {
                    color: [literal(255), literal(0), literal(0), literal(255)],
                    width: literal(4.0),
                    dash_pattern: Some(vec![2.0, 2.0]),
                    line_cap: paint::Cap::Butt,
                    line_join: paint::Join::Miter,
                }),
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
        let edge = &pixels[idx(40, 50)..idx(40, 50) + 4];
        let center = &pixels[idx(50, 50)..idx(50, 50) + 4];

        assert!(edge[3] > 0);
        assert_eq!(center[3], 0);
    }

    #[test]
    fn rectangle_uses_clip_geometry_position_and_size() {
        let mut renderer_ctx =
            RendererContext::new(100, 100, Rational::new(30, 1)).expect("renderer context");
        renderer_ctx.clear();

        let clip = ShapeClip {
            meta: ClipMeta {
                id: Some("rect".to_owned()),
                start_frame: 0,
                end_frame: 0,
            },
            geometry: ClipGeometry {
                x: literal(20.0),
                y: literal(30.0),
                width: literal(10.0),
                height: literal(10.0),
                anchor_x: literal(0.0),
                anchor_y: literal(0.0),
            },
            kind: ShapeKind::Rectangle(RectStyle {
                base: base_style(),
                width: literal(10.0),
                height: literal(10.0),
                corner_radius: [literal(0.0), literal(0.0), literal(0.0), literal(0.0)],
                fill: Some(Fill::Solid {
                    color: [literal(9), literal(8), literal(7), literal(255)],
                }),
                stroke: None,
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

        assert_eq!(&pixels[idx(20, 30)..idx(20, 30) + 4], &[9, 8, 7, 255]);
        assert_eq!(pixels[idx(50, 50) + 3], 0);
    }
}
