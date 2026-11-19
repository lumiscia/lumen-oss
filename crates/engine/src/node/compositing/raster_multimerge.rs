use bytemuck::{Pod, Zeroable};

use crate::node::{
    DelegateEvalContext, DelegateEvaluable, NodeId, NodeParams, compositing::BlendMode,
};

use crate::gpu::frame_graph::{ComputeProgram, dispatch_for};

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
    #[meta()]
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

/// Most overlays one pass composites over its base.
const MAX_PASS_OVERLAYS: usize = 8;

const RECORD_LAYOUT: &[lumen_gpu::LayoutEntry] = &[
    lumen_gpu::LayoutEntry::compute(0, lumen_gpu::LayoutKind::Texture),
    lumen_gpu::LayoutEntry::compute(1, lumen_gpu::LayoutKind::Uniform),
    lumen_gpu::LayoutEntry::compute(2, crate::gpu::frame_graph::RECORD_STORAGE_TEXTURE),
    lumen_gpu::LayoutEntry::compute(3, lumen_gpu::LayoutKind::Texture),
    lumen_gpu::LayoutEntry::compute(4, lumen_gpu::LayoutKind::Texture),
    lumen_gpu::LayoutEntry::compute(5, lumen_gpu::LayoutKind::Texture),
    lumen_gpu::LayoutEntry::compute(6, lumen_gpu::LayoutKind::Texture),
    lumen_gpu::LayoutEntry::compute(7, lumen_gpu::LayoutKind::Texture),
    lumen_gpu::LayoutEntry::compute(8, lumen_gpu::LayoutKind::Texture),
    lumen_gpu::LayoutEntry::compute(9, lumen_gpu::LayoutKind::Texture),
    lumen_gpu::LayoutEntry::compute(10, lumen_gpu::LayoutKind::Texture),
];
const RASTER_MULTIMERGE: ComputeProgram = ComputeProgram {
    label: "raster-multimerge",
    shader: SHADER,
    entry: "cs_main",
    layout: RECORD_LAYOUT,
};

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable, lumen_macros::ShaderLayout)]
#[shader_layout(wgsl = "RasterMultiMergeParams", source = SHADER)]
struct RasterMultiMergeUniforms {
    opacity: f32,
    blend_mode: u32,
    /// Output texel of the pass's first invocation, then canvas pixels of the first texels
    /// of the output, base and each overlay.
    start: [i32; 2],
    origin: [i32; 2],
    base_origin: [i32; 2],
    overlay_origins: [[i32; 4]; MAX_PASS_OVERLAYS],
    /// Overlays the pass composites, in binding order.
    count: u32,
    _pad: [u32; 3],
}

/// How many of the first `overlays` one pass takes: up to `MAX_PASS_OVERLAYS`, for as long as
/// the box around them is no larger than their areas together. A pass writes that whole box, so
/// layers that mostly overlap share a pass and layers scattered over the canvas keep their own.
fn pass_len(overlays: &[crate::gpu::frame_graph::Raster]) -> usize {
    let area = |rect: lumen_gpu::RectI| u64::from(rect.width) * u64::from(rect.height);
    let mut rect = overlays[0].domain.data_rect;
    let mut total = area(rect);
    let mut len = 1;
    for overlay in &overlays[1..overlays.len().min(MAX_PASS_OVERLAYS)] {
        let next = overlay.domain.data_rect;
        let union = rect.union(next);
        total += area(next);
        if area(union) > total {
            break;
        }
        rect = union;
        len += 1;
    }
    len
}

impl crate::gpu::frame_graph::RecordNode for RasterMultiMerge {
    fn programs(&self, programs: &mut crate::gpu::frame_graph::Programs<'_>) -> crate::Result<()> {
        programs.compute(&RASTER_MULTIMERGE)
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
            pixel_scale: cx.pixel_scale(),
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
        let program = cx.program(&RASTER_MULTIMERGE)?;
        // Passes alternate between two textures. Each of the first two writes its whole
        // texture; after that a texture still holds the stack from two passes before, which
        // differs from the stack now only within this pass's overlays' texels and the last's.
        let mut targets: [Option<lumen_gpu::TexRef>; 2] = [None; 2];
        let mut current = first;
        let mut rest = overlays;
        let mut last_rect = lumen_gpu::RectI::new(0, 0, 0, 0);
        for pass in 0.. {
            if rest.is_empty() {
                break;
            }
            let (taken, after) = rest.split_at(pass_len(rest));
            rest = after;
            let tex =
                *targets[pass % 2].get_or_insert_with(|| cx.storage_target(domain.storage_size));
            let rect = taken
                .iter()
                .fold(lumen_gpu::RectI::new(0, 0, 0, 0), |union, overlay| {
                    union.union(overlay.domain.data_rect)
                });
            let changed = if pass < 2 {
                domain.data_rect
            } else {
                rect.union(last_rect).intersection(domain.data_rect)
            };
            last_rect = rect;
            let mut overlay_origins = [[0; 4]; MAX_PASS_OVERLAYS];
            // Bindings past the pass's overlays are never read.
            let mut textures = [current.tex; MAX_PASS_OVERLAYS];
            for (index, overlay) in taken.iter().enumerate() {
                let [x, y] = overlay.origin();
                overlay_origins[index] = [x, y, 0, 0];
                textures[index] = overlay.tex;
            }
            let uniform = cx.rec.uniform(&RasterMultiMergeUniforms {
                opacity: evaluated.opacity as f32,
                blend_mode: evaluated.blend_mode as u32,
                start: [
                    changed.x - domain.data_rect.x,
                    changed.y - domain.data_rect.y,
                ],
                origin: [domain.data_rect.x, domain.data_rect.y],
                base_origin: current.origin(),
                overlay_origins,
                count: taken.len() as u32,
                _pad: [0; 3],
            });
            let mut bindings = vec![
                (0, lumen_gpu::Res::Tex(current.tex)),
                (1, uniform),
                (2, lumen_gpu::Res::Tex(tex)),
            ];
            bindings.extend(
                textures
                    .iter()
                    .enumerate()
                    .map(|(index, &tex)| (3 + index as u32, lumen_gpu::Res::Tex(tex))),
            );
            cx.dispatch(program, &bindings, dispatch_for(changed.size()));
            current = crate::gpu::frame_graph::Raster {
                tex,
                domain,
                bounds: None,
            };
        }
        Ok(current)
    }

    fn magnification(&self) -> f64 {
        1.0
    }
}
