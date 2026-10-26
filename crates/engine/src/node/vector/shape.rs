use super::drawable::DrawableTransform;
use super::paint::Paint;
use super::renderer::{
    MAX_PATH_POINTS, PathBuffers, PathUniforms, ShapeUniforms, VectorRenderer, style_flags,
};
use crate::error::RenderError;
use crate::gpu::{BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuCompiledNode};
use crate::node::{Deferred, NodeId, NodeParamEvalContext, NodeParams, PortRef};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, lumen_macros::NodeEnum, lumen_macros::Delegate,
)]
#[repr(i64)]
#[delegate(kind = "enum")]
pub enum ShapeGeometryKind {
    #[default]
    Rectangle = 0,
    Ellipse = 1,
    Polygon = 2,
}

/// Produces a vector shape layer for GPU rasterization.
#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct ShapeParams {
    /// Geometry primitive to rasterize.
    #[meta(kind = "enum", enum_type = ShapeGeometryKind)]
    pub geometry_kind: ShapeGeometryKind,
    /// Shape width in pixels; 0 draws nothing. Ignored for polygon geometry.
    #[meta(min = 0, step = 1)]
    pub width: i64,
    /// Shape height in pixels; 0 draws nothing. Ignored for polygon geometry.
    #[meta(min = 0, step = 1)]
    pub height: i64,
    /// Corner radius for rectangle geometry.
    #[meta(min = 0, step = 1)]
    pub border_radius: f64,
    /// Polygon vertices in pixels relative to `position`, as `x,y; x,y` (3 to 16384 points).
    #[meta(
        name = "Polygon points",
        format = "point_list",
        multiline,
        recommended_rows = 3
    )]
    pub polygon_points: String,
    /// Shape origin in pixels.
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
    /// Opacity multiplier.
    #[meta(min = 0, max = 1, step = 0.01)]
    pub opacity: f64,
    /// Enables fill rendering.
    #[meta()]
    pub fill_enabled: bool,
    /// Fill paint. Accepts a solid color or gradient.
    #[meta()]
    pub fill_paint: Paint,
    /// Enables stroke rendering.
    #[meta()]
    pub stroke_enabled: bool,
    /// Stroke paint. Accepts a solid color or gradient.
    #[meta()]
    pub stroke_paint: Paint,
    /// Stroke width in untransformed pixels. Under non-uniform scale the stroke stretches
    /// with the shape, so it is wider across the more scaled axis.
    #[meta(min = 0, step = 0.5)]
    pub stroke_width: f64,
    /// Enables analytic distance-based edge antialiasing.
    #[meta()]
    pub edge_antialias: bool,
}

impl Default for ShapeParams {
    fn default() -> Self {
        Self {
            geometry_kind: ShapeGeometryKind::Rectangle,
            width: 1,
            height: 1,
            border_radius: 0.0,
            polygon_points: String::new(),
            position: (0.0, 0.0),
            scale: (1.0, 1.0),
            rotation: 0.0,
            anchor: (0.0, 0.0),
            opacity: 1.0,
            fill_enabled: true,
            fill_paint: Paint::solid([255, 255, 255, 255]),
            stroke_enabled: false,
            stroke_paint: Paint::solid([0, 0, 0, 255]),
            stroke_width: 1.0,
            edge_antialias: true,
        }
    }
}

/// Produces a vector shape layer for GPU rasterization.
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(kind = "shape", name = "Shape", category = "vector")]
pub struct Shape {
    pub id: NodeId,
    #[params]
    pub params: ShapeParamsDelegate,
}

impl Default for Shape {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            params: ShapeParamsDelegate::default(),
        }
    }
}

