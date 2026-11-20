use bytemuck::{Pod, Zeroable};

use crate::gpu::frame_graph::{ComputeProgram, dispatch_for};
use crate::node::{DelegateEvalContext, DelegateEvaluable, NodeId, NodeParams, OutputRef};

use super::gaussian;

const SHADER: &str = concat!(
    include_str!("../../gpu/shaders/color.wgsl"),
    include_str!("sharpen.wgsl")
);

#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct SharpenParams {
    /// Strength of the sharpening. 0 leaves the raster as it is; 1 adds the detail back once.
    #[meta(min = 0, step = 0.05)]
    pub amount: f64,
    /// Blur radius in pixels of the soft copy the detail is measured against, as on `blur`. Small
    /// radii sharpen fine texture, large ones broad edges. 0 leaves the raster as it is.
    #[meta(min = 0, step = 0.1)]
    pub radius: f64,
    /// The smallest local contrast that is sharpened, from 0 to 1, in sRGB-encoded luminance. Finer
    /// detail, such as noise, is left alone, and stronger detail is sharpened by what exceeds it.
    #[meta(min = 0, max = 1, step = 0.005)]
    pub threshold: f64,
}

impl Default for SharpenParams {
    fn default() -> Self {
        Self {
            amount: 1.0,
            radius: 1.5,
            threshold: 0.0,
        }
    }
}

/// Sharpens a raster with an unsharp mask: detail is the difference between a pixel's luminance
/// and that of a blurred copy, and sharpening adds `amount` times it back.
///
/// It works on straight colour, with luminance (Rec. 709) on the sRGB-encoded values. The detail
/// is `luminance - blurred luminance`, reduced towards 0 by `threshold`, and the sharpened
/// luminance is `luminance + amount * detail`. That is held between the darkest and brightest
/// luminance in the 3×3 pixels around it, so hard edges get no light or dark halo. The change is
/// added equally to red, green and blue, which keeps the hue and adds no colour fringes; colour
/// brighter than white stays unclipped. Alpha is unchanged and the output covers the source's
/// area. For example, `amount` 1.5 with `radius` 2 and `threshold` 0.02 firms up edges and fine
/// texture while leaving faint grain alone.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(kind = "sharpen", name = "Sharpen", category = "processing")]
pub struct Sharpen {
    pub id: NodeId,
    #[params]
    pub params: SharpenParamsDelegate,
    #[input()]
    pub source: Option<OutputRef>,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable, lumen_macros::ShaderLayout)]
#[shader_layout(wgsl = "SharpenParams", source = SHADER)]
struct GpuSharpenParams {
    /// Amount and threshold.
    values: [f32; 4],
    /// Canvas pixel of the blur's first texel minus that of the source's, in `xy`.
    blur_offset: [i32; 4],
}

const COMPOSITE_LAYOUT: &[lumen_gpu::LayoutEntry] = &[
    lumen_gpu::LayoutEntry::compute(0, lumen_gpu::LayoutKind::Texture),
    lumen_gpu::LayoutEntry::compute(1, lumen_gpu::LayoutKind::Texture),
    lumen_gpu::LayoutEntry::compute(2, lumen_gpu::LayoutKind::Uniform),
    lumen_gpu::LayoutEntry::compute(4, crate::gpu::frame_graph::RECORD_STORAGE_TEXTURE),
];
const COMPOSITE: ComputeProgram = ComputeProgram {
    label: "sharpen",
    shader: SHADER,
    entry: "composite_main",
    layout: COMPOSITE_LAYOUT,
};

fn finite_or(value: f64, fallback: f64) -> f64 {
    if value.is_finite() { value } else { fallback }
}

impl crate::gpu::frame_graph::RecordNode for Sharpen {
    fn programs(&self, programs: &mut crate::gpu::frame_graph::Programs<'_>) -> crate::Result<()> {
        gaussian::programs(programs, false)?;
        programs.compute(&COMPOSITE)
    }

    fn record(
        &self,
        cx: &mut crate::gpu::frame_graph::FrameCx<'_>,
    ) -> crate::Result<crate::gpu::frame_graph::Raster> {
        let source = cx.required(self.id, "source", self.source)?;
        let params = self.params.eval(&DelegateEvalContext {
            node_id: self.id,
            expr: &cx.expr(),
            pixel_scale: cx.pixel_scale(),
        })?;
        let amount = finite_or(params.amount, 0.0).max(0.0);
        let radius = finite_or(params.radius, 0.0).max(0.0);
        if amount == 0.0 || radius == 0.0 {
            return Ok(source);
        }

        let domain = source.domain;
        let blur_domain = cx.domain_within(
            domain.display_size(),
            cx.bounds_of(gaussian::blur_bounds(
                domain.data_rect,
                [0.0; 2],
                radius,
                cx.pixel_scale(),
            )),
        );
        let blurred = gaussian::record_passes(
            cx,
            source,
            blur_domain,
            false,
            &gaussian::GaussianSettings {
                radius,
                transparent_edge: false,
                offset: [0.0; 2],
            },
        )?;
        let uniform = cx.rec.uniform(&GpuSharpenParams {
            values: [
                amount as f32,
                finite_or(params.threshold, 0.0).clamp(0.0, 1.0) as f32,
                0.0,
                0.0,
            ],
            blur_offset: [
                blur_domain.data_rect.x - domain.data_rect.x,
                blur_domain.data_rect.y - domain.data_rect.y,
                0,
                0,
            ],
        });
        let size = domain.storage_size;
        let tex = cx.storage_target(size);
        let program = cx.program(&COMPOSITE)?;
        cx.dispatch(
            program,
            &[
                (0, lumen_gpu::Res::Tex(source.tex)),
                (1, lumen_gpu::Res::Tex(blurred)),
                (2, uniform),
                (4, lumen_gpu::Res::Tex(tex)),
            ],
            dispatch_for(size),
        );
        Ok(crate::gpu::frame_graph::Raster {
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
