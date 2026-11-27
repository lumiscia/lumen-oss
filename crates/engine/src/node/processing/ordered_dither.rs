use bytemuck::{Pod, Zeroable};

use crate::gpu::frame_graph::ComputeProgram;
use crate::node::{DelegateEvalContext, DelegateEvaluable, NodeId, NodeParams, OutputRef};

pub(crate) const SHADER: &str = concat!(
    include_str!("../../gpu/shaders/alpha.wgsl"),
    include_str!("../../gpu/shaders/color.wgsl"),
    include_str!("ordered_dither.wgsl")
);
const ORDERED_DITHER: ComputeProgram = ComputeProgram::spatial("ordered-dither", SHADER);

/// The threshold matrix an ordered dither compares against.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, lumen_macros::NodeEnum, lumen_macros::Delegate,
)]
#[repr(i64)]
#[delegate(kind = "enum", expressions)]
pub enum DitherMatrix {
    /// A Bayer matrix of 2 × 2 cells.
    Bayer2 = 0,
    /// A Bayer matrix of 4 × 4 cells.
    #[default]
    Bayer4 = 1,
    /// A Bayer matrix of 8 × 8 cells.
    Bayer8 = 2,
}

/// What an ordered dither draws.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, lumen_macros::NodeEnum, lumen_macros::Delegate,
)]
#[repr(i64)]
#[delegate(kind = "enum", expressions)]
pub enum DitherMode {
    /// Quantizes the darkness of each pixel and mixes `paper` to `ink` by it.
    #[default]
    InkPaper = 0,
    /// Quantizes each of the source's red, green and blue channels.
    Source = 1,
}

#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct OrderedDitherParams {
    /// The threshold matrix.
    #[meta()]
    pub matrix: DitherMatrix,
    /// Levels each value quantizes to, at least 2. 2 gives only the extremes.
    #[meta(min = 2, step = 1)]
    pub levels: i64,
    /// Width and height of a matrix cell in pixels. A cell is a whole number of output pixels, at
    /// least one, so the pitch rounds at render scales where it isn't.
    #[meta(min = 1, step = 1, pixels)]
    pub pitch: f64,
    /// What the node draws.
    #[meta()]
    pub mode: DitherMode,
    /// Color of the darkest level. Only `ink_paper` uses it.
    #[meta(role = "color")]
    pub ink: [u8; 4],
    /// Color of the lightest level. Only `ink_paper` uses it.
    #[meta(role = "color")]
    pub paper: [u8; 4],
}

impl Default for OrderedDitherParams {
    fn default() -> Self {
        Self {
            matrix: DitherMatrix::Bayer4,
            levels: 2,
            pitch: 1.0,
            mode: DitherMode::InkPaper,
            ink: [0, 0, 0, 255],
            paper: [255, 255, 255, 255],
        }
    }
}

/// Reduces a raster to a few levels with a Bayer threshold matrix, which trades the banding of
/// plain quantizing for a fixed, regular pattern. Values quantize in sRGB-encoded space. The
/// matrix is anchored to the canvas origin and each cell is a whole number of device pixels, so
/// cell edges are never antialiased. The source's alpha is kept.
///
/// For example, `{ "matrix": "bayer8", "pitch": 2, "ink": [20, 24, 40, 255], "paper": [240, 232, 210, 255] }`
/// redraws a photo in two colours with cells of 2 pixels.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(
    kind = "ordered_dither",
    name = "Ordered Dither",
    category = "processing"
)]
pub struct OrderedDither {
    pub id: NodeId,
    #[params]
    pub params: OrderedDitherParamsDelegate,
    #[input()]
    pub source: Option<OutputRef>,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable, lumen_macros::ShaderLayout)]
#[shader_layout(wgsl = "DitherParams", source = SHADER)]
struct DitherUniforms {
    /// Premultiplied linear colours.
    ink: [f32; 4],
    paper: [f32; 4],
    /// Canvas pixel of the source's first texel in `xy`, the matrix size in `z` and the cell
    /// size in device pixels in `w`.
    origin_matrix_cell: [i32; 4],
    /// The number of levels, then the mode.
    levels_mode: [u32; 4],
}

impl crate::gpu::frame_graph::RecordNode for OrderedDither {
    fn programs(&self, programs: &mut crate::gpu::frame_graph::Programs<'_>) -> crate::Result<()> {
        programs.compute(&ORDERED_DITHER)
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
        let matrix_size = 2_i32 << params.matrix as i32;
        let cell = params.pitch.round().clamp(1.0, f64::from(i32::MAX)) as i32;
        let [x, y] = source.origin();
        let uniforms = DitherUniforms {
            ink: crate::gpu::color::linear_premultiplied(params.ink),
            paper: crate::gpu::color::linear_premultiplied(params.paper),
            origin_matrix_cell: [x, y, matrix_size, cell],
            levels_mode: [
                params.levels.clamp(2, i64::from(u16::MAX)) as u32,
                params.mode as u32,
                0,
                0,
            ],
        };
        cx.filter(&ORDERED_DITHER, source, &uniforms)
    }

    fn magnification(&self) -> f64 {
        1.0
    }
}
