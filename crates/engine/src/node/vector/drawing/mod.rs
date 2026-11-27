//! 2D vector nodes built like geometry nodes. Geometry nodes output an outline without paint,
//! style nodes turn an outline into a drawing, and `render_2d` rasterizes a drawing in one pass.
//! Every node between a source and `render_2d` only adds to a description of what to draw, which
//! `render_2d` evaluates per pixel.

mod build;
mod collect;
mod instance;
#[cfg(feature = "3d")]
mod outline;
mod path;
mod path_bands;
mod record;
mod text;
mod tiles;

use serde_json::Value;

use super::dash::DashPattern;
use super::drawable::DrawableTransform;
use super::paint::Paint;
use crate::expr::Expression;
use crate::node::source::text::{
    TextAlignmentHorizontal, TextAlignmentVertical, TextFontStyle, TextTransform,
};
use crate::node::source::text_content::FontFeatures;
use crate::node::{
    DelegateEvalContext, DelegateEvaluable, Delegated, NodeId, NodeParams, OutputRef, ParamTypeDef,
    PropertyRead,
};

pub(crate) use build::DrawingCache;
#[cfg(feature = "3d")]
pub(crate) use collect::{Outline, Source, resolve_outline};
pub use instance::{
    Instance2d, Instance2dCopies, Instance2dCopy, Instance2dParams, Instance2dParamsDelegate,
};
#[cfg(feature = "3d")]
pub(crate) use outline::{Contour, flatten};
pub(crate) use path::{MAX_PATH_POINTS, build_lyon_path};
#[cfg(feature = "3d")]
pub(crate) use text::{TextGlyphs, shape_text};

/// Where an outline or a merged drawing sits. Scale and rotation pivot about
/// `position + anchor`.
#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct Transform2d {
    /// Origin in pixels.
    #[meta()]
    pub position: (f64, f64),
    /// Scale multiplier per axis, about the anchor.
    #[meta()]
    pub scale: (f64, f64),
    /// Clockwise rotation in degrees, about the anchor.
    #[meta(step = 1)]
    pub rotation: f64,
    /// Pivot for scale and rotation, in pixels relative to `position`.
    #[meta()]
    pub anchor: (f64, f64),
}

impl Default for Transform2d {
    fn default() -> Self {
        Self {
            position: (0.0, 0.0),
            scale: (1.0, 1.0),
            rotation: 0.0,
            anchor: (0.0, 0.0),
        }
    }
}

impl Transform2d {
    fn drawable(&self) -> DrawableTransform {
        DrawableTransform {
            position: self.position,
            scale: self.scale,
            rotation: self.rotation,
            anchor: self.anchor,
            opacity: 1.0,
        }
    }
}

/// One shadow a `shadow_2d` casts: a soft copy of each outline in its drawing, faded from the
/// outline's distance field. On blobs larger than `radius` it looks close to a `shadow` node of
/// the same radius; at sharp corners and on features thinner than `radius` it keeps more of its
/// strength than a Gaussian would.
#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct CastShadow {
    /// Shadow color. Must be a solid color.
    #[meta(role = "color")]
    pub color: Paint,
    /// Offset of the shadow in the drawing's pixels. With no offset the shadow is a glow.
    #[meta(step = 1)]
    pub offset: (f64, f64),
    /// Softness in the drawing's pixels, matching the edge of a `shadow` node of the same
    /// radius.
    #[meta(min = 0, step = 0.5)]
    pub radius: f64,
    /// Shadow opacity.
    #[meta(min = 0, max = 1, step = 0.05)]
    pub opacity: f64,
    /// Pixels of the drawing the outline grows by before it softens, or shrinks by when
    /// negative.
    #[meta(step = 0.5)]
    pub spread: f64,
}

impl Default for CastShadow {
    fn default() -> Self {
        Self {
            color: Paint::solid([0, 0, 0, 255]),
            offset: (8.0, 8.0),
            radius: 8.0,
            opacity: 0.5,
            spread: 0.0,
        }
    }
}

/// Most shadows a `shadow_2d` may cast.
pub const MAX_SHADOWS: usize = 8;

/// Shadows a `shadow_2d` casts, the first lowest. Each field takes a literal or an expression;
/// the whole list cannot be an expression.
#[derive(Debug, Clone, Default, lumen_macros::StructuralHash)]
pub struct Shadows(pub Vec<CastShadow>);

