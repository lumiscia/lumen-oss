use super::drawable::{DeferredTransform, DrawableMotion, DrawableTransform};
use super::paint::Paint;
use super::renderer::{ShapeUniforms, domain_origin, style_flags};
use crate::gpu::{
    BoundFrame, CompileContext, FrameBindContext, GpuCompileNode, GpuCompiledNode, RasterHandle,
    compiler,
};
use crate::node::motion::Shutter;
use crate::node::{DelegateEvalContext, DelegateEvaluable, NodeId, NodeParams};

const SHAPE_SHADER: &str = concat!(
    include_str!("../../gpu/shaders/alpha.wgsl"),
    include_str!("paint/paint.wgsl"),
    include_str!("drawable.wgsl"),
    include_str!("vector_common.wgsl"),
    include_str!("shape_renderer.wgsl")
);

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

/// Produces a vector shape layer for GPU rasterization.
#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct ShapeParams {
    /// Geometry primitive to rasterize.
    #[meta(kind = "enum", enum_type = ShapeGeometryKind)]
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
    /// Motion blur shutter angle in degrees, centred on the frame. 0 is off, 180 is the
    /// usual film look and 360 smears over the whole frame.
    #[meta(min = 0, max = 360, step = 1)]
    pub motion_blur: f64,
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
            width: 1.0,
            height: 1.0,
            border_radius: 0.0,
            position: (0.0, 0.0),
            scale: (1.0, 1.0),
            rotation: 0.0,
            anchor: (0.0, 0.0),
            opacity: 1.0,
            motion_blur: 0.0,
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
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(kind = "shape", name = "Shape", category = "vector")]
pub struct Shape {
    pub id: NodeId,
    #[params]
    pub params: ShapeParamsDelegate,
}

impl GpuCompileNode for Shape {
    fn compile_gpu(&self, ctx: &mut CompileContext<'_>) -> crate::Result<RasterHandle> {
        let bounds = ctx.source_bounds(
            compiler::SourceMotion::of(shape_is_animated(&self.params), &self.params),
            |expr| {
                shape_frame(&self.params, self.id, expr)
                    .map(|uniforms| Some(uniforms.screen_bounds))
            },
        );
        let domain = ctx.source_domain(bounds);
        let source = ctx.compute_source(
            self.id,
            "shape",
            "rasterize",
            SHAPE_SHADER,
            std::mem::size_of::<ShapeUniforms>() as u64,
            None,
            [],
            domain,
        );
        ctx.register_compiled_node(CompiledShape {
            node_id: self.id,
            params: self.params.clone(),
            buffer: source.params,
            origin: domain_origin(domain),
        });
        Ok(source.output)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CompiledShape {
    pub(crate) node_id: NodeId,
    pub(crate) params: ShapeParamsDelegate,
    pub(crate) buffer: lumen_gpu::BufferId,
    /// Canvas pixel of the output's first texel.
    pub(crate) origin: [i32; 2],
}

impl GpuCompiledNode for CompiledShape {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<()> {
        let mut params = shape_frame(&self.params, self.node_id, &ctx.expr_context())?;
        params.origin = self.origin;
        bound.write_buffer(self.buffer, 0, bytemuck::bytes_of(&params));
        Ok(())
    }
}

/// Evaluates a rectangle or ellipse shape's uniforms at the frame of `expr`.
fn shape_frame(
    params: &ShapeParamsDelegate,
    node_id: NodeId,
    expr: &crate::expr::ExpressionContext<'_>,
) -> crate::Result<ShapeUniforms> {
    let evaluated = params.eval(&DelegateEvalContext { node_id, expr })?;
    let shutter = shape_shutter(params, &evaluated, node_id, expr)?;
    Ok(shape_uniforms(&evaluated, shutter.as_ref()))
}

/// Whether a shape's canvas bounds may change between frames.
fn shape_is_animated(params: &ShapeParamsDelegate) -> bool {
    params.width.is_expr()
        || params.height.is_expr()
        || params.position.is_expr()
        || params.scale.is_expr()
        || params.rotation.is_expr()
        || params.anchor.is_expr()
        || params.motion_blur.is_expr()
        || params.fill_enabled.is_expr()
        || params.stroke_enabled.is_expr()
        || params.stroke_width.is_expr()
}

fn shape_uniforms(
    params: &ShapeParams,
    shutter: Option<&Shutter<DrawableTransform>>,
) -> ShapeUniforms {
    let position = [params.position.0 as f32, params.position.1 as f32];
    let size = [params.width.max(0.0) as f32, params.height.max(0.0) as f32];
    // Also catches NaN, which `max` turned to 0.
    let empty = size.contains(&0.0);
    // The shader draws anything thinner than a pixel a pixel across.
    let drawn = if empty {
        size
    } else {
        size.map(|side| side.max(1.0))
    };
    let transform = drawable_transform(params).to_gpu();
    // The stroke reaches half its width beyond the box, and antialiasing a canvas pixel.
    let margin = if params.stroke_enabled {
        (params.stroke_width.max(0.0) * 0.5) as f32
    } else {
        0.0
    };
    let min = [position[0] - margin, position[1] - margin];
    let max = [
        position[0] + drawn[0] + margin,
        position[1] + drawn[1] + margin,
    ];
    let motion = DrawableMotion::new(shutter, min, max);
    let [min_x, min_y, max_x, max_y] = motion.canvas_bounds(transform, min, max);
    ShapeUniforms {
        fill_paint: params.fill_paint.to_gpu(),
        stroke_paint: params.stroke_paint.to_gpu(),
        position,
        size,
        border_radius: params.border_radius.max(0.0) as f32,
        stroke_width: params.stroke_width as f32,
        geometry_kind: params.geometry_kind as u32,
        // Empty shapes disable fill and stroke instead of clamping to the shader's 1px minimum.
        flags: if empty { 0 } else { shape_flags(params) },
        transform,
        screen_bounds: [min_x - 1.0, min_y - 1.0, max_x + 1.0, max_y + 1.0],
        origin: [0, 0],
        _origin_pad: [0, 0],
        motion: motion.to_gpu(),
    }
}

fn shape_shutter(
    deferred: &ShapeParamsDelegate,
    params: &ShapeParams,
    node_id: NodeId,
    expr: &crate::expr::ExpressionContext<'_>,
) -> crate::Result<Option<Shutter<DrawableTransform>>> {
    DeferredTransform {
        position: &deferred.position,
        scale: &deferred.scale,
        rotation: &deferred.rotation,
        anchor: &deferred.anchor,
    }
    .shutter(
        params.motion_blur,
        drawable_transform(params),
        node_id,
        expr,
    )
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

fn shape_flags(params: &ShapeParams) -> u32 {
    style_flags(
        params.fill_enabled,
        params.stroke_enabled,
        params.edge_antialias,
    )
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
        assert_eq!(shape_uniforms(&params, None).border_radius, 0.0);
    }

    #[test]
    fn nan_and_negative_sizes_draw_nothing() {
        for width in [0.0, -1.0, f64::NAN] {
            let params = ShapeParams {
                width,
                ..ShapeParams::default()
            };
            assert_eq!(shape_uniforms(&params, None).flags, 0, "width {width}");
        }
    }
}
