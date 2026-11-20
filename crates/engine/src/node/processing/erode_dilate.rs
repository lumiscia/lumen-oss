use bytemuck::{Pod, Zeroable};

use crate::gpu::frame_graph::{ComputeProgram, RECORD_STORAGE_TEXTURE, Raster, dispatch_for};
use crate::node::{DelegateEvalContext, DelegateEvaluable, NodeId, NodeParams};

const SHADER: &str = include_str!("erode_dilate.wgsl");

/// Where a distance is measured from, and so the shape a grown or shrunk edge takes.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, lumen_macros::NodeEnum, lumen_macros::Delegate,
)]
#[repr(i64)]
#[delegate(kind = "enum", expressions)]
pub enum ErodeDilateShape {
    /// Straight-line distance, so corners round off.
    #[default]
    Round = 0,
    /// The larger of the horizontal and vertical distance, so corners stay square.
    Square = 1,
}

#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct ErodeDilateParams {
    /// How far the alpha edge moves, in pixels: positive grows it, negative shrinks it. 0 leaves
    /// the edge where it is.
    #[meta(step = 0.5, pixels)]
    pub radius: f64,
    /// Whether corners round off or stay square.
    #[meta()]
    pub shape: ErodeDilateShape,
    /// Width in pixels of the fade that feathers the moved edge. 0 gives a hard edge, one pixel
    /// wide.
    #[meta(min = 0, step = 0.5, pixels)]
    pub softness: f64,
}

impl Default for ErodeDilateParams {
    fn default() -> Self {
        Self {
            radius: 2.0,
            shape: ErodeDilateShape::Round,
            softness: 0.0,
        }
    }
}

/// Grows or shrinks the opaque part of a raster by `radius` pixels, for pulling a key's edge in,
/// widening a matte so a blur covers the whole object, or outlining a letter that is already
/// pixels.
///
/// A pixel counts as part of the shape when its alpha is at least 0.5. A positive `radius` makes
/// every pixel within that distance of the shape opaque, in the colour of the nearest pixel of the
/// shape, so the result composites without a dark fringe. A negative `radius` makes every pixel
/// within that distance of transparency, or of an edge of the raster's area that is not an edge of
/// the canvas, transparent, and keeps the colour of the pixels that remain. Alpha is never
/// lowered by growing nor raised by shrinking, and `softness` fades the new edge over that many
/// pixels. Growing widens the output past the source's area by the radius; shrinking keeps the
/// source's area. With `radius` and `softness` 0, the raster is unchanged.
///
/// For example, `radius` 3 grows a key's matte by 3 pixels, and merging the original over a
/// `radius` 4 result outlines a rasterized letter.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(
    kind = "erode_dilate",
    name = "Erode / Dilate",
    category = "processing"
)]
pub struct ErodeDilate {
    pub id: NodeId,
    #[params]
    pub params: ErodeDilateParamsDelegate,
    #[input()]
    pub source: Option<NodeId>,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable, lumen_macros::ShaderLayout)]
#[shader_layout(wgsl = "ErodeDilateParams", source = SHADER)]
struct GpuErodeDilateParams {
    /// Radius magnitude and softness in device pixels, and 1 for the square shape.
    values: [f32; 4],
    /// Source texel minus output texel in `xy`, the jump step, and 1 when eroding.
    offset: [i32; 4],
    /// 1 on the left, top, right and bottom sides past which the source reads as transparent.
    edges: [i32; 4],
}

const SEEDS_FORMAT: lumen_gpu::wgpu::TextureFormat = lumen_gpu::wgpu::TextureFormat::Rg32Uint;

const SEED: ComputeProgram = ComputeProgram {
    label: "erode-dilate",
    shader: SHADER,
    entry: "seed_main",
    layout: &[
        lumen_gpu::LayoutEntry::compute(0, lumen_gpu::LayoutKind::Texture),
        lumen_gpu::LayoutEntry::compute(2, lumen_gpu::LayoutKind::Uniform),
        lumen_gpu::LayoutEntry::compute(3, lumen_gpu::LayoutKind::StorageTexture(SEEDS_FORMAT)),
    ],
};
const JUMP: ComputeProgram = ComputeProgram {
    label: "erode-dilate",
    shader: SHADER,
    entry: "jump_main",
    layout: &[
        lumen_gpu::LayoutEntry::compute(1, lumen_gpu::LayoutKind::UintTexture),
        lumen_gpu::LayoutEntry::compute(2, lumen_gpu::LayoutKind::Uniform),
        lumen_gpu::LayoutEntry::compute(3, lumen_gpu::LayoutKind::StorageTexture(SEEDS_FORMAT)),
    ],
};
const RESOLVE: ComputeProgram = ComputeProgram {
    label: "erode-dilate",
    shader: SHADER,
    entry: "resolve_main",
    layout: &[
        lumen_gpu::LayoutEntry::compute(0, lumen_gpu::LayoutKind::Texture),
        lumen_gpu::LayoutEntry::compute(1, lumen_gpu::LayoutKind::UintTexture),
        lumen_gpu::LayoutEntry::compute(2, lumen_gpu::LayoutKind::Uniform),
        lumen_gpu::LayoutEntry::compute(4, RECORD_STORAGE_TEXTURE),
    ],
};

