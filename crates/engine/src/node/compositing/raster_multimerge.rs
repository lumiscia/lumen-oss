use crate::node::{
    DelegateEvalContext, DelegateEvaluable, NodeId, NodeParams, compositing::BlendMode,
};

use crate::gpu::compiler::Slot;
use crate::gpu::{
    BoundFrame, FrameBindContext, GpuCompileNode, GpuCompiledNode, RasterHandle, compiler,
};

pub(crate) const SHADER: &str = concat!(
    include_str!("../../gpu/shaders/alpha.wgsl"),
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
    /// Blend mode used for each layer in the stack.
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

#[derive(Debug, Clone)]
struct RasterMultiCompiledMerge {
    node_id: NodeId,
    params: RasterMultiMergeParamsDelegate,
    layers: Vec<CompiledLayer>,
}

/// Per-pass uniform buffer, carrying the placement of that pass's inputs.
#[derive(Debug, Clone)]
struct CompiledLayer {
    buffer: lumen_gpu::BufferId,
    /// Output texel of the pass's first invocation, then canvas pixels of the first texels
    /// of the output, base and overlay.
    origins: [[i32; 2]; 4],
}

impl GpuCompiledNode for RasterMultiCompiledMerge {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<()> {
        let evaluated = self.params.eval(&DelegateEvalContext {
            node_id: self.node_id,
            expr: &ctx.expr_context(),
        })?;
        for layer in &self.layers {
            let params = compiler::RasterMultiMergeParams {
                opacity: evaluated.opacity as f32,
                blend_mode: evaluated.blend_mode as u32,
                origins: layer.origins,
            };
            bound.write_buffer(layer.buffer, 0, bytemuck::bytes_of(&params));
        }
        Ok(())
    }
}

impl GpuCompileNode for RasterMultiMerge {
    fn compile_gpu(&self, ctx: &mut crate::gpu::CompileContext<'_>) -> crate::Result<RasterHandle> {
        let layers = self
            .layers
            .iter()
            .map(|&layer| ctx.compile_bounded_port(layer))
            .collect::<crate::Result<Vec<_>>>()?;
        let Some((&first, overlays)) = layers.split_first() else {
            return Ok(ctx.compile_transparent(self.id));
        };

        // Compositing transparent over a pixel leaves it as it was, so the stack is written
        // only where some layer has texels, and a pass changes its base only within its
        // overlay's texels.
        let domain = lumen_gpu::TextureDomain::bounded(
            first.domain.display_size(),
            layers.iter().fold(first.domain.data_rect, |union, layer| {
                union.union(layer.domain.data_rect)
            }),
        );
        let origin = |domain: &lumen_gpu::TextureDomain| [domain.data_rect.x, domain.data_rect.y];
        let program = ctx.compute_program(
            "raster-multimerge",
            SHADER,
            &[Slot::Texture, Slot::Texture, Slot::Uniform, Slot::Output],
        );

        // Passes alternate between two textures. Each of the first two writes its whole
        // texture; after that a texture still holds the stack from two passes before, which
        // differs from the stack now only within this overlay's texels and the last one's.
        let mut targets = Vec::new();
        let mut current = first;
        let mut compiled_layers = Vec::new();
        for (index, overlay) in overlays.iter().enumerate() {
            let params = ctx.builder_mut().buffer(
                Some(format!("raster-multimerge:{}:params-{index}", self.id.0)),
                lumen_gpu::BufferDesc::uniform(
                    std::mem::size_of::<compiler::RasterMultiMergeParams>() as u64,
                ),
            );
            if targets.len() <= index % 2 {
                let texture = ctx.builder_mut().texture(
                    Some(format!("raster-multimerge:{}:layer-{index}", self.id.0)),
                    lumen_gpu::TextureDesc::storage(
                        domain.storage_size,
                        lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
                    )
                    .with_domain(domain),
                );
                targets.push(texture);
            }
            let texture = targets[index % 2];
            let changed = if index < 2 {
                domain.data_rect
            } else {
                overlay
                    .domain
                    .data_rect
                    .union(overlays[index - 1].domain.data_rect)
                    .intersection(domain.data_rect)
            };
            let start = [
                changed.x - domain.data_rect.x,
                changed.y - domain.data_rect.y,
            ];
            compiled_layers.push(CompiledLayer {
                buffer: params,
                origins: [
                    start,
                    origin(&domain),
                    origin(&current.domain),
                    origin(&overlay.domain),
                ],
            });
            ctx.builder_mut().compute_pass(lumen_gpu::ComputePassDesc {
                label: Some(format!("raster-multimerge:{}:layer-{index}", self.id.0)),
                program,
                bindings: vec![
                    lumen_gpu::Binding::sampled_texture(0, 0, current.texture),
                    lumen_gpu::Binding::sampled_texture(0, 1, overlay.texture),
                    lumen_gpu::Binding::uniform(0, 2, params),
                    lumen_gpu::Binding::storage_texture(0, 3, texture),
                ],
                dispatch: compiler::dispatch_for(changed.size()),
            });
            current = RasterHandle {
                texture,
                domain,
                bounds: None,
            };
        }
        ctx.register_compiled_node(RasterMultiCompiledMerge {
            node_id: self.id,
            params: self.params.clone(),
            layers: compiled_layers,
        });

        Ok(current)
    }
}

const RECORD_LAYOUT: &[lumen_gpu::LayoutEntry] = &[
    lumen_gpu::LayoutEntry::compute(0, lumen_gpu::LayoutKind::Texture),
    lumen_gpu::LayoutEntry::compute(1, lumen_gpu::LayoutKind::Texture),
    lumen_gpu::LayoutEntry::compute(2, lumen_gpu::LayoutKind::Uniform),
    lumen_gpu::LayoutEntry::compute(
        3,
        lumen_gpu::LayoutKind::StorageTexture(lumen_gpu::wgpu::TextureFormat::Rgba8Unorm),
    ),
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
            let params = compiler::RasterMultiMergeParams {
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
                compiler::dispatch_for(changed.size()),
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
