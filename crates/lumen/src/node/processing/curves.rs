use crate::node::{NodeId, NodeProperty, PortRef};

use crate::gpu::{
    BoundFrame, CompiledOutput, FrameBindContext, FrameBinding, GpuCompileNode, GpuFrameBindNode,
    RasterHandle, compiler,
};

pub(crate) const SHADER: &str = include_str!("curves.wgsl");

#[derive(Debug, Clone, lumen_macros::Node)]
#[node(
    kind = "curves",
    label = "Curves",
    description = "Applies a 1D RGB curve table to a raster.",
    category = "processing"
)]
pub struct Curves {
    pub id: NodeId,
    #[property(kind = "string")]
    pub curve_source: NodeProperty,
    #[property(kind = "float")]
    pub strength: NodeProperty,
    #[input()]
    pub source: PortRef,
}

impl Default for Curves {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            curve_source: NodeProperty::String("identity".to_string()),
            strength: NodeProperty::Float(1.0),
            source: PortRef::empty(),
        }
    }
}

impl GpuCompileNode for Curves {
    fn compile_gpu(
        &self,
        ctx: &mut crate::gpu::CompileContext<'_>,
        port: &PortRef,
    ) -> crate::Result<CompiledOutput> {
        if port.port != "output" {
            return Err(ctx.missing_output(self.id, &port.port));
        }

        let source = ctx
            .compile_port(&self.source)?
            .into_raster(self.source.id, &self.source.port)?;
        let size = source.domain.storage_size;
        let texture = ctx.builder_mut().texture_for(
            lumen_gpu::NodeKey(self.id.0),
            Some(format!("curves:{}:output", self.id.0)),
            lumen_gpu::TextureDesc::storage(size, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm),
        );
        let params = ctx.builder_mut().buffer_for(
            lumen_gpu::NodeKey(self.id.0),
            Some(format!("curves:{}:params", self.id.0)),
            lumen_gpu::BufferDesc::uniform(std::mem::size_of::<compiler::CurvesParams>() as u64),
        );
        let curve = ctx.builder_mut().buffer_for(
            lumen_gpu::NodeKey(self.id.0),
            Some(format!("curves:{}:table", self.id.0)),
            lumen_gpu::BufferDesc::storage(std::mem::size_of::<compiler::CurvesTable>() as u64),
        );
        let program = ctx.builder_mut().program_for(
            lumen_gpu::NodeKey(self.id.0),
            lumen_gpu::ProgramDesc::Compute(lumen_gpu::ComputeProgramDesc {
                label: Some("curves".to_string()),
                shader: SHADER.to_string(),
                entry: "cs_main".to_string(),
                bind_groups: lumen_gpu::BindGroupLayoutSpec::single(vec![
                    lumen_gpu::BindingLayoutEntry::texture(
                        0,
                        lumen_gpu::wgpu::ShaderStages::COMPUTE,
                    ),
                    lumen_gpu::BindingLayoutEntry::uniform(
                        1,
                        lumen_gpu::wgpu::ShaderStages::COMPUTE,
                    ),
                    lumen_gpu::BindingLayoutEntry::storage(
                        2,
                        lumen_gpu::wgpu::ShaderStages::COMPUTE,
                        true,
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
        ctx.builder_mut().compute_pass(lumen_gpu::ComputePassDesc {
            label: Some(format!("curves:{}:apply", self.id.0)),
            owner: Some(lumen_gpu::NodeKey(self.id.0)),
            program,
            bindings: vec![
                lumen_gpu::Binding::sampled_texture(0, 0, source.texture),
                lumen_gpu::Binding::uniform(0, 1, params),
                lumen_gpu::Binding::storage_buffer(0, 2, curve),
                lumen_gpu::Binding::storage_texture(0, 3, texture),
            ],
            dispatch: compiler::dispatch_for(size),
        });
        ctx.builder_mut().param(
            lumen_gpu::ParamKey {
                owner: lumen_gpu::NodeKey(self.id.0),
                slot: 0,
            },
            lumen_gpu::ParamTarget::Buffer(params),
        );
        ctx.builder_mut().param(
            lumen_gpu::ParamKey {
                owner: lumen_gpu::NodeKey(self.id.0),
                slot: 1,
            },
            lumen_gpu::ParamTarget::Buffer(curve),
        );
        ctx.push_frame_binding(FrameBinding::Curves {
            node_id: self.id,
            curve_source: self.curve_source.clone(),
            strength: self.strength.clone(),
            params_buffer: params,
            curve_buffer: curve,
        });

        Ok(CompiledOutput::Raster(RasterHandle {
            texture,
            domain: source.domain,
            metadata: source.metadata,
        }))
    }
}

impl GpuFrameBindNode for Curves {
    fn bind_gpu_frame(
        &self,
        ctx: &FrameBindContext<'_>,
        binding: &FrameBinding,
        bound: &mut BoundFrame,
    ) -> crate::Result<()> {
        let FrameBinding::Curves {
            node_id,
            curve_source,
            strength,
            params_buffer,
            curve_buffer,
        } = binding
        else {
            return Ok(());
        };
        let curve_source = curve_source.resolve_string(
            *node_id,
            "curve_source",
            &ctx.expr_context(*node_id, "curve_source"),
        )?;
        let params = compiler::CurvesParams {
            values: [
                strength.resolve_float(
                    *node_id,
                    "strength",
                    &ctx.expr_context(*node_id, "strength"),
                )? as f32,
                0.0,
                0.0,
                0.0,
            ],
        };
        let curve = compiler::CurvesTable::parse(*node_id, ctx.frame(), &curve_source)?;
        bound.write_buffer(*params_buffer, 0, bytemuck::bytes_of(&params));
        bound.write_buffer(*curve_buffer, 0, bytemuck::bytes_of(&curve));
        Ok(())
    }
}
