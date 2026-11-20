use bytemuck::{Pod, Zeroable};

use crate::gpu::frame_graph::{ComputeProgram, dispatch_for};
use crate::node::{DelegateEvalContext, DelegateEvaluable, NodeId, NodeParams};

use super::displacement_map::EdgeMode;

pub(crate) const SHADER: &str = concat!(
    include_str!("../../gpu/shaders/sampling.wgsl"),
    include_str!("lens_distortion.wgsl")
);

/// Which way the lens model maps the frame.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, lumen_macros::NodeEnum, lumen_macros::Delegate,
)]
#[repr(i64)]
#[delegate(kind = "enum", expressions)]
pub enum LensDirection {
    /// Applies the lens: the image bows as the coefficients say.
    #[default]
    Distort = 0,
    /// Removes the lens, the exact inverse of `distort` with the same coefficients.
    Undistort = 1,
}

/// How the distorted frame is scaled to the original frame.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, lumen_macros::NodeEnum, lumen_macros::Delegate,
)]
#[repr(i64)]
#[delegate(kind = "enum", expressions)]
pub enum LensFit {
    /// Keeps the scale. Pixels that read from beyond the frame follow `edge`.
    #[default]
    None = 0,
    /// Zooms in until every pixel reads from inside the frame. It never zooms out.
    Fill = 1,
}

#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct LensDistortionParams {
    /// Radial distortion, the `r²` coefficient. Positive values bow straight lines outward
    /// (barrel), negative values bow them inward (pincushion). Distances are measured from the
    /// optical centre in units of the frame's half-diagonal, so the corners are at 1.
    #[meta(step = 0.01)]
    pub k1: f64,
    /// Radial distortion, the `r⁴` coefficient. It bends mostly toward the corners.
    #[meta(step = 0.01)]
    pub k2: f64,
    /// The optical centre as a fraction of the frame: `[0.5, 0.5]` is the middle.
    #[meta()]
    pub center: (f64, f64),
    /// Whether to apply the lens or remove it.
    #[meta()]
    pub direction: LensDirection,
    /// How far red and blue scale apart from green, as a fraction of each pixel's distance from
    /// the optical centre. Positive values magnify red and shrink blue, so red fringes lie
    /// outside blue ones and grow toward the corners.
    #[meta(step = 0.001)]
    pub chromatic_aberration: f64,
    /// Whether to scale the result so that no edge is exposed.
    #[meta()]
    pub fit: LensFit,
    /// What the source reads as beyond its frame.
    #[meta()]
    pub edge: EdgeMode,
}

impl Default for LensDistortionParams {
    fn default() -> Self {
        Self {
            k1: 0.0,
            k2: 0.0,
            center: (0.5, 0.5),
            direction: LensDirection::Distort,
            chromatic_aberration: 0.0,
            fit: LensFit::None,
            edge: EdgeMode::Clamp,
        }
    }
}

/// Bends a raster the way a lens does. With `direction: "distort"`, a pixel at `p` from the optical
/// centre reads the source at `p * (1 + k1 r² + k2 r⁴)`, where `r` is its distance in
/// half-diagonals, so positive `k1` gives barrel distortion and negative `k1` pincushion. With
/// `direction: "undistort"` it reads where that lens would have put it, undoing a distortion with
/// the same coefficients. The result covers the source's frame, and looks the same at any render
/// scale.
///
/// To match graphics to footage, set the footage's coefficients and `direction: "undistort"` on
/// the footage, or leave `direction` as `"distort"` on the graphics. For a finishing look, try
/// `k1: 0.15` with `chromatic_aberration: 0.004`.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(
    kind = "lens_distortion",
    name = "Lens Distortion",
    category = "processing"
)]
pub struct LensDistortion {
    pub id: NodeId,
    #[params]
    pub params: LensDistortionParamsDelegate,
    #[input()]
    pub source: Option<NodeId>,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable, lumen_macros::ShaderLayout)]
#[shader_layout(wgsl = "LensParams", source = SHADER)]
struct GpuLensParams {
    lens: [f32; 4],
    frame: [f32; 4],
    modes: [u32; 4],
    origins: [i32; 4],
    source: [i32; 4],
    branch: [f32; 4],
}

