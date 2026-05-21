use super::paint::{Paint, PaintDelegate};
use crate::gpu::{BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuFrameBinding};
use crate::node::{Deferred, DelegateEvalContext, NodeId, NodeParams, PortRef};

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
#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct ShapeParams {
    /// Geometry primitive to rasterize.
    #[meta(kind = "enum", enum_type = ShapeGeometryKind)]
    pub geometry_kind: i64,
    /// Shape width in pixels.
    #[meta(min = 1, step = 1)]
    pub width: i64,
    /// Shape height in pixels.
    #[meta(min = 1, step = 1)]
    pub height: i64,
    /// Corner radius for rectangle geometry.
    #[meta(min = 0, step = 1)]
    pub border_radius: f64,
    /// Polygon point list formatted as `x,y; x,y`.
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
    /// Fill color.
    #[meta()]
    pub fill_color: [u8; 4],
    /// Fill paint. Accepts a solid color or gradient.
    #[meta()]
    pub fill_paint: Paint,
    /// Enables stroke rendering.
    #[meta()]
    pub stroke_enabled: bool,
    /// Stroke color.
    #[meta()]
    pub stroke_color: [u8; 4],
    /// Stroke paint. Accepts a solid color or gradient.
    #[meta()]
    pub stroke_paint: Paint,
    /// Stroke width in pixels.
    #[meta(min = 0, step = 0.5)]
    pub stroke_width: f64,
}

impl Default for ShapeParams {
    fn default() -> Self {
        Self {
            geometry_kind: ShapeGeometryKind::Rectangle as i64,
            width: 1,
            height: 1,
            border_radius: 0.0,
            polygon_points: String::new(),
            position: (0.0, 0.0),
            fill_enabled: true,
            fill_color: [255, 255, 255, 255],
            fill_paint: Paint::solid([255, 255, 255, 255]),
            stroke_enabled: false,
            stroke_color: [0, 0, 0, 255],
            stroke_paint: Paint::solid([0, 0, 0, 255]),
            stroke_width: 1.0,
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
    pub(crate) fill_paint: PaintDelegate,
    pub(crate) stroke_enabled: Deferred<bool>,
    pub(crate) stroke_color: Deferred<[u8; 4]>,
    pub(crate) stroke_paint: PaintDelegate,
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
        let fill_paint = self.fill_paint.try_into_evaluated(&DelegateEvalContext {
            node_id: self.node_id,
            property_path: "fill_paint",
            expr: &ctx.expr_context(self.node_id, "fill_paint"),
        })?;
        let stroke_paint = self.stroke_paint.try_into_evaluated(&DelegateEvalContext {
            node_id: self.node_id,
            property_path: "stroke_paint",
            expr: &ctx.expr_context(self.node_id, "stroke_paint"),
        })?;
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
            fill_paint: fill_paint.to_gpu(fill),
            stroke_paint: stroke_paint.to_gpu(stroke),
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
