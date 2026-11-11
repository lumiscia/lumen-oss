use crate::node::{
    DelegateEvalContext, DelegateEvaluable, NodeId, NodeParams, compositing::BlendMode,
};

use crate::gpu::params;

pub(crate) const SHADER: &str = concat!(
    include_str!("../../gpu/shaders/alpha.wgsl"),
    include_str!("../../gpu/shaders/color.wgsl"),
    include_str!("../../gpu/shaders/sampling.wgsl"),
    include_str!("blend.wgsl"),
    include_str!("raster_multimerge.wgsl")
);

/// Composites a variadic stack of raster layers in order.
#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct RasterMultiMergeParams {
    /// Opacity of each layer above the first, which is the base and is used as is.
    #[meta(min = 0, max = 1, step = 0.05)]
    pub opacity: f64,
    /// Blend mode used for each layer in the stack. `normal` composites in linear light; every
    /// other mode applies its formula to the sRGB-encoded colours and composites the result in
    /// linear light.
    #[meta(kind = "enum", enum_type = BlendMode)]
    pub blend_mode: BlendMode,
}

impl Default for RasterMultiMergeParams {
    fn default() -> Self {
        Self {
            opacity: 1.0,
            blend_mode: BlendMode::Normal,
        }
    }
}

/// Composites a variadic stack of raster layers in order.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(
    kind = "raster_multimerge",
    name = "Raster Multi Merge",
    category = "compositing"
)]
pub struct RasterMultiMerge {
    pub id: NodeId,
    #[params]
    pub params: RasterMultiMergeParamsDelegate,

    #[input(optional, variadic)]
    pub layers: Vec<NodeId>,
}

const RECORD_LAYOUT: &[lumen_gpu::LayoutEntry] = &[
    lumen_gpu::LayoutEntry::compute(0, lumen_gpu::LayoutKind::Texture),
    lumen_gpu::LayoutEntry::compute(1, lumen_gpu::LayoutKind::Texture),
    lumen_gpu::LayoutEntry::compute(2, lumen_gpu::LayoutKind::Uniform),
    lumen_gpu::LayoutEntry::compute(3, crate::gpu::frame_graph::RECORD_STORAGE_TEXTURE),
];

impl crate::gpu::frame_graph::RecordNode for RasterMultiMerge {
    fn programs(&self, programs: &mut crate::gpu::frame_graph::Programs<'_>) -> crate::Result<()> {
        programs.compute("raster-multimerge", SHADER, "cs_main", RECORD_LAYOUT)
    }

    fn record(
        &self,
        cx: &mut crate::gpu::frame_graph::FrameCx<'_>,
    ) -> crate::Result<crate::gpu::frame_graph::Raster> {
        let layers = self
            .layers
            .iter()
            .map(|&layer| cx.input(layer))
            .collect::<crate::Result<Vec<_>>>()?;
        let Some((&first, overlays)) = layers.split_first() else {
            return Ok(cx.transparent(cx.canvas_size()));
        };
        let evaluated = self.params.eval(&DelegateEvalContext {
            node_id: self.id,
            expr: &cx.expr(),
        })?;
        // Compositing transparent over a pixel leaves it as it was, so the stack is written
        // only where some layer has texels, and a pass changes its base only within its
        // overlay's texels.
        let domain = cx.domain_within(
            first.domain.display_size(),
            Some(layers.iter().fold(first.domain.data_rect, |union, layer| {
                union.union(layer.domain.data_rect)
            })),
        );
        let program = cx.program("raster-multimerge", SHADER, "cs_main", RECORD_LAYOUT)?;
        // Passes alternate between two textures. Each of the first two writes its whole
        // texture; after that a texture still holds the stack from two passes before, which
        // differs from the stack now only within this overlay's texels and the last one's.
        let mut targets: [Option<lumen_gpu::TexRef>; 2] = [None; 2];
        let mut current = first;
        for (index, overlay) in overlays.iter().enumerate() {
            let tex = match targets[index % 2] {
                Some(tex) => tex,
                None => {
                    let tex = cx.storage_target(domain.storage_size);
                    targets[index % 2] = Some(tex);
                    tex
                }
            };
            let changed = if index < 2 {
                domain.data_rect
            } else {
                overlay
                    .domain
                    .data_rect
                    .union(overlays[index - 1].domain.data_rect)
                    .intersection(domain.data_rect)
            };
            let params = params::RasterMultiMergeParams {
                opacity: evaluated.opacity as f32,
                blend_mode: evaluated.blend_mode as u32,
                origins: [
                    [
                        changed.x - domain.data_rect.x,
                        changed.y - domain.data_rect.y,
                    ],
                    [domain.data_rect.x, domain.data_rect.y],
                    current.origin(),
                    overlay.origin(),
                ],
            };
            let uniform = cx.rec.uniform(&params);
            cx.dispatch(
                program,
                &[
                    (0, lumen_gpu::Res::Tex(current.tex)),
                    (1, lumen_gpu::Res::Tex(overlay.tex)),
                    (2, uniform),
                    (3, lumen_gpu::Res::Tex(tex)),
                ],
                params::dispatch_for(changed.size()),
            );
            current = crate::gpu::frame_graph::Raster {
                tex,
                domain,
                bounds: None,
            };
        }
        Ok(current)
    }
}
