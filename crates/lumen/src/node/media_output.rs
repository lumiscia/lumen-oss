use crate::node::{NodeId, PortRef};

use crate::gpu::{
    BoundFrame, CompiledOutput, FrameBindContext, FrameBinding, GpuCompileNode, GpuFrameBindNode,
    RasterHandle, compiler,
};

pub(crate) const SHADER: &str = include_str!("media_output.wgsl");

#[derive(Debug, Clone, lumen_macros::Node)]
#[node(
    kind = "media_output",
    label = "Media Output",
    description = "Copies the compiled raster into the final composition output.",
    category = "output"
)]
pub struct MediaOutput {
    pub id: NodeId,
    #[input()]
    pub source: PortRef,
}

impl Default for MediaOutput {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            source: PortRef::empty(),
        }
    }
}

impl GpuCompileNode for MediaOutput {
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
        let size = lumen_gpu::Size::new(
            ctx.composition().render_settings.width.max(1),
            ctx.composition().render_settings.height.max(1),
        );
        let output = ctx.builder_mut().texture_for(
            lumen_gpu::NodeKey(self.id.0),
            Some("media-output:final".to_string()),
            compiler::copyable_texture_desc(size),
        );
        let program = ctx.builder_mut().program_for(
            lumen_gpu::NodeKey(self.id.0),
            lumen_gpu::ProgramDesc::Compute(lumen_gpu::ComputeProgramDesc {
                label: Some("media-output".to_string()),
                shader: SHADER.to_string(),
                entry: "cs_main".to_string(),
                bind_groups: lumen_gpu::BindGroupLayoutSpec::single(vec![
                    lumen_gpu::BindingLayoutEntry::texture(
                        0,
                        lumen_gpu::wgpu::ShaderStages::COMPUTE,
                    ),
                    lumen_gpu::BindingLayoutEntry::storage_texture(
                        1,
                        lumen_gpu::wgpu::ShaderStages::COMPUTE,
                        lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
                        lumen_gpu::wgpu::StorageTextureAccess::WriteOnly,
                    ),
                ]),
            }),
        );
        ctx.builder_mut().compute_pass(lumen_gpu::ComputePassDesc {
            label: Some("media-output:copy".to_string()),
            owner: Some(lumen_gpu::NodeKey(self.id.0)),
            program,
            bindings: vec![
                lumen_gpu::Binding::sampled_texture(0, 0, source.texture),
                lumen_gpu::Binding::storage_texture(0, 1, output),
            ],
            dispatch: compiler::dispatch_for(size),
        });

        Ok(CompiledOutput::Raster(RasterHandle {
            texture: output,
            domain: lumen_gpu::TextureDomain::full_frame(size),
            metadata: source.metadata,
        }))
    }
}

impl GpuFrameBindNode for MediaOutput {
    fn bind_gpu_frame(
        &self,
        _ctx: &FrameBindContext<'_>,
        _binding: &FrameBinding,
        _bound: &mut BoundFrame,
    ) -> crate::Result<()> {
        Ok(())
    }
}