const RECORD_LAYOUT: &[lumen_gpu::LayoutEntry] = &[
    lumen_gpu::LayoutEntry::compute(0, lumen_gpu::LayoutKind::Texture),
    lumen_gpu::LayoutEntry::compute(1, lumen_gpu::LayoutKind::Uniform),
    lumen_gpu::LayoutEntry::compute(2, crate::gpu::frame_graph::RECORD_STORAGE_TEXTURE),
];
const LENS_DISTORTION: ComputeProgram = ComputeProgram {
    label: "lens-distortion",
    shader: SHADER,
    entry: "cs_main",
    layout: RECORD_LAYOUT,
};

/// Params past this magnitude change nothing visible but would overflow the shader's math.
const MAX_PARAM: f64 = 1e6;

fn finite(value: f64) -> f64 {
    if value.is_finite() {
        value.clamp(-MAX_PARAM, MAX_PARAM)
    } else {
        0.0
    }
}

/// Points taken along each side of the frame to find how far the lens reaches beyond it.
const FIT_POINTS_PER_SIDE: usize = 16;
/// Iterations and the relative change that end the inverse lens's search, shared with the shader.
const NEWTON_ITERATIONS: usize = 32;
const NEWTON_TOLERANCE: f64 = 1e-6;
/// How far an output pixel's outermost subsample lies from its centre along each axis.
const SUBSAMPLE_REACH: f64 = 0.375;

/// The radius `rho * (1 + k1 rho² + k2 rho⁴)` the lens moves `rho` to.
fn lens_radius(rho: f64, k1: f64, k2: f64) -> f64 {
    let rho2 = rho * rho;
    rho * (1.0 + k1 * rho2 + k2 * rho2 * rho2)
}

/// The smallest radius at which the lens stops growing outward, where it starts folding back on
/// itself. Up to it the lens is one-to-one, so only radii below it are undistorted. `None` when it
/// never folds.
fn fold_radius(k1: f64, k2: f64) -> Option<f64> {
    // The slope `1 + 3 k1 u + 5 k2 u²` in `u = rho²`.
    let u = if k2 == 0.0 {
        (k1 < 0.0).then(|| -1.0 / (3.0 * k1))?
    } else {
        let discriminant = 9.0 * k1 * k1 - 20.0 * k2;
        if discriminant < 0.0 {
            return None;
        }
        let root = discriminant.sqrt();
        [
            (-3.0 * k1 - root) / (10.0 * k2),
            (-3.0 * k1 + root) / (10.0 * k2),
        ]
        .into_iter()
        .filter(|u| *u > 0.0)
        .reduce(f64::min)?
    };
    Some(u.sqrt())
}

/// Where a pixel at distance `r` reads: its offset from the optical centre is multiplied by this.
/// `None` where the inverse lens has no solution, past where the lens folds back on itself.
fn source_factor(r: f64, k1: f64, k2: f64, direction: LensDirection) -> Option<f64> {
    match direction {
        LensDirection::Distort => Some(1.0 + k1 * r * r + k2 * r.powi(4)),
        LensDirection::Undistort => {
            if r <= 0.0 {
                return Some(1.0);
            }
            let mut high = match fold_radius(k1, k2) {
                Some(fold) if r >= lens_radius(fold, k1, k2) => return None,
                Some(fold) => fold,
                None => {
                    let mut high = r.max(1.0);
                    for _ in 0..64 {
                        if lens_radius(high, k1, k2) >= r {
                            break;
                        }
                        high *= 2.0;
                    }
                    high
                }
            };
            // Newton's method kept inside a bracket on the one-to-one branch, bisecting when a
            // step would leave it.
            let mut low = 0.0;
            let mut rho = r.min(high);
            for _ in 0..NEWTON_ITERATIONS {
                let value = lens_radius(rho, k1, k2) - r;
                if value > 0.0 {
                    high = rho;
                } else {
                    low = rho;
                }
                let slope = 1.0 + 3.0 * k1 * rho * rho + 5.0 * k2 * rho.powi(4);
                let mut next = rho - value / slope;
                if !(next > low && next < high) {
                    next = 0.5 * (low + high);
                }
                let done = (next - rho).abs() <= NEWTON_TOLERANCE * next.abs();
                rho = next;
                if done {
                    break;
                }
            }
            Some(rho / r)
        }
    }
}

