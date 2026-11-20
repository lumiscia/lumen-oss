use bytemuck::{Pod, Zeroable};

use crate::gpu::frame_graph::ComputeProgram;
use crate::node::{DelegateEvalContext, DelegateEvaluable, NodeId, NodeParams, OutputRef};

pub(crate) const SHADER: &str = concat!(
    include_str!("../../gpu/shaders/alpha.wgsl"),
    include_str!("../../gpu/shaders/color.wgsl"),
    include_str!("film_grain.wgsl")
);
const FILM_GRAIN: ComputeProgram = ComputeProgram::spatial("film-grain", SHADER);

/// How much coarser the second octave of grain is than the first.
const COARSE_FACTOR: f64 = 4.0;

#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct FilmGrainParams {
    /// Grain strength: the standard deviation of the grain in sRGB-encoded units, at the
    /// brightness where `response` makes it strongest.
    #[meta(min = 0, step = 0.01)]
    pub amount: f64,
    /// Size of one grain in composition pixels.
    #[meta(min = 0.5, step = 0.1)]
    pub size: f64,
    /// Mixes in a coarser grain four times the size, for a paper-like tooth, without changing the
    /// strength. 0 is fine grain only and 1 coarse grain only.
    #[meta(min = 0, max = 1, step = 0.05)]
    pub roughness: f64,
    /// How much the strength follows the brightness. 0 grains every brightness alike, and 1
    /// grains midtones only, leaving black and white clean.
    #[meta(min = 0, max = 1, step = 0.05)]
    pub response: f64,
    /// Uses one grain for all channels, which adds no colour noise, rather than one per channel.
    #[meta()]
    pub monochrome: bool,
    /// Picks an unrelated pattern. Use an expression such as `=frame` to animate the grain.
    #[meta(step = 1)]
    pub seed: i64,
}

impl Default for FilmGrainParams {
    fn default() -> Self {
        Self {
            amount: 0.15,
            size: 1.5,
            roughness: 0.0,
            response: 0.5,
            monochrome: true,
            seed: 0,
        }
    }
}

/// Adds film grain to a raster: noise that follows the picture's brightness, in a pattern fixed to
/// the canvas, so it stays put as the source moves and looks the same at any render scale. Alpha
/// is untouched. Set `seed` to `"=frame"` for grain that changes every frame, as in
/// `{ "amount": 0.1, "size": 2, "seed": "=frame" }`.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(kind = "film_grain", name = "Film Grain", category = "processing")]
pub struct FilmGrain {
    pub id: NodeId,
    #[params]
    pub params: FilmGrainParamsDelegate,
    #[input()]
    pub source: Option<OutputRef>,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable, lumen_macros::ShaderLayout)]
#[shader_layout(wgsl = "FilmGrainParams", source = SHADER)]
struct FilmGrainUniforms {
    grain: [f32; 4],
    cells: [f32; 4],
    origin: [i32; 4],
    modes: [u32; 4],
}

fn finite_or(value: f64, fallback: f64) -> f64 {
    if value.is_finite() { value } else { fallback }
}

impl crate::gpu::frame_graph::RecordNode for FilmGrain {
    fn programs(&self, programs: &mut crate::gpu::frame_graph::Programs<'_>) -> crate::Result<()> {
        programs.compute(&FILM_GRAIN)
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
        let size = finite_or(params.size, 1.5).max(0.5);
        // Lattice cells per device pixel: a cell is `size` composition pixels.
        let cells = 1.0 / (size * cx.pixel_scale());
        // Both halves of the seed count, so seeds a multiple of 2^32 apart differ.
        let (low, high) = (params.seed as u32, (params.seed >> 32) as u32);
        let seed = if high == 0 {
            low
        } else {
            low ^ high.wrapping_mul(0x9e37_79b9)
        };
        let data = source.domain.data_rect;
        let gpu_params = FilmGrainUniforms {
            grain: [
                amount as f32,
                finite_or(params.roughness, 0.0).clamp(0.0, 1.0) as f32,
                finite_or(params.response, 0.0).clamp(0.0, 1.0) as f32,
                0.0,
            ],
            cells: [cells as f32, (cells / COARSE_FACTOR) as f32, 0.0, 0.0],
            origin: [data.x, data.y, 0, 0],
            modes: [u32::from(params.monochrome), seed, 0, 0],
        };
        cx.filter(&FILM_GRAIN, source, &gpu_params)
    }

    fn magnification(&self) -> f64 {
        1.0
    }
}
