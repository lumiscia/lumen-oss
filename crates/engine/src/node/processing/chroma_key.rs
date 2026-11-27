use bytemuck::{Pod, Zeroable};

use crate::gpu::frame_graph::ComputeProgram;
use crate::node::{DelegateEvalContext, DelegateEvaluable, NodeId, NodeParams, OutputRef};

pub(crate) const SHADER: &str = concat!(
    include_str!("../../gpu/shaders/alpha.wgsl"),
    include_str!("../../gpu/shaders/color.wgsl"),
    include_str!("chroma_key.wgsl")
);
const CHROMA_KEY: ComputeProgram = ComputeProgram::spatial("chroma-key", SHADER);

#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct ChromaKeyParams {
    /// The screen color.
    #[meta(role = "color")]
    pub key_color: [u8; 4],
    /// How far a color may be from the key color in chromaticity, 0 for the same hue to 1.4 for
    /// the furthest, and still be fully removed.
    #[meta(min = 0, step = 0.005)]
    pub tolerance: f64,
    /// How much further out the edge is feathered, from fully removed to fully kept. 0 is a hard
    /// edge at `tolerance`.
    #[meta(min = 0, step = 0.005)]
    pub softness: f64,
    /// How much of the key color's cast is removed from the colors that are kept, from 0 for none
    /// to 1 for all of it.
    #[meta(min = 0, max = 1, step = 0.05)]
    pub spill: f64,
    /// Outputs the matte as white over transparent instead of the keyed image.
    #[meta()]
    pub matte_only: bool,
}

impl Default for ChromaKeyParams {
    fn default() -> Self {
        Self {
            key_color: [0, 255, 0, 255],
            tolerance: 0.08,
            softness: 0.08,
            spill: 0.5,
            matte_only: false,
        }
    }
}

/// Removes a green or blue screen from a raster, in linear light.
///
/// Each pixel's chromaticity, its colour divided by its brightness, is compared with the key
/// color's, so a screen keys evenly from its shadows to its highlights. Pixels within
/// `tolerance` of the key are removed, kept in full beyond `tolerance + softness`, and fade
/// between. Near-black pixels have no reliable hue and are always kept. Then `spill` pulls the
/// key's dominant channel down to the larger of the other two on the pixels that remain, which
/// takes the green or blue cast out of edges and reflections. The result keeps the source's
/// alpha times the matte. With `matte_only`, the output is white with that alpha instead, which
/// can feed `merge`'s mask or `channels`.
///
/// For example, `{ "key_color": [0, 177, 64, 255], "tolerance": 0.1, "softness": 0.1 }` on
/// footage shot against a green screen leaves the subject over transparency.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(kind = "chroma_key", name = "Chroma Key", category = "processing")]
pub struct ChromaKey {
    pub id: NodeId,
    #[params]
    pub params: ChromaKeyParamsDelegate,
    #[input()]
    pub source: Option<OutputRef>,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable, lumen_macros::ShaderLayout)]
#[shader_layout(wgsl = "ChromaKeyParams", source = SHADER)]
struct ChromaKeyUniforms {
    /// The key's chromaticity in `xy`, then the tolerance and softness.
    key: [f32; 4],
    /// The spill amount in `x`.
    spill: [f32; 4],
    /// The key's dominant channel, 0 to 2, then 1 for matte only.
    modes: [u32; 4],
}

/// The channel of `rgb` that spill suppression limits: the largest, preferring green, then blue.
fn dominant_channel([r, g, b]: [f32; 3]) -> u32 {
    if g >= r && g >= b {
        1
    } else if b >= r {
        2
    } else {
        0
    }
}

impl crate::gpu::frame_graph::RecordNode for ChromaKey {
    fn programs(&self, programs: &mut crate::gpu::frame_graph::Programs<'_>) -> crate::Result<()> {
        programs.compute(&CHROMA_KEY)
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
        // The key's alpha doesn't take part.
        let [r, g, b, _] = params.key_color;
        let key = [r, g, b].map(|c| crate::gpu::color::srgb_to_linear(f32::from(c) / 255.0));
        let sum = key.iter().sum::<f32>().max(f32::EPSILON);
        let uniforms = ChromaKeyUniforms {
            key: [
                key[0] / sum,
                key[1] / sum,
                params.tolerance.max(0.0) as f32,
                params.softness.max(0.0) as f32,
            ],
            spill: [params.spill.clamp(0.0, 1.0) as f32, 0.0, 0.0, 0.0],
            modes: [dominant_channel(key), u32::from(params.matte_only), 0, 0],
        };
        cx.filter(&CHROMA_KEY, source, &uniforms)
    }

    fn magnification(&self) -> f64 {
        1.0
    }
}
