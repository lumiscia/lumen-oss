use skia_safe::{Color, Paint, paint::Style as PaintStyle};

use crate::clip::{
    Clip, ClipGeometry, ClipMeta,
    style::{TextAlign, TextDecoration, TextOverflow, TextStyle, VerticalAlign},
};
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
                let resolved_text = self.style.resolve_placeholder(frame, self.content.as_str());
                let (default_width, default_height) = resolved_text.bounds();
                let geometry = self.geometry.resolve_with_defaults(
                    frame,
                    frame_ctx.width as f32 * 0.15,
                    frame_ctx.height as f32 * 0.15,
                    default_width,
                    default_height,
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
                baseline_paint.set_stroke_width(match resolved_text.font_weight {
                    700.. => 3.0,
                    500..=699 => 2.0,
                    _ => 1.5,
                });
                baseline_paint.set_color(Color::from_argb(
                    resolved_text.color[3],
                    resolved_text.color[0],
                    resolved_text.color[1],
                    resolved_text.color[2],
                ));

                let content_width =
                    (resolved_text.content_width).min((geometry.width - 8.0).max(1.0));
                let content_height =
                    resolved_text.line_box_height * resolved_text.line_count as f32;
                let start_y = match self.style.vertical_align {
                    VerticalAlign::Top => geometry.top() + 4.0,
                    VerticalAlign::Middle => {
                        geometry.top() + (geometry.height - content_height).max(0.0) * 0.5
                    }
                    VerticalAlign::Bottom => {
                        geometry.top() + (geometry.height - content_height - 4.0).max(0.0)
                    }
                };

                for line_index in 0..resolved_text.line_count {
                    let baseline_y = start_y
                        + line_index as f32 * resolved_text.line_box_height
                        + resolved_text.font_size * 0.8;
                    let (line_x0, line_x1) = match self.style.text_align {
                        TextAlign::Left | TextAlign::Justify => {
                            (geometry.left() + 4.0, geometry.left() + 4.0 + content_width)
                        }
                        TextAlign::Center => {
                            let x0 = geometry.left() + (geometry.width - content_width) * 0.5;
                            (x0, x0 + content_width)
                        }
                        TextAlign::Right => {
                            let x1 = geometry.left() + geometry.width - 4.0;
                            (x1 - content_width, x1)
                        }
                    };

                    renderer_ctx.canvas().draw_line(
                        (line_x0, baseline_y),
                        (line_x1, baseline_y),
                        &baseline_paint,
                    );

                    if matches!(
                        self.style.decoration,
                        TextDecoration::Underline | TextDecoration::UnderlineStrikethrough
                    ) {
                        renderer_ctx.canvas().draw_line(
                            (line_x0, baseline_y + resolved_text.font_size * 0.12),
                            (line_x1, baseline_y + resolved_text.font_size * 0.12),
                            &baseline_paint,
                        );
                    }
                    if matches!(
                        self.style.decoration,
                        TextDecoration::Strikethrough | TextDecoration::UnderlineStrikethrough
                    ) {
                        renderer_ctx.canvas().draw_line(
                            (line_x0, baseline_y - resolved_text.font_size * 0.28),
                            (line_x1, baseline_y - resolved_text.font_size * 0.28),
                            &baseline_paint,
                        );
                    }
                }

                if resolved_text.truncated
                    && matches!(self.style.overflow, TextOverflow::Ellipsis)
                    && resolved_text.line_count > 0
                {
                    let dot_radius = (resolved_text.font_size * 0.06).max(1.0);
                    let y = start_y
                        + (resolved_text.line_count - 1) as f32 * resolved_text.line_box_height
                        + resolved_text.font_size * 0.8;
                    let x_end = geometry.left() + geometry.width - 6.0;
                    for i in 0..3 {
                        renderer_ctx.canvas().draw_circle(
                            (x_end - (i as f32 * dot_radius * 3.0), y),
                            dot_radius,
                            &baseline_paint,
                        );
                    }
                }

                Ok(())
            })
    }
}

