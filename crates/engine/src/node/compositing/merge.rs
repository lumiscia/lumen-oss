use crate::node::{
    DelegateEvalContext, DelegateEvaluable, NodeId, NodeParams, compositing::BlendMode,
    processing::gaussian,
};

use crate::gpu::params;

pub(crate) const SHADER: &str = concat!(
    include_str!("../../gpu/shaders/alpha.wgsl"),
    include_str!("../../gpu/shaders/sampling.wgsl"),
    include_str!("blend.wgsl"),
    include_str!("merge.wgsl")
);

/// How the `mask` raster is turned into a per-pixel overlay weight.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, lumen_macros::NodeEnum, lumen_macros::Delegate,
)]
#[repr(u8)]
#[non_exhaustive]
#[delegate(kind = "enum")]
pub enum MaskMode {
    /// Uses the mask's alpha channel.
    #[default]
    Alpha = 0,
    /// Uses the mask's Rec. 709 luma over black, so transparent areas are fully masked out.
    Luma = 1,
}

/// Composites an overlay raster over a base raster.
#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct MergeParams {
    /// Overlay opacity applied before compositing.
    #[meta(min = 0, max = 1, step = 0.05)]
    pub opacity: f64,
    /// Blend mode used when combining the overlay with the base raster.
    #[meta()]
    pub blend_mode: BlendMode,
    /// Channel of the `mask` input that controls overlay visibility.
    #[meta()]
    pub mask_mode: MaskMode,
    /// Swaps the masked and unmasked areas.
    #[meta()]
    pub invert: bool,
    /// Softens the mask edge with a Gaussian blur of this radius in pixels, applied before `invert`. Radii above about 588 look the same.
    #[meta(min = 0, step = 0.5)]
    pub feather: f64,
}

impl Default for MergeParams {
    fn default() -> Self {
        Self {
            opacity: 1.0,
            blend_mode: BlendMode::Normal,
            mask_mode: MaskMode::Alpha,
            invert: false,
            feather: 0.0,
        }
    }
}

/// Composites an overlay raster over a base raster.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(kind = "merge", name = "Merge", category = "compositing")]
pub struct Merge {
    pub id: NodeId,
    #[params]
    pub params: MergeParamsDelegate,

    #[input()]
    pub base: Option<NodeId>,
    #[input()]
    pub overlay: Option<NodeId>,
    #[input(optional)]
    pub mask: Option<NodeId>,
}

/// `MergeParams::mask_source`: no mask, the feathered mask, or the mask as is. A feather
/// that evaluates to zero reads the mask as is, the same as a literal zero.
const MASK_NONE: u32 = 0;
const MASK_FEATHERED: u32 = 1;
const MASK_RAW: u32 = 2;

const RECORD_LAYOUT: &[lumen_gpu::LayoutEntry] = &[
    lumen_gpu::LayoutEntry::compute(0, lumen_gpu::LayoutKind::Texture),
    lumen_gpu::LayoutEntry::compute(1, lumen_gpu::LayoutKind::Texture),
    lumen_gpu::LayoutEntry::compute(2, lumen_gpu::LayoutKind::Texture),
    lumen_gpu::LayoutEntry::compute(3, lumen_gpu::LayoutKind::Uniform),
    lumen_gpu::LayoutEntry::compute(5, lumen_gpu::LayoutKind::Texture),
    lumen_gpu::LayoutEntry::compute(
        4,
        lumen_gpu::LayoutKind::StorageTexture(lumen_gpu::wgpu::TextureFormat::Rgba8Unorm),
    ),
];

impl crate::gpu::frame_graph::RecordNode for Merge {
    fn programs(&self, programs: &mut crate::gpu::frame_graph::Programs<'_>) -> crate::Result<()> {
        gaussian::programs(programs, false)?;
        programs.compute("merge", SHADER, "cs_main", RECORD_LAYOUT)
    }

    fn record(
        &self,
        cx: &mut crate::gpu::frame_graph::FrameCx<'_>,
    ) -> crate::Result<crate::gpu::frame_graph::Raster> {
        let base = cx.required(self.id, "base", self.base)?;
        let overlay = cx.required(self.id, "overlay", self.overlay)?;
        let evaluated = self.params.eval(&DelegateEvalContext {
            node_id: self.id,
            expr: &cx.expr(),
        })?;
        // The blur reads the mask at full frame.
        let feathered = evaluated.feather > 0.0;
        let mask = match self.mask {
            None => None,
            Some(mask) if feathered => Some(cx.input_full(mask)?),
            Some(mask) => Some(cx.input(mask)?),
        };
        let size = base.domain.display_size();
        // Compositing transparent over transparent stays transparent, so only the texels
        // of the base or the overlay are written.
        let domain = cx.domain_within(
            size,
            Some(base.domain.data_rect.union(overlay.domain.data_rect)),
        );
        // The blur spans the mask and the base, so a mask that extends past the base still
        // feathers the base's edge and a smaller one still feathers into the base.
        let feather = match mask.filter(|_| feathered) {
            Some(mask) => {
                let mask_size = mask.domain.display_size();
                let union = lumen_gpu::Size::new(
                    size.width.max(mask_size.width),
                    size.height.max(mask_size.height),
                );
                Some(gaussian::record_passes(
                    cx,
                    mask,
                    lumen_gpu::TextureDomain::full_frame(union),
                    false,
                    &gaussian::GaussianSettings {
                        radius: evaluated.feather.max(0.0),
                        transparent_edge: true,
                        offset: [0.0, 0.0],
                    },
                )?)
            }
            None => None,
        };
        let params = params::MergeParams {
            opacity: evaluated.opacity as f32,
            blend_mode: evaluated.blend_mode as u32,
            mask_source: match feather {
                _ if mask.is_none() => MASK_NONE,
                Some(_) => MASK_FEATHERED,
                None => MASK_RAW,
            },
            mask_mode: evaluated.mask_mode as u32,
            invert: u32::from(evaluated.invert),
            origins: [
                [domain.data_rect.x, domain.data_rect.y],
                base.origin(),
                overlay.origin(),
                mask.map_or(base.origin(), |mask| mask.origin()),
            ],
            _pad0: 0,
        };
        let tex = cx.storage_target(domain.storage_size);
        let program = cx.program("merge", SHADER, "cs_main", RECORD_LAYOUT)?;
        let uniform = cx.rec.uniform(&params);
        cx.dispatch(
            program,
            &[
                (0, lumen_gpu::Res::Tex(base.tex)),
                (1, lumen_gpu::Res::Tex(overlay.tex)),
                (2, lumen_gpu::Res::Tex(feather.unwrap_or(base.tex))),
                (
                    5,
                    lumen_gpu::Res::Tex(mask.map_or(base.tex, |mask| mask.tex)),
                ),
                (3, uniform),
                (4, lumen_gpu::Res::Tex(tex)),
            ],
            params::dispatch_for(domain.storage_size),
        );
        Ok(crate::gpu::frame_graph::Raster {
            tex,
            domain,
            bounds: None,
        })
    }
}
