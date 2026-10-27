use crate::node::{NodeId, NodeParamEvalContext, NodeParams, PortRef};

use crate::gpu::{
    AlphaMode, BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuCompiledNode,
    RasterHandle, RasterMetadata, compiler,
};

pub(crate) const SHADER: &str = concat!(
    include_str!("../../gpu/shaders/alpha.wgsl"),
    include_str!("blend.wgsl"),
    include_str!("raster_multimerge.wgsl")
);

/// Composites a variadic stack of raster layers in order.
#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct RasterMultiMergeParams {
    /// Opacity applied to each layer as it is composited.
    #[meta(min = 0, max = 1, step = 0.05)]
    pub opacity: f64,
    /// Blend mode used for each layer in the stack.
    #[meta(kind = "enum", enum_type = crate::node::compositing::BlendMode)]
    pub blend_mode: i64,
}

impl Default for RasterMultiMergeParams {
    fn default() -> Self {
        Self {
            opacity: 1.0,
            blend_mode: 0,
        }
    }
}

/// Composites a variadic stack of raster layers in order.
#[derive(Debug, Clone, lumen_macros::Node)]
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
    pub layers: Vec<PortRef>,
}

impl Default for RasterMultiMerge {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            params: RasterMultiMergeParamsDelegate::default(),
            layers: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
struct RasterMultiCompiledMerge {
    node_id: NodeId,
    params: RasterMultiMergeParamsDelegate,
    layers: Vec<CompiledLayer>,
}

/// Per-pass uniform buffer, carrying the alpha representation and placement of that pass's
/// inputs.
#[derive(Debug, Clone)]
struct CompiledLayer {
    buffer: lumen_gpu::BufferId,
    base_straight: bool,
    overlay_straight: bool,
    /// Output texel of the pass's first invocation, then canvas pixels of the first texels
    /// of the output, base and overlay.
    origins: [[i32; 2]; 4],
}

impl GpuCompiledNode for RasterMultiCompiledMerge {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<()> {
        let evaluated = self.params.eval(&NodeParamEvalContext {
            node_id: self.node_id,
            expr: &ctx.expr_context(self.node_id, "params"),
        })?;
        for layer in &self.layers {
            let params = compiler::RasterMultiMergeParams {
                values: [
                    evaluated.opacity as f32,
                    evaluated.blend_mode as f32,
                    f32::from(u8::from(layer.base_straight)),
                    f32::from(u8::from(layer.overlay_straight)),
                ],
                origins: layer.origins,
            };
            bound.write_buffer(layer.buffer, 0, bytemuck::bytes_of(&params));
        }
        Ok(())
    }
}

impl GpuCompileNode for RasterMultiMerge {
    fn compile_gpu(
        &self,
        ctx: &mut crate::gpu::CompileContext<'_>,
        port: &PortRef,
    ) -> crate::Result<CompiledOutput> {
        if port.port != "output" {
            return Err(ctx.missing_output(self.id, &port.port));
        }

        let layers = self
            .layers
            .iter()
            .filter(|layer| !layer.is_empty())
            .map(|layer| {
                ctx.compile_bounded_port(layer)
                    .and_then(|output| output.into_raster(layer.id, &layer.port))
            })
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
        let program = ctx.builder_mut().program_for(
            lumen_gpu::NodeKey(self.id.0),
            lumen_gpu::ProgramDesc::Compute(lumen_gpu::ComputeProgramDesc {
                label: Some("raster-multimerge".to_string()),
                shader: SHADER.to_string(),
                entry: "cs_main".to_string(),
                bind_groups: lumen_gpu::BindGroupLayoutSpec::single(vec![
                    lumen_gpu::BindingLayoutEntry::texture(
                        0,
                        lumen_gpu::wgpu::ShaderStages::COMPUTE,
                    ),
                    lumen_gpu::BindingLayoutEntry::texture(
                        1,
                        lumen_gpu::wgpu::ShaderStages::COMPUTE,
                    ),
                    lumen_gpu::BindingLayoutEntry::uniform(
                        2,
                        lumen_gpu::wgpu::ShaderStages::COMPUTE,
                    ),
                    lumen_gpu::BindingLayoutEntry::storage_texture(
                        3,
                        lumen_gpu::wgpu::ShaderStages::COMPUTE,
                        lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
                        lumen_gpu::wgpu::StorageTextureAccess::WriteOnly,
                    ),
                ]),
            }),
        );

        // Passes alternate between two textures. Each of the first two writes its whole
        // texture; after that a texture still holds the stack from two passes before, which
        // differs from the stack now only within this overlay's texels and the last one's.
        let mut targets = Vec::new();
        let mut current = first;
        let mut compiled_layers = Vec::new();
        for (index, overlay) in overlays.iter().enumerate() {
            let params = ctx.builder_mut().buffer_for(
                lumen_gpu::NodeKey(self.id.0),
                Some(format!("raster-multimerge:{}:params-{index}", self.id.0)),
                lumen_gpu::BufferDesc::uniform(
                    std::mem::size_of::<compiler::RasterMultiMergeParams>() as u64,
                ),
            );
            ctx.builder_mut().param(
                lumen_gpu::ParamKey {
                    owner: lumen_gpu::NodeKey(self.id.0),
                    slot: index as u32,
                },
                lumen_gpu::ParamTarget::Buffer(params),
            );
            if targets.len() <= index % 2 {
                let texture = ctx.builder_mut().texture_for(
                    lumen_gpu::NodeKey(self.id.0),
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
                base_straight: current.metadata.alpha_mode == AlphaMode::Unpremultiplied,
                overlay_straight: overlay.metadata.alpha_mode == AlphaMode::Unpremultiplied,
                origins: [
                    start,
                    origin(&domain),
                    origin(&current.domain),
                    origin(&overlay.domain),
                ],
            });
            ctx.builder_mut().compute_pass(lumen_gpu::ComputePassDesc {
                label: Some(format!("raster-multimerge:{}:layer-{index}", self.id.0)),
                owner: Some(lumen_gpu::NodeKey(self.id.0)),
                program,
                bindings: vec![
                    lumen_gpu::Binding::sampled_texture(0, 0, current.texture),
                    lumen_gpu::Binding::sampled_texture(0, 1, overlay.texture),
                    lumen_gpu::Binding::uniform(0, 2, params),
                    lumen_gpu::Binding::storage_texture(0, 3, texture),
                ],
                dispatch: compiler::dispatch_for(changed.size()).into(),
            });
            current = RasterHandle {
                texture,
                domain,
                metadata: RasterMetadata {
                    alpha_mode: AlphaMode::Premultiplied,
                    ..current.metadata
                },
            };
        }
        ctx.register_compiled_node(RasterMultiCompiledMerge {
            node_id: self.id,
            params: self.params.clone(),
            layers: compiled_layers,
        });

        Ok(CompiledOutput::Raster(current))
    }
}
