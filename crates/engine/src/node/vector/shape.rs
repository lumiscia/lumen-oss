use crate::gpu::{BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuFrameBinding};
use crate::node::{NodeId, NodeProperty, PortRef};

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
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(kind = "shape", name = "Shape", category = "vector")]
pub struct Shape {
    pub id: NodeId,
    /// Geometry primitive to rasterize.
    #[property(kind = "enum", enum_type = ShapeGeometryKind)]
    pub geometry_kind: NodeProperty,
    /// Shape width in pixels.
    #[property(kind = "int", min = 1, step = 1)]
    pub width: NodeProperty,
    /// Shape height in pixels.
    #[property(kind = "int", min = 1, step = 1)]
    pub height: NodeProperty,
    /// Corner radius for rectangle geometry.
    #[property(kind = "float", min = 0, step = 1)]
    pub border_radius: NodeProperty,
    /// Polygon point list formatted as `x,y; x,y`.
    #[property(
        kind = "string",
        name = "Polygon points",
        format = "point_list",
        multiline,
        recommended_rows = 3
    )]
    pub polygon_points: NodeProperty,
    /// Shape origin in pixels.
    #[property(kind = "vec2")]
    pub position: NodeProperty,
    /// Enables fill rendering.
    #[property(kind = "bool")]
    pub fill_enabled: NodeProperty,
    /// Fill color.
    #[property(kind = "color")]
    pub fill_color: NodeProperty,
    /// Enables stroke rendering.
    #[property(kind = "bool")]
    pub stroke_enabled: NodeProperty,
    /// Stroke color.
    #[property(kind = "color")]
    pub stroke_color: NodeProperty,
    /// Stroke width in pixels.
    #[property(kind = "float", min = 0, step = 0.5)]
    pub stroke_width: NodeProperty,
}

impl Default for Shape {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            geometry_kind: NodeProperty::Int(ShapeGeometryKind::Rectangle as i64),
            width: NodeProperty::Int(1),
            height: NodeProperty::Int(1),
            border_radius: NodeProperty::Float(0.0),
            polygon_points: NodeProperty::String(String::new()),
            position: NodeProperty::Vec2((0.0, 0.0)),
            fill_enabled: NodeProperty::Bool(true),
            fill_color: NodeProperty::Color([255, 255, 255, 255]),
            stroke_enabled: NodeProperty::Bool(false),
            stroke_color: NodeProperty::Color([0, 0, 0, 255]),
            stroke_width: NodeProperty::Float(1.0),
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
    pub(crate) geometry_kind: NodeProperty,
    pub(crate) width: NodeProperty,
    pub(crate) height: NodeProperty,
    pub(crate) border_radius: NodeProperty,
    pub(crate) position: NodeProperty,
    pub(crate) fill_enabled: NodeProperty,
    pub(crate) fill_color: NodeProperty,
    pub(crate) stroke_enabled: NodeProperty,
    pub(crate) stroke_color: NodeProperty,
    pub(crate) stroke_width: NodeProperty,
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
