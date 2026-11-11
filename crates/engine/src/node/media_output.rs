use crate::node::NodeId;

const SHADER: &str = concat!(
    include_str!("../gpu/shaders/alpha.wgsl"),
    include_str!("../gpu/shaders/color.wgsl"),
    include_str!("../gpu/shaders/dither.wgsl"),
    include_str!("media_output.wgsl")
);

/// Converts its source into the final composition output: sRGB-encoded and quantised once to the
/// output's format.
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

/// `media_output.wgsl`'s `Output`.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct OutputParams {
    pub(crate) origin: [i32; 2],
    pub(crate) opaque: u32,
    pub(crate) dither: u32,
    pub(crate) levels: f32,
    pub(crate) srgb_target: u32,
    pub(crate) _pad: [u32; 2],
}

/// The highest code of a format's colour channels, or 0 for a float format, which takes the output
/// unquantised.
fn levels(format: lumen_gpu::wgpu::TextureFormat) -> f32 {
    use lumen_gpu::wgpu::TextureFormat;
    match format {
        TextureFormat::Rgba8Unorm
        | TextureFormat::Rgba8UnormSrgb
        | TextureFormat::Bgra8Unorm
        | TextureFormat::Bgra8UnormSrgb => 255.0,
        _ => 0.0,
    }
}

fn target(format: lumen_gpu::wgpu::TextureFormat) -> lumen_gpu::RenderTargetSpec {
    lumen_gpu::RenderTargetSpec::flat(format, lumen_gpu::wgpu::BlendState::REPLACE)
}

impl crate::gpu::frame_graph::RecordNode for MediaOutput {
    fn programs(&self, programs: &mut crate::gpu::frame_graph::Programs<'_>) -> crate::Result<()> {
        let format = programs.output_format();
        programs.render(
            "media-output",
            SHADER,
            "vs_main",
            "fs_main",
            RECORD_LAYOUT,
            target(format),
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
        let encoding = cx.output_encoding();
        let tex = cx.output_target(size, format);
        let program = cx.render_program(
            "media-output",
            SHADER,
            "vs_main",
            "fs_main",
            RECORD_LAYOUT,
            target(format),
            0,
        )?;
        let output = cx.rec.uniform(&OutputParams {
            origin: source.origin(),
            opaque: u32::from(encoding.opaque),
            dither: u32::from(encoding.dither),
            levels: levels(format),
            srgb_target: u32::from(format.is_srgb()),
            _pad: [0; 2],
        });
        cx.rec.draw(
            program,
            &[(0, lumen_gpu::Res::Tex(source.tex)), (1, output)],
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
