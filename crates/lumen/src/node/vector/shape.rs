use crate::gpu::{
    BoundFrame, CompiledOutput, FrameBindContext, FrameBinding, GpuCompileNode, GpuFrameBindNode,
};
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

impl GpuFrameBindNode for Shape {
    fn bind_gpu_frame(
        &self,
        ctx: &FrameBindContext<'_>,
        binding: &FrameBinding,
        bound: &mut BoundFrame,
    ) -> crate::Result<()> {
        let FrameBinding::Shape {
            node_id,
            geometry_kind,
            width,
            height,
            border_radius,
            position,
            fill_enabled,
            fill_color,
            stroke_enabled,
            stroke_color,
            stroke_width,
            buffer,
        } = binding
        else {
            return Ok(());
        };
        let (x, y) = position.resolve_vec2(
            *node_id,
            "position",
            &ctx.expr_context(*node_id, "position"),
        )?;
        let fill = fill_color.resolve_color(
            *node_id,
            "fill_color",
            &ctx.expr_context(*node_id, "fill_color"),
        )?;
        let stroke = stroke_color.resolve_color(
            *node_id,
            "stroke_color",
            &ctx.expr_context(*node_id, "stroke_color"),
        )?;
        let mut flags = 0;
        if fill_enabled.resolve_bool(
            *node_id,
            "fill_enabled",
            &ctx.expr_context(*node_id, "fill_enabled"),
        )? {
            flags |= 1;
        }
        if stroke_enabled.resolve_bool(
            *node_id,
            "stroke_enabled",
            &ctx.expr_context(*node_id, "stroke_enabled"),
        )? {
            flags |= 2;
        }
        let params = super::renderer::ShapeParams {
            fill_color: rgba8_to_f32(fill),
            stroke_color: rgba8_to_f32(stroke),
            position: [x as f32, y as f32],
            size: [
                width
                    .resolve_int(*node_id, "width", &ctx.expr_context(*node_id, "width"))?
                    .max(1) as f32,
                height
                    .resolve_int(*node_id, "height", &ctx.expr_context(*node_id, "height"))?
                    .max(1) as f32,
            ],
            border_radius: border_radius.resolve_float(
                *node_id,
                "border_radius",
                &ctx.expr_context(*node_id, "border_radius"),
            )? as f32,
            stroke_width: stroke_width.resolve_float(
                *node_id,
                "stroke_width",
                &ctx.expr_context(*node_id, "stroke_width"),
            )? as f32,
            geometry_kind: ShapeGeometryKind::from_int(geometry_kind.resolve_int(
                *node_id,
                "geometry_kind",
                &ctx.expr_context(*node_id, "geometry_kind"),
            )?) as u32,
            flags,
        };
        bound.write_buffer(*buffer, 0, bytemuck::bytes_of(&params));
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
