use crate::node::NodeId;

use crate::gpu::{GpuCompileNode, RasterHandle};

const SHADER: &str = include_str!("media_output.wgsl");

/// Copies the compiled raster into the final composition output.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(kind = "media_output", name = "Media Output", category = "output")]
pub struct MediaOutput {
    pub id: NodeId,
    #[input()]
    pub source: Option<NodeId>,
}

impl GpuCompileNode for MediaOutput {
    fn compile_gpu(&self, ctx: &mut crate::gpu::CompileContext<'_>) -> crate::Result<RasterHandle> {
        let source = ctx.compile_input(self.id, "source", self.source)?;
        let settings = ctx.composition().render_settings();
        let size = lumen_gpu::Size::new(settings.width, settings.height);
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
        // media is copied.
        if output_format == lumen_gpu::wgpu::TextureFormat::Rgba8Unorm
            && source.domain == lumen_gpu::TextureDomain::full_frame(size)
            && source_is_written
        {
            return Ok(source);
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

        Ok(RasterHandle {
            texture: output,
            domain: lumen_gpu::TextureDomain::full_frame(size),
            bounds: None,
        })
    }
}

const RECORD_LAYOUT: &[lumen_gpu::LayoutEntry] = &[lumen_gpu::LayoutEntry::staged(
    0,
    lumen_gpu::wgpu::ShaderStages::FRAGMENT,
    lumen_gpu::LayoutKind::Texture,
)];

impl crate::gpu::frame_graph::RecordNode for MediaOutput {
    fn programs(&self, programs: &mut crate::gpu::frame_graph::Programs<'_>) -> crate::Result<()> {
        let format = programs.output_format();
        programs.render(
            "media-output",
            SHADER,
            "vs_main",
            "fs_main",
            RECORD_LAYOUT,
            lumen_gpu::RenderTargetSpec {
                format,
                blend: lumen_gpu::wgpu::BlendState::REPLACE,
            },
            0,
        )
    }

    fn record(
        &self,
        cx: &mut crate::gpu::frame_graph::FrameCx<'_>,
    ) -> crate::Result<crate::gpu::frame_graph::Raster> {
        let source = cx.required_full(self.id, "source", self.source)?;
        let settings = cx.composition().render_settings();
        let size = lumen_gpu::Size::new(settings.width.max(1), settings.height.max(1));
        let format = cx.output_format();
        let written = cx.rec.usage(source.tex).intersects(
            lumen_gpu::wgpu::TextureUsages::STORAGE_BINDING
                | lumen_gpu::wgpu::TextureUsages::RENDER_ATTACHMENT,
        );
        if format == lumen_gpu::wgpu::TextureFormat::Rgba8Unorm
            && source.domain == lumen_gpu::TextureDomain::full_frame(size)
            && written
        {
            return Ok(source);
        }
        let tex = cx.output_target(size, format);
        let program = cx.render_program(
            "media-output",
            SHADER,
            "vs_main",
            "fs_main",
            RECORD_LAYOUT,
            lumen_gpu::RenderTargetSpec {
                format,
                blend: lumen_gpu::wgpu::BlendState::REPLACE,
            },
            0,
        )?;
        cx.rec.draw(
            program,
            &[(0, lumen_gpu::Res::Tex(source.tex))],
            tex,
            lumen_gpu::TargetLoad::Clear,
            3,
            1,
            None,
        );
        Ok(crate::gpu::frame_graph::Raster {
            tex,
            domain: lumen_gpu::TextureDomain::full_frame(size),
            bounds: None,
        })
    }
}