fn finite_or(value: f64, fallback: f64) -> f64 {
    if value.is_finite() { value } else { fallback }
}

/// The jump distances of a flood that finds the nearest seed within `reach` pixels: halving from
/// the power of two that reaches it down to 1, then 1 again to repair the pixels the flood gets
/// wrong.
fn jump_steps(reach: u32) -> impl Iterator<Item = i32> {
    let first = reach.max(1).next_power_of_two();
    (0..=first.trailing_zeros())
        .map(move |halvings| (first >> halvings) as i32)
        .chain(std::iter::once(1))
}

impl crate::gpu::frame_graph::RecordNode for ErodeDilate {
    fn programs(&self, programs: &mut crate::gpu::frame_graph::Programs<'_>) -> crate::Result<()> {
        programs.compute(&SEED)?;
        programs.compute(&JUMP)?;
        programs.compute(&RESOLVE)
    }

    fn record(&self, cx: &mut crate::gpu::frame_graph::FrameCx<'_>) -> crate::Result<Raster> {
        let source = cx.required(self.id, "source", self.source)?;
        let params = self.params.eval(&DelegateEvalContext {
            node_id: self.id,
            expr: &cx.expr(),
            pixel_scale: cx.pixel_scale(),
        })?;
        let radius = finite_or(params.radius, 0.0);
        let softness = finite_or(params.softness, 0.0).max(0.0);
        if radius == 0.0 && softness == 0.0 {
            return Ok(source);
        }

        let eroding = radius < 0.0;
        let display = source.domain.display_size();
        let largest = f64::from(display.width.max(display.height));
        // Pixels from the shape's edge beyond which nothing changes, never more than the canvas.
        let reach = (radius.abs() + softness * 0.5 + 1.0).min(largest).ceil() as u32;

        let src = source.domain.data_rect;
        let domain = if eroding {
            source.domain
        } else {
            let grow = f64::from(reach);
            cx.domain_within(
                display,
                cx.bounds_of(Some(
                    [
                        f64::from(src.x) - grow,
                        f64::from(src.y) - grow,
                        f64::from(src.x) + f64::from(src.width) + grow,
                        f64::from(src.y) + f64::from(src.height) + grow,
                    ]
                    .map(|value| value as f32),
                )),
            )
        };
        let size = domain.storage_size;
        let edges = [
            src.x > 0,
            src.y > 0,
            i64::from(src.x) + i64::from(src.width) < i64::from(display.width),
            i64::from(src.y) + i64::from(src.height) < i64::from(display.height),
        ]
        .map(|past| i32::from(eroding && past));
        let gpu_params = |step: i32| GpuErodeDilateParams {
            values: [
                radius.abs() as f32,
                softness as f32,
                f32::from(u8::from(params.shape == ErodeDilateShape::Square)),
                0.0,
            ],
            offset: [
                domain.data_rect.x - src.x,
                domain.data_rect.y - src.y,
                step,
                i32::from(eroding),
            ],
            edges,
        };

        let usage = lumen_gpu::TextureDesc::storage(size, SEEDS_FORMAT).usage;
        let mut seeds = cx.rec.acquire(size, SEEDS_FORMAT, usage);
        let seed = cx.program(&SEED)?;
        let seed_uniform = cx.rec.uniform(&gpu_params(0));
        cx.dispatch(
            seed,
            &[
                (0, lumen_gpu::Res::Tex(source.tex)),
                (2, seed_uniform),
                (3, lumen_gpu::Res::Tex(seeds)),
            ],
            dispatch_for(size),
        );
        let jump = cx.program(&JUMP)?;
        for step in jump_steps(reach) {
            let next = cx.rec.acquire(size, SEEDS_FORMAT, usage);
            let step_uniform = cx.rec.uniform(&gpu_params(step));
            cx.dispatch(
                jump,
                &[
                    (1, lumen_gpu::Res::Tex(seeds)),
                    (2, step_uniform),
                    (3, lumen_gpu::Res::Tex(next)),
                ],
                dispatch_for(size),
            );
            seeds = next;
        }

        let tex = cx.storage_target(size);
        let resolve = cx.program(&RESOLVE)?;
        let resolve_uniform = cx.rec.uniform(&gpu_params(0));
        cx.dispatch(
            resolve,
            &[
                (0, lumen_gpu::Res::Tex(source.tex)),
                (1, lumen_gpu::Res::Tex(seeds)),
                (2, resolve_uniform),
                (4, lumen_gpu::Res::Tex(tex)),
            ],
            dispatch_for(size),
        );
        Ok(Raster {
            tex,
            domain,
            bounds: None,
            reduced: None,
        })
    }

    fn magnification(&self) -> f64 {
        1.0
    }
}
