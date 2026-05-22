use crate::node::{NodeId, NodeParamEvalContext, NodeParams, PortRef};

use crate::gpu::{
    BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuCompiledNode, RasterHandle,
    compiler,
};

pub(crate) const SHADER: &str = include_str!("raster_multimerge.wgsl");

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
    buffer: lumen_gpu::BufferId,
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
        let params = compiler::RasterMultiMergeParams {
            values: [evaluated.opacity as f32, evaluated.blend_mode as f32, 0.0, 0.0],
        };
        bound.write_buffer(self.buffer, 0, bytemuck::bytes_of(&params));
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

        let mut layers = self
            .layers
            .iter()
            .filter(|layer| !layer.is_empty())
            .map(|layer| {
                ctx.compile_port(layer)
                    .and_then(|output| output.into_raster(layer.id, &layer.port))
            })
            .collect::<crate::Result<Vec<_>>>()?
            .into_iter();
        let Some(first) = layers.next() else {
            return Ok(ctx.compile_transparent(self.id));
        };

        let size = first.domain.storage_size;
        let params = ctx.builder_mut().buffer_for(
            lumen_gpu::NodeKey(self.id.0),
            Some(format!("raster-multimerge:{}:params", self.id.0)),
            lumen_gpu::BufferDesc::uniform(
                std::mem::size_of::<compiler::RasterMultiMergeParams>() as u64
            ),
        );
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

        let mut current = first;
        for (index, overlay) in layers.enumerate() {
            let texture = ctx.builder_mut().texture_for(
                lumen_gpu::NodeKey(self.id.0),
                Some(format!("raster-multimerge:{}:layer-{index}", self.id.0)),
                lumen_gpu::TextureDesc::storage(size, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm),
            );
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
                dispatch: compiler::dispatch_for(size).into(),
            });
            current = RasterHandle {
                texture,
                domain: lumen_gpu::TextureDomain::full_frame(size),
                metadata: current.metadata,
            };
        }
        ctx.builder_mut().param(
            lumen_gpu::ParamKey {
                owner: lumen_gpu::NodeKey(self.id.0),
                slot: 0,
            },
            lumen_gpu::ParamTarget::Buffer(params),
        );
        ctx.register_compiled_node(RasterMultiCompiledMerge {
            node_id: self.id,
            params: self.params.clone(),
            buffer: params,
        });

        Ok(CompiledOutput::Raster(current))
    }
}
