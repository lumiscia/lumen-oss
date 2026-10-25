use super::paint::Paint;
use super::renderer::{MAX_PATH_POINTS, PathUniforms, ShapeUniforms, VectorRenderer, style_flags};
use crate::gpu::{BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuCompiledNode};
use crate::node::{NodeId, NodeParamEvalContext, NodeParams, PortRef};

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
    /// Shape width in pixels. Ignored for polygon geometry.
    #[meta(min = 1, step = 1)]
    pub width: i64,
    /// Shape height in pixels. Ignored for polygon geometry.
    #[meta(min = 1, step = 1)]
    pub height: i64,
    /// Corner radius for rectangle geometry.
    #[meta(min = 0, step = 1)]
    pub border_radius: f64,
    /// Polygon vertices in pixels relative to `position`, as `x,y; x,y` (3 to 128 points).
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
    /// Stroke width in pixels.
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
    pub(crate) params_buffer: lumen_gpu::BufferId,
    pub(crate) points_buffer: lumen_gpu::BufferId,
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
        let vertices =
            parse_polygon_points(&evaluated.polygon_points, MAX_PATH_POINTS).unwrap_or_default();
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
            _pad: 0,
        };
        bound.write_buffer(self.params_buffer, 0, bytemuck::bytes_of(&params));
        if !points.is_empty() {
            bound.write_buffer(self.points_buffer, 0, bytemuck::cast_slice(&points));
        }
        Ok(())
    }
}

fn shape_uniforms(params: &ShapeParams) -> ShapeUniforms {
    let (x, y) = params.position;
    ShapeUniforms {
        fill_paint: params.fill_paint.to_gpu([255, 255, 255, 255]),
        stroke_paint: params.stroke_paint.to_gpu([0, 0, 0, 255]),
        position: [x as f32, y as f32],
        size: [params.width.max(1) as f32, params.height.max(1) as f32],
        border_radius: params.border_radius as f32,
        stroke_width: params.stroke_width as f32,
        geometry_kind: params.geometry_kind as u32,
        flags: shape_flags(params),
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
/// well-formed and has 3 to `max_points` finite points.
fn parse_polygon_points(text: &str, max_points: usize) -> Option<Vec<[f32; 2]>> {
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
    (3..=max_points)
        .contains(&vertices.len())
        .then_some(vertices)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_semicolon_and_newline_separated_points() {
        assert_eq!(
            parse_polygon_points("0,0; 10.5, 0\n 5,-8;", 128),
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
                parse_polygon_points(text, 128).is_none(),
                "{text:?} should draw nothing"
            );
        }
        assert!(parse_polygon_points("0,0; 1,0; 1,1; 0,1", 3).is_none());
    }
}
