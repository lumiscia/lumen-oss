use bytemuck::{Pod, Zeroable};

use crate::gpu::frame_graph::ComputeProgram;
use crate::node::{DelegateEvalContext, DelegateEvaluable, NodeId, NodeParams, OutputRef};

pub(crate) const SHADER: &str = concat!(
    include_str!("../../gpu/shaders/alpha.wgsl"),
    include_str!("../../gpu/shaders/color.wgsl"),
    include_str!("halftone.wgsl")
);
const HALFTONE: ComputeProgram = ComputeProgram::spatial("halftone", SHADER);

/// What each cell of a halftone draws.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, lumen_macros::NodeEnum, lumen_macros::Delegate,
)]
#[repr(i64)]
#[delegate(kind = "enum", expressions)]
pub enum HalftoneShape {
    /// A dot at the centre of the cell.
    #[default]
    Round = 0,
    /// A line along the rotated x axis, through the centre of the cell.
    Line = 1,
}

#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct HalftoneParams {
    /// Width and height of a cell in pixels.
    #[meta(min = 1, step = 0.5, pixels)]
    pub pitch: f64,
    /// Rotation of the grid in degrees.
    #[meta(step = 1)]
    pub angle: f64,
    /// What each cell draws.
    #[meta()]
    pub shape: HalftoneShape,
    /// Color where the source is dark.
    #[meta(role = "color")]
    pub ink: [u8; 4],
    /// Color where the source is light.
    #[meta(role = "color")]
    pub paper: [u8; 4],
}

impl Default for HalftoneParams {
    fn default() -> Self {
        Self {
            pitch: 8.0,
            angle: 45.0,
            shape: HalftoneShape::Round,
            ink: [0, 0, 0, 255],
            paper: [255, 255, 255, 255],
        }
    }
}

/// Redraws a raster as a grid of dots or lines in `ink` on `paper`, as in a print screen. Each
/// cell takes the darkness, 1 minus the luma of the sRGB-encoded colour, at its centre, and
/// covers that fraction of its area, so a flat colour gives uniform dots and black fills the
/// whole cell. The grid is anchored to the canvas origin, so the pattern stays put when the
/// source moves, and the source's alpha is kept.
///
/// For example, `{ "pitch": 55, "angle": -19, "ink": [110, 28, 38, 255], "paper": [120, 134, 150, 255] }`
/// over a flat colour gives oxblood dots on a grey-blue ground, on a grid rotated -19 degrees.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(kind = "halftone", name = "Halftone", category = "processing")]
pub struct Halftone {
    pub id: NodeId,
    #[params]
    pub params: HalftoneParamsDelegate,
    #[input()]
    pub source: Option<OutputRef>,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable, lumen_macros::ShaderLayout)]
#[shader_layout(wgsl = "HalftoneParams", source = SHADER)]
struct HalftoneUniforms {
    /// Premultiplied linear colours.
    ink: [f32; 4],
    paper: [f32; 4],
    /// Cell size in device pixels, then the cosine and sine of the grid's angle.
    grid: [f32; 4],
    /// Canvas pixel of the source's first texel in `xy`, then the shape.
    origin_shape: [i32; 4],
}

impl crate::gpu::frame_graph::RecordNode for Halftone {
    fn programs(&self, programs: &mut crate::gpu::frame_graph::Programs<'_>) -> crate::Result<()> {
        programs.compute(&HALFTONE)
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
        let (sin, cos) = params.angle.to_radians().sin_cos();
        let [x, y] = source.origin();
        let uniforms = HalftoneUniforms {
            ink: crate::gpu::color::linear_premultiplied(params.ink),
            paper: crate::gpu::color::linear_premultiplied(params.paper),
            grid: [
                params.pitch.max(f64::from(f32::EPSILON)) as f32,
                cos as f32,
                sin as f32,
                0.0,
            ],
            origin_shape: [x, y, params.shape as i32, 0],
        };
        cx.filter(&HALFTONE, source, &uniforms)
    }

    fn magnification(&self) -> f64 {
        1.0
    }
}