impl GpuCompileNode for Shape {
    fn compile_gpu(
        &self,
        ctx: &mut crate::gpu::CompileContext<'_>,
        port: &PortRef,
    ) -> crate::Result<CompiledOutput> {
        let mut renderer = VectorRenderer::new(ctx);
        // Enum params are always constants, so the pipeline can be chosen here.
        match self.params.geometry_kind {
            ShapeGeometryKindDelegate::Polygon => renderer.compile_polygon_shape(self, port),
            _ => renderer.compile_shape(self, port),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CompiledShape {
    pub(crate) node_id: NodeId,
    pub(crate) params: ShapeParamsDelegate,
    pub(crate) buffer: lumen_gpu::BufferId,
}

impl GpuCompiledNode for CompiledShape {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<()> {
        let evaluated = self.params.eval(&NodeParamEvalContext {
            node_id: self.node_id,
            expr: &ctx.expr_context(self.node_id, "params"),
        })?;
        let params = shape_uniforms(&evaluated);
        bound.write_buffer(self.buffer, 0, bytemuck::bytes_of(&params));
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CompiledPolygonShape {
    pub(crate) node_id: NodeId,
    pub(crate) params: ShapeParamsDelegate,
    pub(crate) buffers: PathBuffers,
}

impl GpuCompiledNode for CompiledPolygonShape {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<()> {
        let evaluated = self.params.eval(&NodeParamEvalContext {
            node_id: self.node_id,
            expr: &ctx.expr_context(self.node_id, "params"),
        })?;
        // Invalid or incomplete point lists draw nothing, so editing the field never fails the frame.
        let vertices = parse_polygon_points(&evaluated.polygon_points).unwrap_or_default();
        if vertices.len() > self.buffers.point_capacity {
            return Err(RenderError::NodeEvaluation {
                frame: ctx.frame(),
                node_id: self.node_id,
                node_kind: "Shape",
                details: format!(
                    "polygon has more than {} points",
                    self.buffers.point_capacity
                ),
            }
            .into());
        }
        let points = super::path::closed_contour(&vertices);
        let (x, y) = evaluated.position;
        let params = PathUniforms {
            fill_paint: evaluated.fill_paint.to_gpu([255, 255, 255, 255]),
            stroke_paint: evaluated.stroke_paint.to_gpu([0, 0, 0, 255]),
            position: [x as f32, y as f32],
            bounds_min: super::path::bounds_min(&points),
            bounds_size: super::path::bounds_size(&points),
            stroke_width: evaluated.stroke_width as f32,
            flags: shape_flags(&evaluated),
            point_count: points.len() as u32,
            trim_start: 0.0,
            trim_end: 1.0,
            ..bytemuck::Zeroable::zeroed()
        };
        self.buffers
            .write(bound, params, &points, &drawable_transform(&evaluated));
        Ok(())
    }
}

fn shape_uniforms(params: &ShapeParams) -> ShapeUniforms {
    let position = [params.position.0 as f32, params.position.1 as f32];
    let empty = params.width <= 0 || params.height <= 0;
    let size = [params.width.max(0) as f32, params.height.max(0) as f32];
    let transform = drawable_transform(params).to_gpu();
    // The stroke reaches half its width beyond the box, and antialiasing a canvas pixel.
    let margin = if params.stroke_enabled {
        (params.stroke_width.max(0.0) * 0.5) as f32
    } else {
        0.0
    };
    let [min_x, min_y, max_x, max_y] = transform.canvas_bounds(
        [position[0] - margin, position[1] - margin],
        [
            position[0] + size[0] + margin,
            position[1] + size[1] + margin,
        ],
    );
    ShapeUniforms {
        fill_paint: params.fill_paint.to_gpu([255, 255, 255, 255]),
        stroke_paint: params.stroke_paint.to_gpu([0, 0, 0, 255]),
        position,
        size,
        border_radius: params.border_radius.max(0.0) as f32,
        stroke_width: params.stroke_width as f32,
        geometry_kind: params.geometry_kind as u32,
        // Empty shapes disable fill and stroke instead of clamping to the shader's 1px minimum.
        flags: if empty { 0 } else { shape_flags(params) },
        transform,
        screen_bounds: [min_x - 1.0, min_y - 1.0, max_x + 1.0, max_y + 1.0],
    }
}

fn drawable_transform(params: &ShapeParams) -> DrawableTransform {
    DrawableTransform {
        position: params.position,
        scale: params.scale,
        rotation: params.rotation,
        anchor: params.anchor,
        opacity: params.opacity,
    }
}

/// Points buffer capacity for a polygon: its vertex count when the list is constant, or
/// `MAX_PATH_POINTS` when it may change per frame.
pub(super) fn polygon_point_capacity(polygon_points: &Deferred<String>) -> usize {
    match polygon_points {
        Deferred::Value(text) => parse_polygon_points(text)
            .map_or(1, |vertices| vertices.len())
            .min(MAX_PATH_POINTS),
        Deferred::Expr(_) => MAX_PATH_POINTS,
    }
}

fn shape_flags(params: &ShapeParams) -> u32 {
    style_flags(
        params.fill_enabled,
        params.stroke_enabled,
        params.edge_antialias,
    )
}

/// Parses `x,y` pairs separated by `;` into polygon vertices. Newlines are accepted as
/// separators too, since the editor field is multiline. Returns `None` unless the list is
/// well-formed and has at least 3 finite points.
fn parse_polygon_points(text: &str) -> Option<Vec<[f32; 2]>> {
    let vertices = text
        .split([';', '\n'])
        .map(str::trim)
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (x, y) = pair.split_once(',')?;
            let coordinate = |value: &str| {
                value
                    .trim()
                    .parse::<f32>()
                    .ok()
                    .filter(|value| value.is_finite())
            };
            Some([coordinate(x)?, coordinate(y)?])
        })
        .collect::<Option<Vec<_>>>()?;
    (vertices.len() >= 3).then_some(vertices)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negative_border_radius_binds_as_zero() {
        let params = ShapeParams {
            border_radius: -12.0,
            ..ShapeParams::default()
        };
        assert_eq!(shape_uniforms(&params).border_radius, 0.0);
    }

    #[test]
    fn parses_semicolon_and_newline_separated_points() {
        assert_eq!(
            parse_polygon_points("0,0; 10.5, 0\n 5,-8;"),
            Some(vec![[0.0, 0.0], [10.5, 0.0], [5.0, -8.0]])
        );
    }

    #[test]
    fn invalid_or_incomplete_points_draw_nothing() {
        for text in [
            "0,0; 1,0; 1",
            "0,0; 1,0; 1,1,1",
            "0,0; 1,0; inf,1",
            "0,0; 1,1",
            "",
        ] {
            assert!(
                parse_polygon_points(text).is_none(),
                "{text:?} should draw nothing"
            );
        }
    }
}
