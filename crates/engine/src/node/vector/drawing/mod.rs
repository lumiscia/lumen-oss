//! 2D vector nodes built like geometry nodes. Geometry nodes output an outline without paint,
//! style nodes turn an outline into a drawing, and `render_2d` rasterizes a drawing in one pass.
//! Every node between a source and `render_2d` only adds to a description of what to draw, which
//! `render_2d` evaluates per pixel.

mod build;
mod collect;
#[cfg(feature = "3d")]
mod outline;
mod record;
mod tiles;

use serde_json::Value;

use super::dash::DashPattern;
use super::drawable::DrawableTransform;
use super::paint::Paint;
use super::shape::ShapeGeometryKind;
use crate::expr::Expression;
use crate::node::{
    DelegateEvalContext, DelegateEvaluable, Delegated, NodeId, NodeParams, ParamTypeDef,
    PropertyRead,
};

pub(crate) use build::DrawingCache;
#[cfg(feature = "3d")]
pub(crate) use collect::{Outline, Source, resolve_outline};
#[cfg(feature = "3d")]
pub(crate) use outline::{Contour, flatten};

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
    pub geometry: Option<NodeId>,
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
    pub geometry: Option<NodeId>,
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
    pub geometry: Option<NodeId>,
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
    pub drawing: Option<NodeId>,
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
    pub drawings: Vec<NodeId>,
}

/// Rasterizes a drawing in one pass over the canvas: each 16 by 16 pixel tile composites the
/// drawables that reach it, in order, with normal blending. Outlines are measured per pixel, so
/// they stay sharp under any transform.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(kind = "render_2d", name = "Render 2D", category = "vector")]
pub struct Render2d {
    pub id: NodeId,

    #[input(drawing)]
    pub drawing: Option<NodeId>,
}