pub(crate) static SHADOWS: ParamTypeDef = ParamTypeDef {
    name: "shadows",
    #[cfg(feature = "metadata")]
    schema: || serde_json::json!({ "$ref": "#/$defs/shadows" }),
};

/// [`Shadows`] whose fields take literals or expressions.
#[derive(Debug, Clone, Default, lumen_macros::StructuralHash)]
pub struct ShadowsDelegate(pub Vec<CastShadowDelegate>);

impl Delegated for Shadows {
    type Delegate = ShadowsDelegate;

    fn into_delegate(self) -> Self::Delegate {
        ShadowsDelegate(self.0.into_iter().map(Delegated::into_delegate).collect())
    }
}

impl DelegateEvaluable for ShadowsDelegate {
    type Evaluated = Shadows;

    const ACCEPTS_EXPRESSIONS: bool = false;

    fn eval(&self, ctx: &DelegateEvalContext<'_>) -> crate::Result<Self::Evaluated> {
        self.0.eval(ctx).map(Shadows)
    }

    fn for_each_expression(&self, f: &mut dyn FnMut(&Expression)) {
        self.0.for_each_expression(f);
    }

    fn from_json_value(value: &Value, path: &str) -> Result<Self, String> {
        crate::json::reject_expression(value, path)?;
        let shadows = Vec::<CastShadowDelegate>::from_json_value(value, path)?;
        if shadows.len() > MAX_SHADOWS {
            return Err(format!(
                "`{path}` has {} shadows, more than {MAX_SHADOWS}",
                shadows.len()
            ));
        }
        Ok(Self(shadows))
    }

    fn to_json_value(&self) -> Option<Value> {
        self.0.to_json_value()
    }

    fn read(&self) -> Option<PropertyRead<'_>> {
        Some(PropertyRead::Value(None))
    }
}

#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct Path2dParams {
    /// SVG-style path data, laid out in untransformed pixels from `position`.
    #[meta(
        name = "Path data",
        format = "path_data",
        multiline,
        recommended_rows = 5
    )]
    pub data: String,
    #[meta(flatten, doc(position = "Path origin in pixels."))]
    pub transform: Transform2d,
}

impl Default for Path2dParams {
    fn default() -> Self {
        Self {
            data: "M 0 0 L 100 0 L 100 100 L 0 100 Z".to_string(),
            transform: Transform2d::default(),
        }
    }
}

/// The outline of SVG path data, without paint. The fill covers it under the even-odd rule, and
/// a stroke follows its contours, leaving the closing segment of an open contour unstroked.
/// Curves are flattened for the largest scale `render_2d` draws them at.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(
    kind = "path_2d",
    name = "Path 2D",
    category = "vector",
    output = "geometry"
)]
pub struct Path2d {
    pub id: NodeId,
    #[params]
    pub params: Path2dParamsDelegate,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, lumen_macros::NodeEnum, lumen_macros::Delegate,
)]
#[repr(i64)]
#[delegate(kind = "enum")]
pub enum ShapeGeometryKind {
    #[default]
    Rectangle = 0,
    Ellipse = 1,
}

#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct Shape2dParams {
    /// Geometry primitive the outline follows.
    #[meta()]
    pub geometry_kind: ShapeGeometryKind,
    /// Shape width in pixels; 0 draws nothing and anything thinner than a pixel draws a pixel
    /// wide.
    #[meta(min = 0, step = 1)]
    pub width: f64,
    /// Shape height in pixels; 0 draws nothing and anything thinner than a pixel draws a pixel
    /// tall.
    #[meta(min = 0, step = 1)]
    pub height: f64,
    /// Corner radius for rectangle geometry.
    #[meta(min = 0, step = 1)]
    pub border_radius: f64,
    #[meta(
        flatten,
        doc(position = "Shape origin in pixels: its top left corner.")
    )]
    pub transform: Transform2d,
}

impl Default for Shape2dParams {
    fn default() -> Self {
        Self {
            geometry_kind: ShapeGeometryKind::Rectangle,
            width: 1.0,
            height: 1.0,
            border_radius: 0.0,
            transform: Transform2d::default(),
        }
    }
}

