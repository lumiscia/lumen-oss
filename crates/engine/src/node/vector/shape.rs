use super::drawable::{DrawableMotion, DrawablePlacement, DrawableStyle, DrawableTransform};
use super::renderer::{ShapeUniforms, domain_origin, style_flags};
use super::repeat::{GpuCopies, Repeat, RepeatedTransform};
use crate::gpu::params;
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
    #[meta(flatten, doc(position = "Shape origin in pixels."))]
    pub placement: DrawablePlacement,
    #[meta(
        flatten,
        doc(
            stroke_width = "Stroke width in untransformed pixels. Under non-uniform scale the stroke stretches\nwith the shape, so it is wider across the more scaled axis."
        )
    )]
    pub style: DrawableStyle,
    /// Copies of the shape drawn in one pass. Absent draws the shape once.
    #[meta()]
    pub repeat: Option<Repeat>,
}

impl Default for ShapeParams {
    fn default() -> Self {
        Self {
            geometry_kind: ShapeGeometryKind::Rectangle,
            width: 1.0,
            height: 1.0,
            border_radius: 0.0,
            placement: DrawablePlacement::default(),
            style: DrawableStyle::default(),
            repeat: None,
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

/// Evaluates a rectangle or ellipse shape's uniforms at the frame of `expr`, and its copies when
/// it repeats.
fn shape_frame(
    params: &ShapeParamsDelegate,
    node_id: NodeId,
    expr: &crate::expr::ExpressionContext<'_>,
) -> crate::Result<(ShapeUniforms, Option<GpuCopies>)> {
    let evaluated = params.eval(&DelegateEvalContext { node_id, expr })?;
    let shutter = params
        .placement
        .shutter(&evaluated.placement, node_id, expr)?;
    let mut uniforms = shape_uniforms(&evaluated, shutter.as_ref());
    let (Some(repeat), Some(delegate)) = (&evaluated.repeat, &params.repeat) else {
        return Ok((uniforms, None));
    };
    if !repeat.draws_copies() {
        return Ok((uniforms, None));
    }
    let repeat_shutter =
        delegate.shutter(repeat, evaluated.placement.motion_blur, node_id, expr)?;
    let (min, max) = shape_box(&evaluated);
    let copies = RepeatedTransform {
        base: &evaluated.placement.transform(),
        base_shutter: shutter.as_ref(),
        repeat,
        repeat_shutter: repeat_shutter.as_ref(),
    }
    .copies(min, max)
    // Antialiasing reaches a canvas pixel beyond each copy's box.
    .to_gpu(min, max, |_| 0.0, 1.0);
    uniforms.screen_bounds = copies.bounds;
    Ok((uniforms, Some(copies)))
}

/// The untransformed box around what the shape draws: the stroke reaches half its width beyond
/// the shape, which the shader draws at least a pixel across.
fn shape_box(params: &ShapeParams) -> ([f32; 2], [f32; 2]) {
    let position = [
        params.placement.position.0 as f32,
        params.placement.position.1 as f32,
    ];
    let size = [params.width.max(0.0) as f32, params.height.max(0.0) as f32];
    // Also catches NaN, which `max` turned to 0.
    let drawn = if size.contains(&0.0) {
        size
    } else {
        size.map(|side| side.max(1.0))
    };
    let margin = if params.style.stroke_enabled {
        (params.style.stroke_width.max(0.0) * 0.5) as f32
    } else {
        0.0
    };
    let min = [position[0] - margin, position[1] - margin];
    let max = [
        position[0] + drawn[0] + margin,
        position[1] + drawn[1] + margin,
    ];
    (min, max)
}

fn shape_uniforms(
    params: &ShapeParams,
    shutter: Option<&Shutter<DrawableTransform>>,
) -> ShapeUniforms {
    let style = &params.style;
    let position = [
        params.placement.position.0 as f32,
        params.placement.position.1 as f32,
    ];
    let size = [params.width.max(0.0) as f32, params.height.max(0.0) as f32];
    // Also catches NaN, which `max` turned to 0.
    let empty = size.contains(&0.0);
    let transform = params.placement.transform().to_gpu();
    let (min, max) = shape_box(params);
    let motion = DrawableMotion::new(shutter, min, max);
    let [min_x, min_y, max_x, max_y] = motion.canvas_bounds(transform, min, max);
    ShapeUniforms {
        fill_paint: style.fill_paint.to_gpu(),
        stroke_paint: style.stroke_paint.to_gpu(),
        position,
        size,
        border_radius: params.border_radius.max(0.0) as f32,
        stroke_width: style.stroke_width as f32,
        geometry_kind: params.geometry_kind as u32,
        // Empty shapes disable fill and stroke instead of clamping to the shader's 1px minimum.
        flags: if empty { 0 } else { shape_flags(params) },
        transform,
        // Antialiasing reaches a canvas pixel beyond the box.
        screen_bounds: [min_x - 1.0, min_y - 1.0, max_x + 1.0, max_y + 1.0],
        origin: [0, 0],
        _origin_pad: [0, 0],
        motion: motion.to_gpu(),
    }
}

fn shape_flags(params: &ShapeParams) -> u32 {
    style_flags(
        params.style.fill_enabled,
        params.style.stroke_enabled,
        params.style.edge_antialias,
    )
}

impl crate::gpu::frame_graph::RecordNode for Shape {
    fn programs(&self, programs: &mut crate::gpu::frame_graph::Programs<'_>) -> crate::Result<()> {
        programs.compute("shape", SHAPE_SHADER, "cs_main", RECORD_SHAPE_LAYOUT)?;
        if self.params.repeat.is_some() {
            programs.compute(
                "shape_repeat",
                SHAPE_SHADER,
                "cs_repeat",
                RECORD_SHAPE_REPEAT_LAYOUT,
            )?;
        }
        Ok(())
    }

    fn record(
        &self,
        cx: &mut crate::gpu::frame_graph::FrameCx<'_>,
    ) -> crate::Result<crate::gpu::frame_graph::Raster> {
        let (mut uniforms, copies) = shape_frame(&self.params, self.id, &cx.expr())?;
        record_shape_source(cx, &mut uniforms, copies.as_ref())
    }
}

const RECORD_SHAPE_LAYOUT: &[lumen_gpu::LayoutEntry] = &[
    lumen_gpu::LayoutEntry::compute(0, lumen_gpu::LayoutKind::Uniform),
    lumen_gpu::LayoutEntry::compute(1, crate::gpu::frame_graph::RECORD_STORAGE_TEXTURE),
];

const RECORD_SHAPE_REPEAT_LAYOUT: &[lumen_gpu::LayoutEntry] = &[
    lumen_gpu::LayoutEntry::compute(0, lumen_gpu::LayoutKind::Uniform),
    lumen_gpu::LayoutEntry::compute(1, crate::gpu::frame_graph::RECORD_STORAGE_TEXTURE),
    lumen_gpu::LayoutEntry::compute(2, lumen_gpu::LayoutKind::Storage),
    lumen_gpu::LayoutEntry::compute(3, lumen_gpu::LayoutKind::Storage),
    lumen_gpu::LayoutEntry::compute(4, lumen_gpu::LayoutKind::Storage),
];

/// Records a rectangle or ellipse, sized to what its uniforms draw this frame, or its copies.
fn record_shape_source(
    cx: &mut crate::gpu::frame_graph::FrameCx<'_>,
    uniforms: &mut ShapeUniforms,
    copies: Option<&GpuCopies>,
) -> crate::Result<crate::gpu::frame_graph::Raster> {
    let bounds = cx.bounds_of(Some(uniforms.screen_bounds));
    let domain = cx.source_domain(bounds);
    uniforms.origin = domain_origin(domain);
    let size = domain.storage_size;
    let tex = cx.storage_target(size);
    let uniform = cx.rec.uniform(&*uniforms);
    if let Some(copies) = copies {
        let program = cx.program(
            "shape_repeat",
            SHAPE_SHADER,
            "cs_repeat",
            RECORD_SHAPE_REPEAT_LAYOUT,
        )?;
        let [copy_list, taps, tiles] = copies.storage(cx, domain);
        cx.dispatch(
            program,
            &[
                (0, uniform),
                (1, lumen_gpu::Res::Tex(tex)),
                (2, copy_list),
                (3, taps),
                (4, tiles),
            ],
            params::dispatch_for(size),
        );
    } else {
        let program = cx.program("shape", SHAPE_SHADER, "cs_main", RECORD_SHAPE_LAYOUT)?;
        cx.dispatch(
            program,
            &[(0, uniform), (1, lumen_gpu::Res::Tex(tex))],
            params::dispatch_for(size),
        );
    }
    Ok(crate::gpu::frame_graph::Raster {
        tex,
        domain,
        bounds: None,
    })
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
