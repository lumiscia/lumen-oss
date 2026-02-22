use skia_safe::font_style::Slant as FontSlant;

use crate::clip::style::{BaseStyle, StyleContext, StyleProperty};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
    Justify,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VerticalAlign {
    #[default]
    Top,
    Middle,
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextOverflow {
    #[default]
    Clip,
    Ellipsis,
    Visible,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextDecoration {
    #[default]
    None,
    Underline,
    Strikethrough,
    UnderlineStrikethrough,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolvedTextPlaceholder {
    pub font_size: f32,
    pub font_weight: u32,
    pub line_height_multiplier: f32,
    pub letter_spacing: f32,
    pub color: [u8; 4],
    pub content_width: f32,
    pub line_box_height: f32,
    pub line_count: u32,
    pub truncated: bool,
}

impl ResolvedTextPlaceholder {
    pub fn bounds(&self) -> (f32, f32) {
        (
            (self.content_width + 8.0).max(16.0),
            (self.line_box_height * self.line_count as f32 + 8.0).max(12.0),
        )
    }
}

#[derive(Debug, Clone)]
pub struct TextStyle {
    pub base: BaseStyle,
    pub font_family: String,
    pub font_size: StyleProperty<f32>,
    pub font_weight: StyleProperty<u32>,
    pub font_style: FontSlant,
    pub color: [StyleProperty<u8>; 4],
    pub line_height: StyleProperty<f32>,
    pub letter_spacing: StyleProperty<f32>,
    pub text_align: TextAlign,
    pub vertical_align: VerticalAlign,
    pub max_width: Option<StyleProperty<f32>>,
    pub max_lines: Option<u32>,
    pub overflow: TextOverflow,
    pub decoration: TextDecoration,
}

impl TextStyle {
    pub fn resolve_placeholder(&self, frame: u32, content: &str) -> ResolvedTextPlaceholder {
        let ctx = StyleContext::new(frame);
        let font_size = self.font_size.resolve_or(&ctx, 16.0).max(1.0);
        let font_weight = self.font_weight.resolve_or(&ctx, 400).clamp(100, 900);
        let line_height_multiplier = self.line_height.resolve_or(&ctx, 1.2).max(0.5);
        let letter_spacing = self.letter_spacing.resolve_or(&ctx, 0.0);
        let char_advance = (font_size * 0.6 + letter_spacing).max(0.5);
        let char_count = content.chars().count().max(1) as f32;
        let raw_width = char_count * char_advance;
        let line_box_height = (font_size * line_height_multiplier).max(font_size);
        let resolved_max_width = self
            .max_width
            .as_ref()
            .map(|max_width| max_width.resolve_or(&ctx, raw_width))
            .filter(|width| width.is_finite() && *width > 0.0);

        let (mut line_count, content_width) = if let Some(max_width) = resolved_max_width {
            let line_count = (raw_width / max_width).ceil().max(1.0) as u32;
            (line_count, raw_width.min(max_width))
        } else {
            (1, raw_width)
        };

        let truncated = if let Some(max_lines) = self.max_lines {
            let max_lines = max_lines.max(1);
            let was_truncated = line_count > max_lines;
            line_count = line_count.min(max_lines);
            was_truncated
        } else {
            false
        };

        ResolvedTextPlaceholder {
            font_size,
            font_weight,
            line_height_multiplier,
            letter_spacing,
            color: [
                self.color[0].resolve_or(&ctx, 32),
                self.color[1].resolve_or(&ctx, 32),
                self.color[2].resolve_or(&ctx, 32),
                self.color[3].resolve_or(&ctx, 255),
            ],
            content_width,
            line_box_height,
            line_count,
            truncated,
        }
    }
}

#[cfg(test)]
mod tests {
    use skia_safe::{BlendMode, font_style::Slant as FontSlant};

    use super::{TextAlign, TextDecoration, TextOverflow, TextStyle, VerticalAlign};
    use crate::clip::style::{BaseStyle, StyleProperty, StyleValue, TransformStyle};

    fn literal<T>(value: T) -> StyleProperty<T> {
        StyleProperty::Value(StyleValue::Literal(value))
    }

    fn base_style() -> BaseStyle {
        BaseStyle {
            visible: literal(true),
            opacity: literal(1.0),
            blend_mode: BlendMode::SrcOver,
            blur: literal(0.0),
            shadows: Vec::new(),
            clip_radius: [literal(0.0), literal(0.0), literal(0.0), literal(0.0)],
            transform: TransformStyle {
                translate: [literal(0.0), literal(0.0)],
                scale: [literal(1.0), literal(1.0)],
                rotation: literal(0.0),
                skew: [literal(0.0), literal(0.0)],
                origin: [literal(0.0), literal(0.0)],
            },
            alignment: [literal(0.0), literal(0.0)],
            mask: None,
        }
    }

    fn text_style() -> TextStyle {
        TextStyle {
            base: base_style(),
            font_family: "sans-serif".to_owned(),
            font_size: literal(16.0),
            font_weight: literal(400),
            font_style: FontSlant::Upright,
            color: [literal(10), literal(20), literal(30), literal(255)],
            line_height: literal(1.25),
            letter_spacing: literal(1.0),
            text_align: TextAlign::Left,
            vertical_align: VerticalAlign::Top,
            max_width: None,
            max_lines: None,
            overflow: TextOverflow::Clip,
            decoration: TextDecoration::None,
        }
    }

    #[test]
    fn placeholder_resolution_uses_text_metrics_and_color() {
        let style = text_style();
        let resolved = style.resolve_placeholder(0, "hello");

        assert_eq!(resolved.font_size, 16.0);
        assert_eq!(resolved.font_weight, 400);
        assert_eq!(resolved.color, [10, 20, 30, 255]);
        assert_eq!(resolved.line_count, 1);
        assert!(resolved.content_width > 0.0);
        let (_w, h) = resolved.bounds();
        assert!(h > 16.0);
    }

    #[test]
    fn placeholder_resolution_wraps_and_respects_max_lines() {
        let mut style = text_style();
        style.max_width = Some(literal(20.0));
        style.max_lines = Some(2);

        let resolved = style.resolve_placeholder(0, "this is a long line");
        assert_eq!(resolved.line_count, 2);
        assert!(resolved.truncated);
        assert!(resolved.content_width <= 20.0);
    }
}
