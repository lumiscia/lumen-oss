use crate::node::{NodeId, NodeProperty, PortRef};

use crate::gpu::{
    BoundFrame, CompiledOutput, FrameBindContext, FrameBinding, GpuCompileNode, GpuFrameBindNode,
    RasterHandle, compiler,
};

pub(crate) const SHADER: &str = include_str!("raster_multimerge.wgsl");

/// Composites a variadic stack of raster layers in order.
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(
    kind = "raster_multimerge",
    name = "Raster Multi Merge",
    category = "compositing"
)]
pub struct RasterMultiMerge {
    pub id: NodeId,
    /// Opacity applied to each layer as it is composited.
    #[property(kind = "float", min = 0, max = 1, step = 0.05)]
    pub opacity: NodeProperty,
    /// Blend mode used for each layer in the stack.
    #[property(kind = "enum", enum_type = crate::node::compositing::BlendMode)]
    pub blend_mode: NodeProperty,
    #[input(optional, variadic)]
    pub layers: Vec<PortRef>,
}

impl Default for RasterMultiMerge {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            opacity: NodeProperty::Float(1.0),
            blend_mode: NodeProperty::Int(0),
            layers: Vec::new(),
        }
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
                dispatch: compiler::dispatch_for(size),
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
        ctx.push_frame_binding(FrameBinding::RasterMultiMerge {
            node_id: self.id,
            opacity: self.opacity.clone(),
            blend_mode: self.blend_mode.clone(),
            buffer: params,
        });

        Ok(CompiledOutput::Raster(current))
    }
}

impl GpuFrameBindNode for RasterMultiMerge {
    fn bind_gpu_frame(
        &self,
        ctx: &FrameBindContext<'_>,
        binding: &FrameBinding,
        bound: &mut BoundFrame,
    ) -> crate::Result<()> {
        let FrameBinding::RasterMultiMerge {
            node_id,
            opacity,
            blend_mode,
            buffer,
        } = binding
        else {
            return Ok(());
        };
        let params = compiler::RasterMultiMergeParams {
            values: [
                opacity.resolve_float(
                    *node_id,
                    "opacity",
                    &ctx.expr_context(*node_id, "opacity"),
                )? as f32,
                blend_mode.resolve_int(
                    *node_id,
                    "blend_mode",
                    &ctx.expr_context(*node_id, "blend_mode"),
                )? as f32,
                0.0,
                0.0,
            ],
        };
        bound.write_buffer(*buffer, 0, bytemuck::bytes_of(&params));
        Ok(())
    }
}