#[cfg(test)]
mod tests {
    use skia_safe::{BlendMode, font_style::Slant as FontSlant};

    use super::TextClip;
    use crate::clip::{
        Clip, ClipGeometry, ClipMeta,
        style::{
            BaseStyle, StyleProperty, StyleValue, TextAlign, TextDecoration, TextOverflow,
            TextStyle, TransformStyle, VerticalAlign,
        },
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

    fn text_style() -> TextStyle {
        TextStyle {
            base: base_style(),
            font_family: "sans-serif".to_owned(),
            font_size: literal(16.0),
            font_weight: literal(600),
            font_style: FontSlant::Upright,
            color: [literal(220), literal(30), literal(40), literal(255)],
            line_height: literal(1.25),
            letter_spacing: literal(0.0),
            text_align: TextAlign::Left,
            vertical_align: VerticalAlign::Top,
            max_width: None,
            max_lines: None,
            overflow: TextOverflow::Clip,
            decoration: TextDecoration::None,
        }
    }

    #[test]
    fn text_clip_uses_text_style_color_for_placeholder_lines() {
        let mut renderer_ctx =
            RendererContext::new(100, 100, Rational::new(30, 1)).expect("renderer context");
        renderer_ctx.clear();

        let clip = TextClip {
            meta: ClipMeta {
                id: Some("txt".to_owned()),
                start_frame: 0,
                end_frame: 0,
            },
            geometry: ClipGeometry {
                x: literal(10.0),
                y: literal(10.0),
                width: literal(50.0),
                height: literal(30.0),
                anchor_x: literal(0.0),
                anchor_y: literal(0.0),
            },
            content: "hello".to_owned(),
            style: text_style(),
        };
        let frame_ctx = FrameContext {
            frame: 0,
            time_seconds: 0.0,
            width: 100,
            height: 100,
            device_scale: 1.0,
        };

        clip.draw(0, &frame_ctx, &mut renderer_ctx)
            .expect("text clip should draw");

        let pixels = read_surface_rgba(&mut renderer_ctx).expect("readback");
        let idx = |x: usize, y: usize| (y * 100 + x) * 4;
        let sample = &pixels[idx(14, 26)..idx(14, 26) + 4];
        assert_eq!(sample, &[220, 30, 40, 255]);
    }

    #[test]
    fn text_clip_supports_wrapping_and_ellipsis_placeholder_markers() {
        let mut renderer_ctx =
            RendererContext::new(120, 120, Rational::new(30, 1)).expect("renderer context");
        renderer_ctx.clear();

        let mut style = text_style();
        style.max_width = Some(literal(24.0));
        style.max_lines = Some(2);
        style.overflow = TextOverflow::Ellipsis;
        style.decoration = TextDecoration::Underline;

        let clip = TextClip {
            meta: ClipMeta {
                id: Some("txt".to_owned()),
                start_frame: 0,
                end_frame: 0,
            },
            geometry: ClipGeometry {
                x: literal(20.0),
                y: literal(20.0),
                width: literal(36.0),
                height: literal(48.0),
                anchor_x: literal(0.0),
                anchor_y: literal(0.0),
            },
            content: "wrap me over many chars".to_owned(),
            style,
        };
        let frame_ctx = FrameContext {
            frame: 0,
            time_seconds: 0.0,
            width: 120,
            height: 120,
            device_scale: 1.0,
        };

        clip.draw(0, &frame_ctx, &mut renderer_ctx)
            .expect("text clip should draw");

        let pixels = read_surface_rgba(&mut renderer_ctx).expect("readback");
        let idx = |x: usize, y: usize| (y * 120 + x) * 4;
        let ellipsis_alpha = pixels[idx(50, 56) + 3];
        assert!(ellipsis_alpha > 0);
    }
}
