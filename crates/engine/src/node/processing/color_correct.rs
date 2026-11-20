use bytemuck::{Pod, Zeroable};

use super::color_curves::{ColorCurves, MAX_CURVE_POINTS};
use crate::gpu::frame_graph::ComputeProgram;
use crate::node::{DelegateEvalContext, DelegateEvaluable};
use crate::node::{NodeId, NodeParams};

pub(crate) const SHADER: &str = concat!(
    include_str!("../../gpu/shaders/alpha.wgsl"),
    include_str!("../../gpu/shaders/color.wgsl"),
    include_str!("color_correct.wgsl")
);
const COLOR_CORRECT: ComputeProgram = ComputeProgram::spatial("color_correct", SHADER);

/// Smallest gamma the node divides by.
const MIN_GAMMA: f64 = 0.01;
/// Smallest pivot the contrast divides by.
const MIN_PIVOT: f64 = 0.001;

#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct ColorCorrectParams {
    /// Colour temperature. Positive warms, scaling red up and blue down by half a stop at 1;
    /// negative cools.
    #[meta(min = -1, max = 1, step = 0.01)]
    pub temperature: f64,
    /// Green–magenta tint. Positive shifts toward magenta, scaling green down by half a stop at
    /// 1; negative shifts toward green.
    #[meta(min = -1, max = 1, step = 0.01)]
    pub tint: f64,
    /// Raises the darks of each channel by this much, leaving white where it is.
    #[meta(step = 0.01)]
    pub lift: [f64; 3],
    /// Midtone gamma of each channel: above 1 brightens the midtones. Each is at least 0.01.
    #[meta(step = 0.01)]
    pub gamma: [f64; 3],
    /// Multiplier of each channel's linear light.
    #[meta(step = 0.01)]
    pub gain: [f64; 3],
    /// Linear light added to each channel after the gain.
    #[meta(step = 0.01)]
    pub offset: [f64; 3],
    /// Contrast around the pivot, in linear light: above 1 pushes values away from it.
    #[meta(min = 0, step = 0.01)]
    pub contrast: f64,
    /// Linear light the contrast leaves unchanged, 0.18 for mid grey.
    #[meta(min = 0.001, step = 0.01)]
    pub pivot: f64,
    /// Tone curves on the sRGB-encoded colour, applied last.
    #[meta()]
    pub curves: ColorCurves,
}

impl Default for ColorCorrectParams {
    fn default() -> Self {
        Self {
            temperature: 0.0,
            tint: 0.0,
            lift: [0.0; 3],
            gamma: [1.0; 3],
            gain: [1.0; 3],
            offset: [0.0; 3],
            contrast: 1.0,
            pivot: 0.18,
            curves: ColorCurves::default(),
        }
    }
}

/// Corrects the colour of a raster. Each step works on the straight colour in linear light, in
/// this order, and the alpha is untouched, so the defaults leave the raster unchanged.
///
/// 1. White balance multiplies red, green and blue by `2^(temperature / 2)`, `2^(-tint / 2)` and
///    `2^(-temperature / 2)`, divided by the Rec. 709 luminance of those three, so a neutral grey
///    keeps its brightness.
///
/// 2. Lift, gain and offset: `gain * (x + lift * (1 - x)) + offset` on each channel.
///
/// 3. Gamma: `max(x, 0) ^ (1 / gamma)` on each channel.
///
/// 4. Contrast: `pivot * (max(x, 0) / pivot) ^ contrast` on each channel.
///
/// 5. Curves act on the sRGB-encoded colour, clamped to 0..1: `master` on every channel, then
///    `red`, `green` and `blue` on theirs. Each is a smooth monotone curve through its points, flat
///    before the first and after the last. With no curves set, values above 1 pass through.
///
/// Each channel param is `[red, green, blue]`; an expression for one is a `vec3`. A curve is up
/// to 16 `[x, y]` points of numbers or expressions, with `x` increasing, all in 0..1:
/// `"curves": { "master": [[0, 0], [0.5, 0.65], [1, 1]] }` brightens the midtones.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(
    kind = "color_correct",
    name = "Color Correct",
    category = "processing"
)]
pub struct ColorCorrect {
    pub id: NodeId,
    #[params]
    pub params: ColorCorrectParamsDelegate,
    #[input()]
    pub source: Option<NodeId>,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable, lumen_macros::ShaderLayout)]
#[shader_layout(wgsl = "ColorCorrectParams", source = SHADER)]
struct ColorCorrectUniforms {
    white_balance: [f32; 4],
    lift: [f32; 4],
    gamma: [f32; 4],
    gain: [f32; 4],
    offset: [f32; 4],
    contrast: f32,
    pivot: f32,
    curved: u32,
    _pad: u32,
    curve_counts: [u32; 4],
    curve_points: [[f32; 4]; 4 * MAX_CURVE_POINTS],
}

/// `rgb` as the first three lanes of a uniform vector.
fn vec4(rgb: [f64; 3]) -> [f32; 4] {
    let [r, g, b] = rgb.map(|channel| channel as f32);
    [r, g, b, 0.0]
}

/// The white balance multipliers of `temperature` and `tint`, scaled to keep the luminance of
/// grey.
fn white_balance(temperature: f64, tint: f64) -> [f64; 3] {
    let factors = [
        (0.5 * temperature).exp2(),
        (-0.5 * tint).exp2(),
        (-0.5 * temperature).exp2(),
    ];
    let luminance = 0.2126 * factors[0] + 0.7152 * factors[1] + 0.0722 * factors[2];
    factors.map(|factor| factor / luminance)
}

impl crate::gpu::frame_graph::RecordNode for ColorCorrect {
    fn programs(&self, programs: &mut crate::gpu::frame_graph::Programs<'_>) -> crate::Result<()> {
        programs.compute(&COLOR_CORRECT)
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
        let (curve_counts, curve_points) = params.curves.gpu();
        let gpu_params = ColorCorrectUniforms {
            white_balance: vec4(white_balance(params.temperature, params.tint)),
            lift: vec4(params.lift),
            gamma: vec4(params.gamma.map(|gamma| gamma.max(MIN_GAMMA))),
            gain: vec4(params.gain),
            offset: vec4(params.offset),
            contrast: params.contrast.max(0.0) as f32,
            pivot: params.pivot.max(MIN_PIVOT) as f32,
            curved: u32::from(curve_counts.iter().any(|&count| count > 0)),
            _pad: 0,
            curve_counts,
            curve_points,
        };
        cx.filter(&COLOR_CORRECT, source, &gpu_params)
    }

    fn magnification(&self) -> f64 {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn white_balance_keeps_the_luminance_of_grey() {
        for (temperature, tint) in [(0.0, 0.0), (1.0, 0.0), (-1.0, 0.5), (0.3, -1.0)] {
            let [r, g, b] = white_balance(temperature, tint);
            let luminance = 0.2126 * r + 0.7152 * g + 0.0722 * b;
            assert!((luminance - 1.0).abs() < 1e-12, "{temperature} {tint}");
        }
        let [r, _, b] = white_balance(1.0, 0.0);
        assert!(r > 1.0 && b < 1.0);
    }
}
