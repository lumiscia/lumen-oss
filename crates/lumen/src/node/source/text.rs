use skia_safe::textlayout::Paragraph;

use crate::{
    node::{
        NodeId, NodeProperty,
        pixel_utils::{ClearMode, render_to_surface_ephemeral},
        source::text_layout::{TextLayoutStyle, build_paragraph, resolved_max_width},
    },
    raster::{AlphaMode, RasterFrame, RectI},
    render::RenderContext,
};
use lumen_macros::{Node, node_impl};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextFontStyle {
    Normal,
    Italic,
    Oblique,
}

impl TextFontStyle {
    pub(crate) fn from_int(value: i64) -> Self {
        match value {
            1 => Self::Italic,
            2 => Self::Oblique,
            _ => Self::Normal,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextAlignmentHorizontal {
    Left,
    Center,
    Right,
    Justify,
}

impl TextAlignmentHorizontal {
    fn from_int(value: i64) -> Self {
        match value {
            1 => Self::Center,
            2 => Self::Right,
            3 => Self::Justify,
            _ => Self::Left,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextAlignmentVertical {
    Top,
    Middle,
    Bottom,
}

impl TextAlignmentVertical {
    fn from_int(value: i64) -> Self {
        match value {
            1 => Self::Middle,
            2 => Self::Bottom,
            _ => Self::Top,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextAlignment {
    pub horizontal: TextAlignmentHorizontal,
    pub vertical: TextAlignmentVertical,
}

impl Default for TextAlignment {
    fn default() -> Self {
        Self {
            horizontal: TextAlignmentHorizontal::Left,
            vertical: TextAlignmentVertical::Top,
        }
    }
}

#[derive(Debug, Clone, Node)]
pub struct Text {
    pub id: NodeId,

    #[property(expected = String)]
    pub content: NodeProperty,
    #[property(expected = String)]
    pub font_family: NodeProperty,
    #[property(expected = Float)]
    pub font_size: NodeProperty,
    #[property(expected = Int)]
    pub font_weight: NodeProperty,
    #[property(expected = Int)]
    pub font_style: NodeProperty,
    #[property(expected = Float)]
    pub max_width: NodeProperty,
    #[property(expected = Color)]
    pub color: NodeProperty,
    #[property(expected = Int)]
    pub alignment_horizontal: NodeProperty,
    #[property(expected = Int)]
    pub alignment_vertical: NodeProperty,
}

impl Default for Text {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            content: NodeProperty::String(String::new()),
            font_family: NodeProperty::String("sans-serif".to_string()),
            font_size: NodeProperty::Float(16.0),
            font_weight: NodeProperty::Int(400),
            font_style: NodeProperty::Int(0),
            max_width: NodeProperty::Float(0.0),
            color: NodeProperty::Color([255, 255, 255, 255]),
            alignment_horizontal: NodeProperty::Int(0),
            alignment_vertical: NodeProperty::Int(0),
        }
    }
}

#[node_impl]
impl Text {
    #[output(port = "output", kind = Raster)]
    fn eval_output(&self, ctx: &mut RenderContext) -> crate::Result<RasterFrame> {
        let content = self.resolve_content(ctx)?;
        let font_family = self.resolve_font_family(ctx)?;
        let font_size = self.resolve_font_size(ctx)? as f32;
        let font_weight = self.resolve_font_weight(ctx)? as i32;
        let font_style = TextFontStyle::from_int(self.resolve_font_style(ctx)?);
        let max_width = resolved_max_width(self.resolve_max_width(ctx)? as f32);
        let color = self.resolve_color(ctx)?;
        let alignment = TextAlignment {
            horizontal: TextAlignmentHorizontal::from_int(self.resolve_alignment_horizontal(ctx)?),
            vertical: TextAlignmentVertical::from_int(self.resolve_alignment_vertical(ctx)?),
        };

        let layout_width = max_width
            .unwrap_or(ctx.renderer.composition.render_settings.width as f32)
            .clamp(1.0, u32::MAX as f32);
        let text_style = TextLayoutStyle::new(font_family, font_size, font_weight, font_style);
        let mut paragraph: Paragraph =
            build_paragraph(&content, &text_style, color, alignment.horizontal);
        paragraph.layout(layout_width);

        let rendered_width = if max_width.is_some() {
            paragraph.longest_line()
        } else {
            paragraph.max_intrinsic_width()
        }
        .max(1.0)
        .min(layout_width);

        let horizontal_offset = match alignment.horizontal {
            TextAlignmentHorizontal::Left | TextAlignmentHorizontal::Justify => 0.0,
            TextAlignmentHorizontal::Center => ((layout_width - rendered_width) * 0.5).max(0.0),
            TextAlignmentHorizontal::Right => (layout_width - rendered_width).max(0.0),
        };

        let width = rendered_width.ceil().max(1.0) as u32;
        let height = paragraph.height().ceil().max(1.0) as u32;
        let vertical_offset = match alignment.vertical {
            TextAlignmentVertical::Top => 0.0,
            TextAlignmentVertical::Middle => (height as f32 - paragraph.height()).max(0.0) * 0.5,
            TextAlignmentVertical::Bottom => (height as f32 - paragraph.height()).max(0.0),
        };
        render_to_surface_ephemeral(
            width,
            height,
            ctx,
            RectI::from_size(width, height),
            RectI::from_size(width, height),
            AlphaMode::Premultiplied,
            ClearMode::Transparent,
            |canvas| {
                paragraph.paint(canvas, (-horizontal_offset, vertical_offset));
            },
        )
    }
}
