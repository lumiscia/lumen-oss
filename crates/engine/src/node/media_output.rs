use crate::node::{NodeId, PortRef};

use crate::gpu::{CompiledOutput, GpuCompileNode, RasterHandle};

const SHADER: &str = include_str!("media_output.wgsl");

/// Copies the compiled raster into the final composition output.
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(kind = "media_output", name = "Media Output", category = "output")]
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
            ctx.composition().render_settings().width.max(1),
            ctx.composition().render_settings().height.max(1),
        );
        let output_format = ctx.output_format();
        let source_is_written =
            ctx.builder_mut()
                .texture_desc(source.texture)
                .is_some_and(|desc| {
                    desc.usage.intersects(
                        lumen_gpu::wgpu::TextureUsages::STORAGE_BINDING
                            | lumen_gpu::wgpu::TextureUsages::RENDER_ATTACHMENT,
                    )
                });
        // External output textures stand in for the output, so a pass has to write it: uploaded
        // media is copied, and a memo's copy stays its own.
        if output_format == lumen_gpu::wgpu::TextureFormat::Rgba8Unorm
            && source.domain == lumen_gpu::TextureDomain::full_frame(size)
            && source_is_written
            && !ctx.is_memo_texture(source.texture)
        {
            return Ok(CompiledOutput::Raster(source));
        }

        let output = ctx.builder_mut().texture(
            Some("media-output:final".to_string()),
            lumen_gpu::TextureDesc::render_target(size, output_format),
        );
        let program = ctx.builder_mut().program(lumen_gpu::ProgramDesc::Render(
            lumen_gpu::RenderProgramDesc {
                label: Some("media-output".to_string()),
                shader: SHADER.to_string(),
                vertex_entry: "vs_main".to_string(),
                fragment_entry: "fs_main".to_string(),
                bind_groups: lumen_gpu::BindGroupLayoutSpec::single(vec![
                    lumen_gpu::BindingLayoutEntry::texture(
                        0,
                        lumen_gpu::wgpu::ShaderStages::FRAGMENT,
                    ),
                ]),
                targets: vec![Some(lumen_gpu::wgpu::ColorTargetState {
                    format: output_format,
                    blend: Some(lumen_gpu::wgpu::BlendState::REPLACE),
                    write_mask: lumen_gpu::wgpu::ColorWrites::ALL,
                })],
                primitive: lumen_gpu::wgpu::PrimitiveState::default(),
            },
        ));
        ctx.builder_mut().render_pass(lumen_gpu::RenderPassDesc {
            label: Some("media-output:render".to_string()),
            program,
            targets: vec![lumen_gpu::RenderTargetRef {
                texture: output,
                load: lumen_gpu::LoadOp::Clear(lumen_gpu::wgpu::Color::TRANSPARENT),
                store: lumen_gpu::wgpu::StoreOp::Store,
            }],
            bindings: vec![lumen_gpu::Binding::sampled_texture(0, 0, source.texture)],
            draw: lumen_gpu::Draw {
                vertices: 0..3,
                instances: 0..1,
            },
            viewport: None,
        });

        Ok(CompiledOutput::Raster(RasterHandle {
            texture: output,
            domain: lumen_gpu::TextureDomain::full_frame(size),
            metadata: source.metadata,
            bounds: None,
        }))
    }
}