/// The outline of a rectangle or ellipse, without paint, measured analytically. A stroke of a
/// shape is always whole: `stroke_2d`'s trim and dash apply to paths only.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(
    kind = "shape_2d",
    name = "Shape 2D",
    category = "vector",
    output = "geometry"
)]
pub struct Shape2d {
    pub id: NodeId,
    #[params]
    pub params: Shape2dParamsDelegate,
}

#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct Text2dParams {
    /// Text to outline, in one style. Line breaks start new lines. Whitespace has no outline, and
    /// glyphs that their font draws from colour or bitmap tables, such as emoji, have none either
    /// and are left out with a `font.no_outlines` warning. A node outlines at most as many glyphs
    /// as the host's text glyph limit lets a `text` node draw, 16384 unless the host sets
    /// another, and one glyph's outline has at most 4096 commands; more is a render error.
    #[meta(multiline, recommended_rows = 4)]
    pub content: String,
    /// Font family name, or `asset:<id>` for a font asset of the host's. A family no font answers
    /// to falls back as a `text` node's does.
    #[meta(format = "font_family")]
    pub font_family: String,
    /// Font size in pixels: the height of the font's em square. 0 draws nothing, and so does
    /// anything under 0.000001. At most 1000000, as are `max_width`, `letter_spacing` and
    /// `line_height`.
    #[meta(min = 0, enforced_min, step = 1)]
    pub font_size: f64,
    /// Font weight. Variable fonts render any value in their weight range.
    #[meta(min = 1, max = 1000)]
    pub font_weight: i64,
    /// Font style.
    #[meta()]
    pub font_style: TextFontStyle,
    /// Maximum line width in pixels, at which lines wrap. Use 0 for automatic width.
    #[meta(min = 0, enforced_min, step = 1)]
    pub max_width: f64,
    /// Extra space after every glyph in pixels. Negative values tighten the text.
    #[meta(step = 0.1)]
    pub letter_spacing: f64,
    /// OpenType features by four-character tag, such as `{ "tnum": 1, "liga": 0 }`. 0 turns a
    /// feature off, 1 turns it on and larger values select an alternate. Values take literals
    /// or expressions, but the whole map cannot be an expression.
    #[meta()]
    pub font_features: FontFeatures,
    /// Distance between lines in pixels. Use 0 for 1.2 × the font size.
    #[meta(min = 0, enforced_min, step = 1)]
    pub line_height: f64,
    /// How lines align with each other, and which point of the text's box sits at `position`:
    /// its left edge, centre or right edge. Justified lines anchor at the left edge.
    #[meta()]
    pub alignment_horizontal: TextAlignmentHorizontal,
    /// Which point of the text sits at `position` vertically: the top of its first line, its
    /// middle, or the bottom of its last line. `baseline`: the position is on the first line's
    /// baseline, so texts in different fonts line up.
    #[meta()]
    pub alignment_vertical: TextAlignmentVertical,
    /// Case applied to the text.
    #[meta()]
    pub text_transform: TextTransform,
    #[meta(
        flatten,
        doc(
            position = "Where the point of the text's box that the alignments pick sits, in pixels."
        )
    )]
    pub transform: Transform2d,
}

impl Default for Text2dParams {
    fn default() -> Self {
        Self {
            content: "Text".to_string(),
            font_family: lumen_text::DEFAULT_FONT_FAMILY.to_string(),
            font_size: 64.0,
            font_weight: 400,
            font_style: TextFontStyle::Normal,
            max_width: 0.0,
            letter_spacing: 0.0,
            font_features: FontFeatures::default(),
            line_height: 0.0,
            alignment_horizontal: TextAlignmentHorizontal::Left,
            alignment_vertical: TextAlignmentVertical::Top,
            text_transform: TextTransform::None,
            transform: Transform2d::default(),
        }
    }
}

