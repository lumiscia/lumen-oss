use crate::gpu::{BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuFrameBinding};
use crate::node::{Deferred, NodeId, NodeParams, PortRef};

#[derive(Debug, Clone, Copy, PartialEq, Eq, lumen_macros::NodeEnum)]
#[repr(i64)]
pub enum ShapeGeometryKind {
    Rectangle = 0,
    Ellipse = 1,
    Polygon = 2,
}

impl ShapeGeometryKind {
    pub fn from_int(value: i64) -> Self {
        match value {
            1 => Self::Ellipse,
            2 => Self::Polygon,
            _ => Self::Rectangle,
        }
    }
}

/// Produces a vector shape layer for GPU rasterization.
#[derive(Debug, Clone, lumen_macros::NodeParams)]
#[params(evaluated = EvaluatedShapeParams)]
#[cfg_attr(feature = "json", derive(serde::Deserialize), serde(default))]
pub struct ShapeParams {
    /// Geometry primitive to rasterize.
    #[param(kind = "enum", enum_type = ShapeGeometryKind)]
    pub geometry_kind: Deferred<i64>,
    /// Shape width in pixels.
    #[param(kind = "int", min = 1, step = 1)]
    pub width: Deferred<i64>,
    /// Shape height in pixels.
    #[param(kind = "int", min = 1, step = 1)]
    pub height: Deferred<i64>,
    /// Corner radius for rectangle geometry.
    #[param(kind = "float", min = 0, step = 1)]
    pub border_radius: Deferred<f64>,
    /// Polygon point list formatted as `x,y; x,y`.
    #[param(
        kind = "string",
        name = "Polygon points",
        format = "point_list",
        multiline,
        recommended_rows = 3
    )]
    pub polygon_points: Deferred<String>,
    /// Shape origin in pixels.
    #[param(kind = "vec2")]
    pub position: Deferred<(f64, f64)>,
    /// Enables fill rendering.
    #[param(kind = "bool")]
    pub fill_enabled: Deferred<bool>,
    /// Fill color.
    #[param(kind = "color")]
    pub fill_color: Deferred<[u8; 4]>,
    /// Enables stroke rendering.
    #[param(kind = "bool")]
    pub stroke_enabled: Deferred<bool>,
    /// Stroke color.
    #[param(kind = "color")]
    pub stroke_color: Deferred<[u8; 4]>,
    /// Stroke width in pixels.
    #[param(kind = "float", min = 0, step = 0.5)]
    pub stroke_width: Deferred<f64>,
}

impl Default for ShapeParams {
    fn default() -> Self {
        Self {
            geometry_kind: Deferred::value(ShapeGeometryKind::Rectangle as i64),
            width: Deferred::value(1),
            height: Deferred::value(1),
            border_radius: Deferred::value(0.0),
            polygon_points: Deferred::value(String::new()),
            position: Deferred::value((0.0, 0.0)),
            fill_enabled: Deferred::value(true),
            fill_color: Deferred::value([255, 255, 255, 255]),
            stroke_enabled: Deferred::value(false),
            stroke_color: Deferred::value([0, 0, 0, 255]),
            stroke_width: Deferred::value(1.0),
        }
    }
}

/// Produces a vector shape layer for GPU rasterization.
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(kind = "shape", name = "Shape", category = "vector")]
pub struct Shape {
    pub id: NodeId,
    #[params]
    pub params: ShapeParams,
}

impl Default for Shape {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            params: ShapeParams::default(),
        }
    }
}

impl GpuCompileNode for Shape {
    fn compile_gpu(
        &self,
        ctx: &mut crate::gpu::CompileContext<'_>,
        port: &PortRef,
    ) -> crate::Result<CompiledOutput> {
        crate::node::vector::renderer::VectorRenderer::new(ctx).compile_shape(self, port)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ShapeFrameBinding {
    pub(crate) node_id: NodeId,
    pub(crate) geometry_kind: Deferred<i64>,
    pub(crate) width: Deferred<i64>,
    pub(crate) height: Deferred<i64>,
    pub(crate) border_radius: Deferred<f64>,
    pub(crate) position: Deferred<(f64, f64)>,
    pub(crate) fill_enabled: Deferred<bool>,
    pub(crate) fill_color: Deferred<[u8; 4]>,
    pub(crate) stroke_enabled: Deferred<bool>,
    pub(crate) stroke_color: Deferred<[u8; 4]>,
    pub(crate) stroke_width: Deferred<f64>,
    pub(crate) buffer: lumen_gpu::BufferId,
}

impl GpuFrameBinding for ShapeFrameBinding {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<()> {
        let (x, y) = self.position.resolve_vec2(
            self.node_id,
            "position",
            &ctx.expr_context(self.node_id, "position"),
        )?;
        let fill = self.fill_color.resolve_color(
            self.node_id,
            "fill_color",
            &ctx.expr_context(self.node_id, "fill_color"),
        )?;
        let stroke = self.stroke_color.resolve_color(
            self.node_id,
            "stroke_color",
            &ctx.expr_context(self.node_id, "stroke_color"),
        )?;
        let mut flags = 0;
        if self.fill_enabled.resolve_bool(
            self.node_id,
            "fill_enabled",
            &ctx.expr_context(self.node_id, "fill_enabled"),
        )? {
            flags |= 1;
        }
        if self.stroke_enabled.resolve_bool(
            self.node_id,
            "stroke_enabled",
            &ctx.expr_context(self.node_id, "stroke_enabled"),
        )? {
            flags |= 2;
        }
        let params = super::renderer::ShapeParams {
            fill_color: rgba8_to_f32(fill),
            stroke_color: rgba8_to_f32(stroke),
            position: [x as f32, y as f32],
            size: [
                self.width
                    .resolve_int(
                        self.node_id,
                        "width",
                        &ctx.expr_context(self.node_id, "width"),
                    )?
                    .max(1) as f32,
                self.height
                    .resolve_int(
                        self.node_id,
                        "height",
                        &ctx.expr_context(self.node_id, "height"),
                    )?
                    .max(1) as f32,
            ],
            border_radius: self.border_radius.resolve_float(
                self.node_id,
                "border_radius",
                &ctx.expr_context(self.node_id, "border_radius"),
            )? as f32,
            stroke_width: self.stroke_width.resolve_float(
                self.node_id,
                "stroke_width",
                &ctx.expr_context(self.node_id, "stroke_width"),
            )? as f32,
            geometry_kind: ShapeGeometryKind::from_int(self.geometry_kind.resolve_int(
                self.node_id,
                "geometry_kind",
                &ctx.expr_context(self.node_id, "geometry_kind"),
            )?) as u32,
            flags,
        };
        bound.write_buffer(self.buffer, 0, bytemuck::bytes_of(&params));
        Ok(())
    }
}

fn rgba8_to_f32(color: [u8; 4]) -> [f32; 4] {
    [
        f32::from(color[0]) / 255.0,
        f32::from(color[1]) / 255.0,
        f32::from(color[2]) / 255.0,
        f32::from(color[3]) / 255.0,
    ]
}
