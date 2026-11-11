use bytemuck::{Pod, Zeroable};

use crate::gpu::params;
use crate::node::{DelegateEvalContext, DelegateEvaluable, NodeId, NodeParams};

pub(crate) const SHADER: &str = concat!(
    include_str!("../../gpu/shaders/color.wgsl"),
    include_str!("fractal_noise.wgsl")
);

/// The most layers of detail one noise sums.
pub const MAX_NOISE_OCTAVES: usize = 8;
/// Gradient directions in the shader's table, before each is turned at one of four speeds.
const BASE_GRADIENTS: usize = 16;
const GRADIENT_SPEEDS: [f64; 4] = [1.0, -1.0, 2.0, -2.0];

/// How the layers of a fractal noise are summed.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, lumen_macros::NodeEnum, lumen_macros::Delegate,
)]
#[repr(i64)]
#[delegate(kind = "enum", expressions)]
pub enum NoiseMode {
    /// Soft clouds around mid grey.
    #[default]
    Smooth = 0,
    /// Folds each layer at zero, giving dark creases between brighter billows.
    Turbulent = 1,
}

#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct FractalNoiseParams {
    /// Size of the largest features in pixels.
    #[meta(min = 1, step = 1)]
    pub scale: f64,
    /// Layers of detail, from 1 to 8. Each adds features half the size, at half the strength.
    #[meta(min = 1, max = 8, step = 1)]
    pub octaves: u32,
    /// Picks an unrelated pattern.
    #[meta(step = 1)]
    pub seed: i64,
    /// Morphs the pattern smoothly, in turns: it repeats every whole turn, so `=time` loops once a
    /// second.
    #[meta(step = 0.01)]
    pub evolution: f64,
    /// How the layers are summed.
    #[meta()]
    pub mode: NoiseMode,
    /// Contrast multiplier around mid grey.
    #[meta(min = 0, step = 0.01)]
    pub contrast: f64,
    /// Added to every pixel after contrast.
    #[meta(min = -1, max = 1, step = 0.01)]
    pub brightness: f64,
    /// Moves the pattern right by this many pixels.
    #[meta(name = "Offset X", step = 1)]
    pub offset_x: f64,
    /// Moves the pattern down by this many pixels.
    #[meta(name = "Offset Y", step = 1)]
    pub offset_y: f64,
    /// Output width in pixels. Use 0 to match the composition width.
    #[meta(min = 0, enforced_min, step = 1, constant)]
    pub width: u32,
    /// Output height in pixels. Use 0 to match the composition height.
    #[meta(min = 0, enforced_min, step = 1, constant)]
    pub height: u32,
}

impl Default for FractalNoiseParams {
    fn default() -> Self {
        Self {
            scale: 64.0,
            octaves: 4,
            seed: 0,
            evolution: 0.0,
            mode: NoiseMode::Smooth,
            contrast: 1.0,
            brightness: 0.0,
            offset_x: 0.0,
            offset_y: 0.0,
            width: 0,
            height: 0,
        }
    }
}

