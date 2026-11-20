use bytemuck::{Pod, Zeroable};

use super::palette::{MAX_PALETTE_COLORS, Palette};
use crate::gpu::frame_graph::ComputeProgram;
use crate::node::{DelegateEvalContext, DelegateEvaluable, NodeId, NodeParams, OutputRef};

pub(crate) const SHADER: &str = concat!(
    include_str!("../../gpu/shaders/alpha.wgsl"),
    include_str!("../../gpu/shaders/color.wgsl"),
    include_str!("../../gpu/shaders/dither.wgsl"),
    include_str!("posterize.wgsl")
);
const POSTERIZE: ComputeProgram = ComputeProgram::spatial("posterize", SHADER);

/// How a posterize reduces colour.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, lumen_macros::NodeEnum, lumen_macros::Delegate,
)]
#[repr(i64)]
#[delegate(kind = "enum", expressions)]
pub enum PosterizeMode {
    /// Quantizes each of the red, green and blue channels to `levels` steps.
    #[default]
    Levels = 0,
    /// Gives each pixel the `colors` entry whose band of brightness it falls in.
    Palette = 1,
}

#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct PosterizeParams {
    /// What the node reduces the colour to.
    #[meta()]
    pub mode: PosterizeMode,
    /// Steps each channel quantizes to, 2 to 256. 2 gives only the extremes. Only `levels` uses
    /// it.
    #[meta(min = 2, max = 256, step = 1)]
    pub levels: i64,
    /// The colours a pixel may take, 2 to 16, as solid sRGB colours. Only `palette` uses it. Their
    /// alpha is ignored.
    #[meta()]
    pub colors: Palette,
    /// How much of an 8 × 8 Bayer pattern is added before quantizing, 0 to 1. 0 gives hard bands,
    /// and 1 mixes neighbouring steps or colours in proportion to how far between them a pixel
    /// lies.
    #[meta(min = 0, max = 1, step = 0.05)]
    pub dither: f64,
}

impl Default for PosterizeParams {
    fn default() -> Self {
        Self {
            mode: PosterizeMode::Levels,
            levels: 4,
            colors: Palette::default(),
            dither: 0.0,
        }
    }
}

/// Reduces a raster to a few colours, either by quantizing each channel to `levels` steps or by
/// mapping the brightness of each pixel onto the colours of a palette. Both work on the
/// sRGB-encoded colour, where steps are evenly spaced to the eye, and keep the source's alpha.
///
/// In `palette` mode the colours are sorted by their brightness, the Rec. 709 luma of the encoded
/// colour, and a pixel takes the colour whose band it falls in, the bands splitting at the
/// midpoints between neighbouring colours. `dither` places the pixels near a boundary in an 8 × 8
/// Bayer pattern anchored to the canvas origin, so it holds still when the source moves and when
/// the render scale changes.
///
/// For example, `{ "mode": "palette", "colors": [[8, 8, 8, 255], [191, 20, 20, 255], [201, 209, 214, 255]] }`
/// prints a photo in three inks.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(kind = "posterize", name = "Posterize", category = "processing")]
pub struct Posterize {
    pub id: NodeId,
    #[params]
    pub params: PosterizeParamsDelegate,
    #[input()]
    pub source: Option<OutputRef>,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable, lumen_macros::ShaderLayout)]
#[shader_layout(wgsl = "PosterizeParams", source = SHADER)]
struct PosterizeUniforms {
    /// The palette sorted by brightness, as straight linear colours.
    colors: [[f32; 4]; MAX_PALETTE_COLORS],
    /// The brightness of each palette colour, sorted.
    lumas: [[f32; 4]; MAX_PALETTE_COLORS / 4],
    /// Canvas pixel of the source's first texel in `xy`.
    origin: [i32; 4],
    /// The dither amount in `x`.
    dither: [f32; 4],
    /// The number of levels, the mode, and the number of palette colours.
    levels_mode_count: [u32; 4],
}

/// The Rec. 709 luma of the sRGB-encoded colour `rgb`.
fn luma(rgb: [u8; 4]) -> f32 {
    let [r, g, b, _] = rgb.map(|channel| f32::from(channel) / 255.0);
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

impl crate::gpu::frame_graph::RecordNode for Posterize {
    fn programs(&self, programs: &mut crate::gpu::frame_graph::Programs<'_>) -> crate::Result<()> {
        programs.compute(&POSTERIZE)
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
        let mut sorted = params.colors.0;
        sorted.truncate(MAX_PALETTE_COLORS);
        sorted.sort_by(|a, b| luma(*a).total_cmp(&luma(*b)));
        let mut colors = [[0.0; 4]; MAX_PALETTE_COLORS];
        let mut lumas = [0.0; MAX_PALETTE_COLORS];
        for (index, color) in sorted.iter().enumerate() {
            let [r, g, b, _] =
                crate::gpu::color::linear_premultiplied([color[0], color[1], color[2], 255]);
            colors[index] = [r, g, b, 1.0];
            lumas[index] = luma(*color);
        }
        // One dither cell per composition pixel, a whole number of device pixels.
        let cell = cx.pixel_scale().round().clamp(1.0, f64::from(i32::MAX)) as i32;
        let [x, y] = source.origin();
        let uniforms = PosterizeUniforms {
            colors,
            lumas: std::array::from_fn(|row| std::array::from_fn(|lane| lumas[row * 4 + lane])),
            origin: [x, y, cell, 0],
            dither: [params.dither.clamp(0.0, 1.0) as f32, 0.0, 0.0, 0.0],
            levels_mode_count: [
                params.levels.clamp(2, 256) as u32,
                params.mode as u32,
                sorted.len() as u32,
                0,
            ],
        };
        cx.filter(&POSTERIZE, source, &uniforms)
    }

    fn magnification(&self) -> f64 {
        1.0
    }
}
