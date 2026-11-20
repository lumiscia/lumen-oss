use bytemuck::{Pod, Zeroable};

use crate::gpu::frame_graph::{ComputeProgram, dispatch_for};
use crate::node::{DelegateEvalContext, DelegateEvaluable, NodeId, NodeParams};

pub(crate) const SHADER: &str = concat!(
    include_str!("../../gpu/shaders/alpha.wgsl"),
    include_str!("../../gpu/shaders/sampling.wgsl"),
    include_str!("channels.wgsl")
);

/// Where an output channel of `channels` takes its value from. Values are read from the straight
/// colour as they are, with no transfer function, and luma is Rec. 709 on the linear colour.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, lumen_macros::NodeEnum, lumen_macros::Delegate,
)]
#[repr(i64)]
#[delegate(kind = "enum")]
pub enum ChannelSource {
    #[default]
    SourceRed = 0,
    SourceGreen = 1,
    SourceBlue = 2,
    SourceAlpha = 3,
    SourceLuma = 4,
    OtherRed = 5,
    OtherGreen = 6,
    OtherBlue = 7,
    OtherAlpha = 8,
    OtherLuma = 9,
    Zero = 10,
    One = 11,
}

impl ChannelSourceDelegate {
    fn reads_other(&self) -> bool {
        matches!(
            self,
            Self::OtherRed
                | Self::OtherGreen
                | Self::OtherBlue
                | Self::OtherAlpha
                | Self::OtherLuma
        )
    }
}

#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct ChannelsParams {
    /// Where the output's red comes from.
    #[meta()]
    pub red: ChannelSource,
    /// Where the output's green comes from.
    #[meta()]
    pub green: ChannelSource,
    /// Where the output's blue comes from.
    #[meta()]
    pub blue: ChannelSource,
    /// Where the output's alpha comes from.
    #[meta()]
    pub alpha: ChannelSource,
}

impl Default for ChannelsParams {
    fn default() -> Self {
        Self {
            red: ChannelSource::SourceRed,
            green: ChannelSource::SourceGreen,
            blue: ChannelSource::SourceBlue,
            alpha: ChannelSource::SourceAlpha,
        }
    }
}

/// Builds each channel of a raster from a channel of `source`, a channel of `other`, the luma of
/// either, 0 or 1. The output covers the area of `source`, and `other` is read at the same canvas
/// pixels, as transparent outside its own area. Choosing an `other_*` value with nothing connected
/// to `other` is an error. Colour is taken unpremultiplied and the result is premultiplied by the
/// output alpha.
///
/// To set a raster's alpha from another raster's luma, connect it to `other` and set `alpha` to
/// `other_luma`. To use the green channel of a matte as alpha, connect the matte to `other` and set
/// `alpha` to `other_green`.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(kind = "channels", name = "Channels", category = "processing")]
pub struct Channels {
    pub id: NodeId,
    #[params]
    pub params: ChannelsParamsDelegate,
    #[input()]
    pub source: Option<NodeId>,
    #[input(optional)]
    pub other: Option<NodeId>,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable, lumen_macros::ShaderLayout)]
#[shader_layout(wgsl = "ChannelsParams", source = SHADER)]
struct ChannelsUniforms {
    picks: [u32; 4],
    origins: [i32; 4],
}

const RECORD_LAYOUT: &[lumen_gpu::LayoutEntry] = &[
    lumen_gpu::LayoutEntry::compute(0, lumen_gpu::LayoutKind::Texture),
    lumen_gpu::LayoutEntry::compute(1, lumen_gpu::LayoutKind::Texture),
    lumen_gpu::LayoutEntry::compute(2, lumen_gpu::LayoutKind::Uniform),
    lumen_gpu::LayoutEntry::compute(3, crate::gpu::frame_graph::RECORD_STORAGE_TEXTURE),
];
const CHANNELS: ComputeProgram = ComputeProgram {
    label: "channels",
    shader: SHADER,
    entry: "cs_main",
    layout: RECORD_LAYOUT,
};

impl crate::gpu::frame_graph::RecordNode for Channels {
    fn programs(&self, programs: &mut crate::gpu::frame_graph::Programs<'_>) -> crate::Result<()> {
        programs.compute(&CHANNELS)
    }

    fn record(
        &self,
        cx: &mut crate::gpu::frame_graph::FrameCx<'_>,
    ) -> crate::Result<crate::gpu::frame_graph::Raster> {
        let source = cx.required(self.id, "source", self.source)?;
        let other = match self.other {
            Some(other) => Some(cx.input(other)?),
            None => None,
        };
        let params = self.params.eval(&DelegateEvalContext {
            node_id: self.id,
            expr: &cx.expr(),
            pixel_scale: cx.pixel_scale(),
        })?;
        // Without `other`, no pick reads it, so the source stands in for the binding.
        let other = other.unwrap_or(source);
        let uniforms = ChannelsUniforms {
            picks: [params.red, params.green, params.blue, params.alpha].map(|pick| pick as u32),
            origins: [
                source.domain.data_rect.x,
                source.domain.data_rect.y,
                other.domain.data_rect.x,
                other.domain.data_rect.y,
            ],
        };
        let size = source.domain.storage_size;
        let tex = cx.storage_target(size);
        let program = cx.program(&CHANNELS)?;
        let uniform = cx.rec.uniform(&uniforms);
        cx.dispatch(
            program,
            &[
                (0, lumen_gpu::Res::Tex(source.tex)),
                (1, lumen_gpu::Res::Tex(other.tex)),
                (2, uniform),
                (3, lumen_gpu::Res::Tex(tex)),
            ],
            dispatch_for(size),
        );
        Ok(crate::gpu::frame_graph::Raster { tex, ..source })
    }

    fn magnification(&self) -> f64 {
        1.0
    }

    fn unwired_input_param_errors(&self) -> Vec<(&'static str, String)> {
        if self.other.is_some() {
            return Vec::new();
        }
        let picks = [
            ("red", &self.params.red),
            ("green", &self.params.green),
            ("blue", &self.params.blue),
            ("alpha", &self.params.alpha),
        ];
        picks
            .into_iter()
            .filter(|(_, pick)| pick.reads_other())
            .map(|(param, _)| {
                (
                    param,
                    format!("`{param}` reads the `other` input, which isn't connected"),
                )
            })
            .collect()
    }
}
