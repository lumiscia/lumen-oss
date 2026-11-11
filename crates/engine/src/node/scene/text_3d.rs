//! `text_3d`: text shaped as the 2D `text` node shapes it, extruded into a solid.

use super::{Material3d, Placement3d};
use crate::node::source::text::{
    TextAlignmentHorizontal, TextAlignmentVertical, TextFontStyle, TextTransform,
};
use crate::node::source::text_content::FontFeatures;
use crate::node::{Deferred, NodeId, NodeParams};

#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct Text3dParams {
    /// Text to extrude, in one style. Line breaks start new lines. Whitespace is not drawn, and
    /// glyphs that their font draws from colour or bitmap tables, such as emoji, have no outline
    /// and are left out with a `font.no_outlines` warning. A node extrudes at most as many glyphs
    /// as the host's text glyph limit lets a `text` node draw, 16384 unless the host sets
    /// another, and at most 4000000 vertices and 12000000 indices; more is a render error.
    #[meta(multiline, recommended_rows = 4)]
    pub content: String,
    /// Font family name, or `asset:<id>` for a font asset of the host's. A family no font answers
    /// to falls back as a `text` node's does.
    #[meta(format = "font_family")]
    pub font_family: String,
    /// Font size in world units: the height of the font's em square, so capitals are about 0.7
    /// of it tall. At 1, the default, a word is about as tall as a default `shape_3d` box is
    /// wide. 0 draws nothing.
    #[meta(min = 0, step = 0.05)]
    pub font_size: f64,
    /// Font weight. Variable fonts render any value in their weight range.
    #[meta(min = 1, max = 1000)]
    pub font_weight: i64,
    /// Font style.
    #[meta(kind = "enum", enum_type = TextFontStyle)]
    pub font_style: TextFontStyle,
    /// Maximum line width in world units, at which lines wrap. Use 0 for automatic width.
    #[meta(min = 0, step = 0.1)]
    pub max_width: f64,
    /// Extra space after every glyph in world units. Negative values tighten the text.
    #[meta(step = 0.01)]
    pub letter_spacing: f64,
    /// OpenType features by four-character tag, such as `{ "tnum": 1, "liga": 0 }`. 0 turns a
    /// feature off, 1 turns it on and larger values select an alternate. Values take literals
    /// or expressions, but the whole map cannot be an expression.
    #[meta()]
    pub font_features: FontFeatures,
    /// Distance between lines in world units. Use 0 for 1.2 × the font size.
    #[meta(min = 0, step = 0.05)]
    pub line_height: f64,
    /// How lines align with each other, and which point of the text's box sits at `position`:
    /// its left edge, centre or right edge. Justified lines anchor at the left edge.
    #[meta(kind = "enum", enum_type = TextAlignmentHorizontal)]
    pub alignment_horizontal: TextAlignmentHorizontal,
    /// Which point of the text's box sits at `position` vertically: the top of its first line,
    /// its middle, or the bottom of its last line.
    #[meta(kind = "enum", enum_type = TextAlignmentVertical)]
    pub alignment_vertical: TextAlignmentVertical,
    /// Case applied to the text.
    #[meta(kind = "enum", enum_type = TextTransform)]
    pub text_transform: TextTransform,
    /// Thickness along Z in world units, centred on `position`, so the front faces +Z at half of
    /// it. 0 is flat.
    #[meta(min = 0, step = 0.05)]
    pub depth: f64,
    /// How far the bevel rounds off each glyph's edges in world units, in from the outline and in
    /// from the front and back, up to half of `depth`; 0 is none. The outline keeps its size
    /// where the sides meet the bevel. A glyph whose strokes are too thin for the bevel gets the
    /// widest one that fits it, so at regular weights bevels past about 0.04 of the font size
    /// stop growing on most glyphs.
    #[meta(min = 0, step = 0.005)]
    pub bevel_size: f64,
    /// Steps of the bevel's quarter-round profile, up to 16: 1 is a flat 45° chamfer, and 0 is no
    /// bevel.
    #[meta(min = 0, max = 16, step = 1)]
    pub bevel_segments: u32,
    #[meta(flatten)]
    pub placement: Placement3d,
    #[meta(flatten)]
    pub material: Material3d,
}

impl Default for Text3dParams {
    fn default() -> Self {
        Self {
            content: "Text".to_string(),
            font_family: lumen_text::DEFAULT_FONT_FAMILY.to_string(),
            font_size: 1.0,
            font_weight: 400,
            font_style: TextFontStyle::Normal,
            max_width: 0.0,
            letter_spacing: 0.0,
            font_features: FontFeatures::default(),
            line_height: 0.0,
            alignment_horizontal: TextAlignmentHorizontal::Center,
            alignment_vertical: TextAlignmentVertical::Middle,
            text_transform: TextTransform::None,
            depth: 0.25,
            bevel_size: 0.0,
            bevel_segments: 4,
            placement: Placement3d::default(),
            material: Material3d::default(),
        }
    }
}

/// Text from its font's outlines, extruded into a solid with the material params of a
/// `shape_3d`. The text is shaped as a `text` node with the same font, size and layout shapes
/// it, with its kerning, ligatures, font fallback and wrapping, and lies in the XY plane facing
/// +Z, reading along +X, with `alignment_horizontal` and `alignment_vertical` choosing the
/// point of its box at `position`. Its geometry is built again only when the text, font, layout,
/// depth or bevel change, so animating its transform or material costs no more than for a
/// `shape_3d`. One style covers the whole text: rich text spans, decorations, strokes and the
/// `glyph_*` params of a `text` node have no counterpart.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(
    kind = "text_3d",
    name = "Text 3D",
    category = "scene",
    output = "scene"
)]
pub struct Text3d {
    pub id: NodeId,
    #[params]
    pub params: Text3dParamsDelegate,
}

impl Text3d {
    /// The family the node names as a literal, with the param naming it, for checks that run
    /// before a render.
    pub(crate) fn literal_font_families(&self) -> Vec<(String, &str)> {
        match &self.params.font_family {
            Deferred::Value(family) => vec![("font_family".to_string(), family.as_str())],
            Deferred::Expr(_) => Vec::new(),
        }
    }
}
