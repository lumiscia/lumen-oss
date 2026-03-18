use crate::{
    node::{
        NodeId, NodeProperty, VectorData, VectorPosition, VectorStroke, VectorStyle,
        VectorTextData,
        source::text::{
            TextAlignment, TextAlignmentHorizontal, TextAlignmentVertical, TextFontStyle,
        },
    },
    render::RenderContext,
};
use lumen_macros::{Node, node_impl};

#[derive(Debug, Clone, Node)]
pub struct VectorText {
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
    #[property(expected = Int)]
    pub alignment_horizontal: NodeProperty,
    #[property(expected = Int)]
    pub alignment_vertical: NodeProperty,

    #[property(expected = Vec2)]
    pub position: NodeProperty,

    #[property(expected = Bool)]
    pub fill_enabled: NodeProperty,
    #[property(expected = Color)]
    pub fill_color: NodeProperty,
    #[property(expected = Bool)]
    pub stroke_enabled: NodeProperty,
    #[property(expected = Color)]
    pub stroke_color: NodeProperty,
    #[property(expected = Float)]
    pub stroke_width: NodeProperty,
}

impl Default for VectorText {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            content: NodeProperty::String(String::new()),
            font_family: NodeProperty::String("sans-serif".to_string()),
            font_size: NodeProperty::Float(16.0),
            font_weight: NodeProperty::Int(400),
            font_style: NodeProperty::Int(0),
            max_width: NodeProperty::Float(0.0),
            alignment_horizontal: NodeProperty::Int(0),
            alignment_vertical: NodeProperty::Int(0),
            position: NodeProperty::Vec2((0.0, 0.0)),
            fill_enabled: NodeProperty::Bool(false),
            fill_color: NodeProperty::Color([255, 255, 255, 255]),
            stroke_enabled: NodeProperty::Bool(false),
            stroke_color: NodeProperty::Color([0, 0, 0, 255]),
            stroke_width: NodeProperty::Float(1.0),
        }
    }
}

#[node_impl]
impl VectorText {
    #[output(port = "vector", kind = Vector)]
    fn eval_vector(&self, ctx: &mut RenderContext) -> crate::Result<VectorData> {
        let (x, y) = self.resolve_position(ctx)?;

        let style = VectorStyle {
            color: if self.resolve_fill_enabled(ctx)? {
                Some(self.resolve_fill_color(ctx)?)
            } else {
                None
            },
            stroke: if self.resolve_stroke_enabled(ctx)? {
                Some(VectorStroke {
                    color: self.resolve_stroke_color(ctx)?,
                    width: (self.resolve_stroke_width(ctx)? as f32).max(0.0),
                })
            } else {
                None
            },
        };

        Ok(VectorData::Text(VectorTextData {
            content: self.resolve_content(ctx)?,
            font_family: self.resolve_font_family(ctx)?,
            font_size: (self.resolve_font_size(ctx)? as f32).max(1.0),
            font_weight: self.resolve_font_weight(ctx)?.clamp(0, u16::MAX as i64) as u16,
            font_style: resolve_font_style(self.resolve_font_style(ctx)?),
            max_width: resolve_max_width(self.resolve_max_width(ctx)? as f32),
            alignment: TextAlignment {
                horizontal: resolve_horizontal(self.resolve_alignment_horizontal(ctx)?),
                vertical: resolve_vertical(self.resolve_alignment_vertical(ctx)?),
            },
            position: VectorPosition {
                x: x as f32,
                y: y as f32,
            },
            style,
        }))
    }
}

fn resolve_font_style(value: i64) -> TextFontStyle {
    match value {
        1 => TextFontStyle::Italic,
        2 => TextFontStyle::Oblique,
        _ => TextFontStyle::Normal,
    }
}

fn resolve_horizontal(value: i64) -> TextAlignmentHorizontal {
    match value {
        1 => TextAlignmentHorizontal::Center,
        2 => TextAlignmentHorizontal::Right,
        3 => TextAlignmentHorizontal::Justify,
        _ => TextAlignmentHorizontal::Left,
    }
}

fn resolve_vertical(value: i64) -> TextAlignmentVertical {
    match value {
        1 => TextAlignmentVertical::Middle,
        2 => TextAlignmentVertical::Bottom,
        _ => TextAlignmentVertical::Top,
    }
}

fn resolve_max_width(value: f32) -> Option<f32> {
    if value.is_finite() && value > 0.0 {
        Some(value)
    } else {
        None
    }
}