/// Generates opaque grey fractal noise, the same for a given seed and evolution.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(kind = "fractal_noise", name = "Fractal Noise", category = "source")]
pub struct FractalNoise {
    pub id: NodeId,
    #[params]
    pub params: FractalNoiseParamsDelegate,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct GpuNoiseOctave {
    origin: [f32; 4],
    cell: [u32; 2],
    seed: u32,
    _pad: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct GpuNoiseParams {
    gradients: [[f32; 4]; BASE_GRADIENTS * GRADIENT_SPEEDS.len() / 2],
    octaves: [GpuNoiseOctave; MAX_NOISE_OCTAVES],
    octave_count: u32,
    turbulent: u32,
    contrast: f32,
    brightness: f32,
}

/// The shader's integer mixer, for the per-octave seeds and lattice shifts.
fn noise_hash(value: u32) -> u32 {
    let mut h = value;
    h ^= h >> 16;
    h = h.wrapping_mul(0x7feb_352d);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846c_a68b);
    h ^ (h >> 16)
}

/// The top 24 bits of `hash` as a value in `[0, 1)`, exact in `f32`.
fn unit(hash: u32) -> f64 {
    f64::from(hash >> 8) / f64::from(1u32 << 24)
}

fn finite_or_zero(value: f64) -> f64 {
    if value.is_finite() { value } else { 0.0 }
}

fn gpu_params(params: &FractalNoiseParams) -> GpuNoiseParams {
    // Speeds are whole turns per turn of evolution, so only its fraction matters, and taking it
    // here keeps a long-running `=time` precise.
    let evolution = finite_or_zero(params.evolution).rem_euclid(1.0);
    let mut gradients = [[0.0; 4]; BASE_GRADIENTS * GRADIENT_SPEEDS.len() / 2];
    for (index, pair) in gradients.iter_mut().enumerate() {
        let direction = |slot: usize| {
            let base = (slot % BASE_GRADIENTS) as f64 / BASE_GRADIENTS as f64;
            let speed = GRADIENT_SPEEDS[slot / BASE_GRADIENTS];
            let angle = std::f64::consts::TAU * (base + speed * evolution);
            [angle.cos() as f32, angle.sin() as f32]
        };
        let [a, b] = [direction(index * 2), direction(index * 2 + 1)];
        *pair = [a[0], a[1], b[0], b[1]];
    }

    let count = params.octaves.clamp(1, MAX_NOISE_OCTAVES as u32);
    let total: f64 = (0..count).map(|octave| 0.5f64.powi(octave as i32)).sum();
    let scale = params.scale.max(1e-3);
    let offset = [
        finite_or_zero(params.offset_x),
        finite_or_zero(params.offset_y),
    ];
    // Both halves of the seed count, so seeds a multiple of 2^32 apart differ.
    let (low, high) = (params.seed as u32, (params.seed >> 32) as u32);
    let seed = if high == 0 {
        low
    } else {
        low ^ noise_hash(high)
    };
    let mut octaves = [GpuNoiseOctave::zeroed(); MAX_NOISE_OCTAVES];
    for (octave, slot) in (0..count).zip(&mut octaves) {
        let seed = noise_hash(seed ^ noise_hash(octave.wrapping_add(0x9e37_79b9)));
        let frequency = f64::from(1u32 << octave) / scale;
        // Each octave's lattice is shifted, so the layers don't share a corner at the origin.
        let shift = [unit(noise_hash(seed ^ 1)), unit(noise_hash(seed ^ 2))];
        let point = |axis: usize| (0.5 - offset[axis]) * frequency + shift[axis];
        let [x, y] = [point(0), point(1)];
        *slot = GpuNoiseOctave {
            origin: [
                (x - x.floor()) as f32,
                (y - y.floor()) as f32,
                frequency as f32,
                (0.5f64.powi(octave as i32) / total) as f32,
            ],
            // Lattice cells wrap at 2^32, so a coordinate that far out only repeats them.
            cell: [x, y].map(|value| value.floor().rem_euclid(4_294_967_296.0) as u32),
            seed,
            _pad: 0,
        };
    }
    GpuNoiseParams {
        gradients,
        octaves,
        octave_count: count,
        turbulent: u32::from(params.mode == NoiseMode::Turbulent),
        contrast: params.contrast as f32,
        brightness: params.brightness as f32,
    }
}

const RECORD_LAYOUT: &[lumen_gpu::LayoutEntry] = &[
    lumen_gpu::LayoutEntry::compute(0, lumen_gpu::LayoutKind::Uniform),
    lumen_gpu::LayoutEntry::compute(1, crate::gpu::frame_graph::RECORD_STORAGE_TEXTURE),
];

impl crate::gpu::frame_graph::RecordNode for FractalNoise {
    fn programs(&self, programs: &mut crate::gpu::frame_graph::Programs<'_>) -> crate::Result<()> {
        programs.compute("fractal-noise", SHADER, "cs_main", RECORD_LAYOUT)
    }

    fn record(
        &self,
        cx: &mut crate::gpu::frame_graph::FrameCx<'_>,
    ) -> crate::Result<crate::gpu::frame_graph::Raster> {
        let params = self.params.eval(&DelegateEvalContext {
            node_id: self.id,
            expr: &cx.expr(),
        })?;
        let dimension = |value: u32, canvas: u32| if value == 0 { canvas } else { value };
        let canvas = cx.canvas_size();
        let size = lumen_gpu::Size::new(
            dimension(params.width, canvas.width),
            dimension(params.height, canvas.height),
        );
        let tex = cx.storage_target(size);
        let program = cx.program("fractal-noise", SHADER, "cs_main", RECORD_LAYOUT)?;
        let uniform = cx.rec.uniform(&gpu_params(&params));
        cx.dispatch(
            program,
            &[(0, uniform), (1, lumen_gpu::Res::Tex(tex))],
            params::dispatch_for(size),
        );
        Ok(crate::gpu::frame_graph::Raster {
            tex,
            domain: lumen_gpu::TextureDomain::full_frame(size),
            bounds: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evolution_repeats_every_turn() {
        let at = |evolution| {
            gpu_params(&FractalNoiseParams {
                evolution,
                ..Default::default()
            })
            .gradients
        };
        let close = |a: [[f32; 4]; 32], b: [[f32; 4]; 32]| {
            a.iter()
                .flatten()
                .zip(b.iter().flatten())
                .all(|(a, b)| (a - b).abs() < 1e-6)
        };
        assert!(close(at(0.0), at(1.0)));
        assert!(close(at(0.25), at(-2.75)));
        assert!(close(at(0.3), at(1e6 + 0.3)));
        assert!(!close(at(0.0), at(0.5)));
    }

    #[test]
    fn weights_sum_to_one_and_octaves_clamp() {
        for (octaves, expected) in [(0, 1), (1, 1), (4, 4), (99, MAX_NOISE_OCTAVES as u32)] {
            let gpu = gpu_params(&FractalNoiseParams {
                octaves,
                ..Default::default()
            });
            assert_eq!(gpu.octave_count, expected);
            let sum: f32 = gpu.octaves.iter().map(|octave| octave.origin[3]).sum();
            assert!((sum - 1.0).abs() < 1e-6, "{octaves} octaves weigh {sum}");
        }
    }
}