/// The outlines of text's glyphs, without paint, so fills, strokes, offsets and shadows style
/// type as they do any other geometry, and one `text_2d` can feed several of them:
///
/// ```json
/// { "id": 1, "type": "text_2d", "params": { "content": "Lumen", "font_size": 240, "position": [960, 540], "alignment_horizontal": "center", "alignment_vertical": "middle" } },
/// { "id": 2, "type": "offset_2d", "params": { "offset": 12 }, "in": { "geometry": 1 } },
/// { "id": 3, "type": "fill_2d", "params": { "paint": "#ee5b24" }, "in": { "geometry": 2 } },
/// { "id": 4, "type": "fill_2d", "params": { "paint": "#ffffff" }, "in": { "geometry": 1 } },
/// { "id": 5, "type": "stroke_2d", "params": { "paint": "#111111", "width": 4 }, "in": { "geometry": 1 } },
/// { "id": 6, "type": "merge_2d", "in": { "drawings": [3, 4, 5] } },
/// { "id": 7, "type": "render_2d", "in": { "drawing": 6 } }
/// ```
///
/// The text is shaped as a `text` node with the same font, size and layout shapes it, with its
/// kerning, ligatures, font fallback and wrapping, so with left and top alignment a white fill
/// lines up with a white `text` node at the same `position`. The outline covers what the glyphs
/// cover by the nonzero rule, as fonts expect, so contours that overlap, as in variable fonts,
/// and glyphs that overlap each other fill as one, and a stroke follows only the outline's
/// edge, measuring trim and dash along each of its contours. Glyphs are shaped and outlined
/// again only when the text, font or layout change, not when the transform or the styles
/// animate. Text of more than 8192 outline points is drawn as several outlines one after
/// another, glyph by glyph, which shadows and translucent paints treat as separate drawables
/// where they overlap. Curves are flattened for the largest scale `render_2d` draws them at,
/// coarser where a text would pass 1048576 points.
///
/// It suits titles and short lines. A moving paragraph costs far more than a `text` node, whose
/// glyphs are drawn from cached images, since its outline is placed again every frame. One
/// style covers the whole text: rich text spans, decorations and the `glyph_*` params of a
/// `text` node have no counterpart.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(
    kind = "text_2d",
    name = "Text 2D",
    category = "vector",
    output = "geometry"
)]
pub struct Text2d {
    pub id: NodeId,
    #[params]
    pub params: Text2dParamsDelegate,
}

#[derive(Debug, Clone, Default, lumen_macros::Delegate)]
pub struct Offset2dParams {
    /// Distance in the geometry's untransformed pixels that the outline grows outward by, or
    /// shrinks inward by when negative.
    #[meta(step = 0.5)]
    pub offset: f64,
}

/// Grows its geometry's outline by `offset`, or shrinks it when negative, rounding the corners
/// it grows around; chained offsets add up. Fills and strokes of the result follow the offset
/// outline: a stroke is centred on it, so it lies where the signed distance from the original
/// outline equals the offset. The offset is taken from the outline the fill covers, so a path's
/// open contours count as closed by their closing segment, and a stroke of an offset path
/// strokes that closed outline. Trim and dash on such a stroke are measured along the original
/// outline and carried across to the offset one, so around a corner the offset rounds, the
/// stroke keeps the place the corner has.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(
    kind = "offset_2d",
    name = "Offset 2D",
    category = "vector",
    output = "geometry"
)]
pub struct Offset2d {
    pub id: NodeId,
    #[params]
    pub params: Offset2dParamsDelegate,

    #[input(geometry)]
    pub geometry: Option<OutputRef>,
}

#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct Fill2dParams {
    /// Fill paint. Accepts a solid color or gradient, laid out in the geometry's untransformed
    /// pixels.
    #[meta()]
    pub paint: Paint,
    /// Opacity multiplier.
    #[meta(min = 0, max = 1, step = 0.01)]
    pub opacity: f64,
    /// Width in the geometry's untransformed pixels over which the fill's edge fades out,
    /// centred on the edge.
    #[meta(min = 0, step = 0.5)]
    pub feather: f64,
    /// Antialiases the fill's edge from its distance. Off, every pixel is covered or not.
    #[meta()]
    pub edge_antialias: bool,
}

impl Default for Fill2dParams {
    fn default() -> Self {
        Self {
            paint: Paint::solid([255, 255, 255, 255]),
            opacity: 1.0,
            feather: 0.0,
            edge_antialias: true,
        }
    }
}

/// Fills its geometry's outline with a paint, as a drawing of one drawable.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(
    kind = "fill_2d",
    name = "Fill 2D",
    category = "vector",
    output = "drawing"
)]
pub struct Fill2d {
    pub id: NodeId,
    #[params]
    pub params: Fill2dParamsDelegate,

    #[input(geometry)]
    pub geometry: Option<OutputRef>,
}

