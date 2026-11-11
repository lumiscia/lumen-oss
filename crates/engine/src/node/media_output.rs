use crate::gpu::params;
use crate::node::NodeId;

const SHADER: &str = include_str!("media_output.wgsl");

/// Copies its source into the final composition output.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(kind = "media_output", name = "Media Output", category = "output")]
pub struct MediaOutput {
    pub id: NodeId,
    #[input()]
    pub source: Option<NodeId>,
}

const RECORD_LAYOUT: &[lumen_gpu::LayoutEntry] = &[
    lumen_gpu::LayoutEntry::staged(
        0,
        lumen_gpu::wgpu::ShaderStages::FRAGMENT,
        lumen_gpu::LayoutKind::Texture,
    ),
    lumen_gpu::LayoutEntry::staged(
        1,
        lumen_gpu::wgpu::ShaderStages::FRAGMENT,
        lumen_gpu::LayoutKind::Uniform,
    ),
];

impl crate::gpu::frame_graph::RecordNode for MediaOutput {
    fn programs(&self, programs: &mut crate::gpu::frame_graph::Programs<'_>) -> crate::Result<()> {
        let format = programs.output_format();
        programs.render(
            "media-output",
            SHADER,
            "vs_main",
            "fs_main",
            RECORD_LAYOUT,
            lumen_gpu::RenderTargetSpec::flat(format, lumen_gpu::wgpu::BlendState::REPLACE),
            0,
        )
    }

    fn record(
        &self,
        cx: &mut crate::gpu::frame_graph::FrameCx<'_>,
    ) -> crate::Result<crate::gpu::frame_graph::Raster> {
        let source = cx.required(self.id, "source", self.source)?;
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
            lumen_gpu::RenderTargetSpec::flat(format, lumen_gpu::wgpu::BlendState::REPLACE),
            0,
        )?;
        let origin = cx.rec.uniform(&params::SourceOriginParams {
            origin: source.origin(),
        });
        cx.rec.draw(
            program,
            &[(0, lumen_gpu::Res::Tex(source.tex)), (1, origin)],
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