/// The scale about the optical centre, at most 1, under which 64 points along the boundary of the
/// frame, as far out as any output pixel's subsamples reach, read inside the frame's pixel
/// centres, where every sample's bilinear footprint is on source pixels.
fn fill_scale(
    display: [f64; 2],
    center: [f64; 2],
    half_diagonal: f64,
    [k1, k2, aberration]: [f64; 3],
    direction: LensDirection,
) -> f64 {
    let (low, high) = ([0.5; 2], [display[0] - 0.5, display[1] - 0.5]);
    let inset = 0.5 - SUBSAMPLE_REACH;
    let (out_low, out_high) = ([inset; 2], [display[0] - inset, display[1] - inset]);
    let mut scale: f64 = 1.0;
    let steps = FIT_POINTS_PER_SIDE as f64;
    for side in 0..4 {
        for step in 0..FIT_POINTS_PER_SIDE {
            let t = step as f64 / steps;
            let point = match side {
                0 => [out_low[0] + t * (out_high[0] - out_low[0]), out_low[1]],
                1 => [out_high[0], out_low[1] + t * (out_high[1] - out_low[1])],
                2 => [out_high[0] - t * (out_high[0] - out_low[0]), out_high[1]],
                _ => [out_low[0], out_high[1] - t * (out_high[1] - out_low[1])],
            };
            let offset = [point[0] - center[0], point[1] - center[1]];
            let r = offset[0].hypot(offset[1]) / half_diagonal;
            let Some(factor) = source_factor(r, k1, k2, direction) else {
                continue;
            };
            let reach = factor * (1.0 + aberration.abs());
            for axis in 0..2 {
                let moved = offset[axis] * reach;
                if moved > 0.0 {
                    scale = scale.min((high[axis] - center[axis]) / moved);
                } else if moved < 0.0 {
                    scale = scale.min((center[axis] - low[axis]) / -moved);
                }
            }
        }
    }
    scale.max(0.0)
}

impl crate::gpu::frame_graph::RecordNode for LensDistortion {
    fn programs(&self, programs: &mut crate::gpu::frame_graph::Programs<'_>) -> crate::Result<()> {
        programs.compute(&LENS_DISTORTION)
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
        let display = source.domain.display_size();
        let extent = [f64::from(display.width), f64::from(display.height)];
        let center = [
            finite(params.center.0) * extent[0],
            finite(params.center.1) * extent[1],
        ];
        let half_diagonal = extent[0].hypot(extent[1]) / 2.0;
        let (k1, k2, aberration) = (
            finite(params.k1),
            finite(params.k2),
            finite(params.chromatic_aberration),
        );
        let fit = match params.fit {
            LensFit::None => 1.0,
            LensFit::Fill => fill_scale(
                extent,
                center,
                half_diagonal,
                [k1, k2, aberration],
                params.direction,
            ),
        };
        let domain = cx.domain_within(display, None);
        let data = source.domain.data_rect;
        let gpu_params = GpuLensParams {
            lens: [k1 as f32, k2 as f32, aberration as f32, fit as f32],
            frame: [
                center[0] as f32,
                center[1] as f32,
                half_diagonal as f32,
                0.0,
            ],
            modes: [params.direction as u32, params.edge as u32, 0, 0],
            origins: [domain.data_rect.x, domain.data_rect.y, data.x, data.y],
            source: [display.width as i32, display.height as i32, 0, 0],
            branch: match fold_radius(k1, k2) {
                Some(fold) => [fold as f32, lens_radius(fold, k1, k2) as f32, 1.0, 0.0],
                None => [0.0; 4],
            },
        };
        let size = domain.storage_size;
        let tex = cx.storage_target(size);
        let program = cx.program(&LENS_DISTORTION)?;
        let uniform = cx.rec.uniform(&gpu_params);
        cx.dispatch(
            program,
            &[
                (0, lumen_gpu::Res::Tex(source.tex)),
                (1, uniform),
                (2, lumen_gpu::Res::Tex(tex)),
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn undistort(r: f64, k1: f64, k2: f64) -> Option<f64> {
        source_factor(r, k1, k2, LensDirection::Undistort).map(|factor| factor * r)
    }

    #[test]
    fn undistort_finds_the_root_before_the_fold() {
        let rho = undistort(0.95, 1.0, -1.0).expect("a root before the fold");
        assert!((lens_radius(rho, 1.0, -1.0) - 0.95).abs() < 1e-9);
        assert!(rho < fold_radius(1.0, -1.0).unwrap());
    }

    #[test]
    fn undistort_rejects_radii_past_the_fold() {
        assert_eq!(undistort(0.8, -3.0, 3.0), None);
    }
}