#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct Stroke2dParams {
    /// Stroke paint. Accepts a solid color or gradient, laid out in the geometry's untransformed
    /// pixels.
    #[meta()]
    pub paint: Paint,
    /// Stroke width in the geometry's untransformed pixels. Under non-uniform scale the stroke
    /// stretches with the geometry, so it is wider across the more scaled axis.
    #[meta(min = 0, step = 0.5)]
    pub width: f64,
    /// Opacity multiplier.
    #[meta(min = 0, max = 1, step = 0.01)]
    pub opacity: f64,
    /// Normalized start of the visible stroke range for each contour of a path.
    #[meta(name = "Trim start", min = 0, max = 1, step = 0.01)]
    pub trim_start: f64,
    /// Normalized end of the visible stroke range for each contour of a path.
    #[meta(name = "Trim end", min = 0, max = 1, step = 0.01)]
    pub trim_end: f64,
    /// Lengths in the geometry's untransformed pixels that the stroke alternately draws and
    /// skips along each contour of a path, starting with a dash.
    #[meta()]
    pub dash: DashPattern,
    /// Distance in the geometry's untransformed pixels into the dash pattern at which each
    /// contour starts.
    #[meta(name = "Dash offset", step = 0.5)]
    pub dash_offset: f64,
}

impl Default for Stroke2dParams {
    fn default() -> Self {
        Self {
            paint: Paint::solid([0, 0, 0, 255]),
            width: 1.0,
            opacity: 1.0,
            trim_start: 0.0,
            trim_end: 1.0,
            dash: DashPattern::default(),
            dash_offset: 0.0,
        }
    }
}

/// Strokes its geometry's outline with a paint, as a drawing of one drawable. Caps and joins are
/// round. Trim and dash apply to paths; a shape's stroke is always whole.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(
    kind = "stroke_2d",
    name = "Stroke 2D",
    category = "vector",
    output = "drawing"
)]
pub struct Stroke2d {
    pub id: NodeId,
    #[params]
    pub params: Stroke2dParamsDelegate,

    #[input(geometry)]
    pub geometry: Option<OutputRef>,
}

#[derive(Debug, Clone, Default, lumen_macros::Delegate)]
pub struct Shadow2dParams {
    /// Shadows cast beneath the whole drawing, the first lowest.
    #[meta()]
    pub shadows: Shadows,
}

/// Casts shadows of every drawable in its drawing, all beneath the drawing's own drawables rather
/// than between them. Each shadow's copies of the drawables are drawn in the drawing's order,
/// and the shadows in list order. Shadows of drawables that overlap composite over each other,
/// so where they overlap they are darker than one Gaussian shadow of their union would be. A
/// shadow of a fill follows the filled outline, a shadow of a stroke the stroked band, and a
/// shadow of a shadow combines both: offsets and spreads add up and radii add as Gaussians do.
/// Lengths are in the drawing's pixels, so a `merge_2d` around this node scales them.
/// A shadow is as opaque as the paint it falls from, so a transparent fill casts none.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(
    kind = "shadow_2d",
    name = "Shadow 2D",
    category = "vector",
    output = "drawing"
)]
pub struct Shadow2d {
    pub id: NodeId,
    #[params]
    pub params: Shadow2dParamsDelegate,

    #[input(drawing)]
    pub drawing: Option<OutputRef>,
}

#[derive(Debug, Clone, Default, lumen_macros::Delegate)]
pub struct Merge2dParams {
    #[meta(flatten)]
    pub transform: Transform2d,
}

/// Draws drawings one over another in input order, the first lowest, placing them together by
/// its transform. The transform applies to everything inside, strokes and shadows included.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(
    kind = "merge_2d",
    name = "Merge 2D",
    category = "vector",
    output = "drawing"
)]
pub struct Merge2d {
    pub id: NodeId,
    #[params]
    pub params: Merge2dParamsDelegate,

    #[input(drawing, optional, variadic)]
    pub drawings: Vec<OutputRef>,
}

/// Rasterizes a drawing in one pass over the canvas: each 16 by 16 pixel tile composites the
/// drawables that reach it, in order, with normal blending. Outlines are measured per pixel, so
/// they stay sharp under any transform.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(kind = "render_2d", name = "Render 2D", category = "vector")]
pub struct Render2d {
    pub id: NodeId,

    #[input(drawing)]
    pub drawing: Option<OutputRef>,
}
