use crate::gpu::{BoundFrame, CompileContext, compiler};
use crate::node::NodeId;

/// `radius` is the half-width of the discrete box blur it replaces. A box of
/// `2r + 1` texels has variance `r(r + 1) / 3`, and the Gaussian matches it.
fn sigma_for_radius(radius: f64) -> f32 {
    let radius = radius.max(0.0);
    ((radius * (radius + 1.0) / 3.0).sqrt()).min(MAX_SIGMA) as f32
}

/// Beyond this the reduced chain would need more than ~65 taps per pass.
const MAX_SIGMA: f64 = 340.0;

/// Reduction factor between the full-resolution, quarter and sixteenth levels.
/// `gaussian.wgsl` picks the level from sigma, so the pass schedule is fixed.
const LEVEL_FACTOR: u32 = 4;

pub(crate) const SHADER: &str = concat!(
    include_str!("../../gpu/shaders/alpha.wgsl"),
    include_str!("gaussian.wgsl")
);

/// Compiled Gaussian passes: write [`GaussianSettings`] to `params` every frame
/// and read the premultiplied RGBA result from `output`.
#[derive(Debug, Clone)]
pub(crate) struct GaussianPasses {
    pub(crate) output: lumen_gpu::TextureId,
    params: lumen_gpu::BufferId,
}

/// Per-frame inputs of a compiled Gaussian.
pub(crate) struct GaussianSettings {
    /// Blur radius in pixels, as exposed on the node.
    pub(crate) radius: f64,
    /// The source is straight alpha and gets premultiplied before filtering.
    pub(crate) straight_source: bool,
    /// Texels outside the source read as transparent instead of clamping to the edge.
    pub(crate) transparent_edge: bool,
    /// Shifts the source by this many pixels, rounded to whole pixels.
    pub(crate) offset: [f64; 2],
}

impl GaussianPasses {
    pub(crate) fn write(&self, bound: &mut BoundFrame, settings: &GaussianSettings) {
        let flag = |value: bool| if value { 1.0 } else { 0.0 };
        let params = compiler::BlurParams {
            values: [
                sigma_for_radius(settings.radius),
                flag(settings.straight_source),
                flag(settings.transparent_edge),
                0.0,
            ],
            offset: [
                settings.offset[0].round() as f32,
                settings.offset[1].round() as f32,
                0.0,
                0.0,
            ],
        };
        bound.write_buffer(self.params, 0, bytemuck::bytes_of(&params));
    }
}

fn reduced(size: lumen_gpu::Size) -> lumen_gpu::Size {
    lumen_gpu::Size::new(
        size.width.div_ceil(LEVEL_FACTOR).max(1),
        size.height.div_ceil(LEVEL_FACTOR).max(1),
    )
}

/// Compiles a separable Gaussian from `source` into a new premultiplied
/// `rgba8unorm` texture of the same size, using the node's param slot 0.
///
/// Small radii blur at full resolution. Larger ones are box-reduced by 4 or 16,
/// blurred there with a bounded kernel and sampled back bilinearly, so the cost
/// stays flat as the radius grows. Every pass is always scheduled; the ones a
/// radius does not need return immediately.
pub(crate) fn compile_passes(
    ctx: &mut CompileContext<'_>,
    node_id: NodeId,
    label: &str,
    source: lumen_gpu::TextureId,
    size: lumen_gpu::Size,
) -> GaussianPasses {
    use lumen_gpu::wgpu::{ShaderStages, StorageTextureAccess, TextureFormat};

    let owner = lumen_gpu::NodeKey(node_id.0);
    let builder = ctx.builder_mut();
    let texture = |builder: &mut lumen_gpu::RenderPlanBuilder,
                   name: &str,
                   size: lumen_gpu::Size,
                   format: TextureFormat| {
        builder.texture_for(
            owner,
            Some(format!("{label}:{}:{name}", node_id.0)),
            lumen_gpu::TextureDesc::storage(size, format),
        )
    };
    let quarter_size = reduced(size);
    let sixteenth_size = reduced(quarter_size);
    let horizontal = texture(builder, "horizontal", size, TextureFormat::Rgba16Float);
    let quarter = [
        texture(
            builder,
            "quarter-down",
            quarter_size,
            TextureFormat::Rgba16Float,
        ),
        texture(
            builder,
            "quarter-horizontal",
            quarter_size,
            TextureFormat::Rgba16Float,
        ),
        texture(
            builder,
            "quarter-vertical",
            quarter_size,
            TextureFormat::Rgba16Float,
        ),
    ];
    let sixteenth = [
        texture(
            builder,
            "sixteenth-down",
            sixteenth_size,
            TextureFormat::Rgba16Float,
        ),
        texture(
            builder,
            "sixteenth-horizontal",
            sixteenth_size,
            TextureFormat::Rgba16Float,
        ),
        texture(
            builder,
            "sixteenth-vertical",
            sixteenth_size,
            TextureFormat::Rgba16Float,
        ),
    ];
    let output = texture(builder, "output", size, TextureFormat::Rgba8Unorm);
    let params = builder.buffer_for(
        owner,
        Some(format!("{label}:{}:params", node_id.0)),
        lumen_gpu::BufferDesc::uniform(std::mem::size_of::<compiler::BlurParams>() as u64),
    );
    let sampler = builder.sampler(
        Some(format!("{label}:{}:linear", node_id.0)),
        lumen_gpu::wgpu::SamplerDescriptor {
            mag_filter: lumen_gpu::wgpu::FilterMode::Linear,
            min_filter: lumen_gpu::wgpu::FilterMode::Linear,
            ..Default::default()
        },
    );

    // Texture inputs bind at 0..=2, params at 3, the sampler at 4, the float
    // output at 5 and the unorm output at 6.
    let mut pass = |entry: &str,
                    inputs: &[lumen_gpu::TextureId],
                    target: lumen_gpu::TextureId,
                    target_size: lumen_gpu::Size| {
        let unorm = target == output;
        let (output_binding, format) = if unorm {
            (6, TextureFormat::Rgba8Unorm)
        } else {
            (5, TextureFormat::Rgba16Float)
        };
        let mut layout: Vec<_> = (0..inputs.len() as u32)
            .map(|binding| lumen_gpu::BindingLayoutEntry::texture(binding, ShaderStages::COMPUTE))
            .collect();
        layout.extend([
            lumen_gpu::BindingLayoutEntry::uniform(3, ShaderStages::COMPUTE),
            lumen_gpu::BindingLayoutEntry::sampler(4, ShaderStages::COMPUTE),
            lumen_gpu::BindingLayoutEntry::storage_texture(
                output_binding,
                ShaderStages::COMPUTE,
                format,
                StorageTextureAccess::WriteOnly,
            ),
        ]);
        let program = builder.program_for(
            owner,
            lumen_gpu::ProgramDesc::Compute(lumen_gpu::ComputeProgramDesc {
                label: Some(format!("{label}-gaussian-{entry}")),
                shader: SHADER.to_string(),
                entry: entry.to_string(),
                bind_groups: lumen_gpu::BindGroupLayoutSpec::single(layout),
            }),
        );
        let mut bindings: Vec<_> = inputs
            .iter()
            .zip(0u32..)
            .map(|(&texture, binding)| lumen_gpu::Binding::sampled_texture(0, binding, texture))
            .collect();
        bindings.extend([
            lumen_gpu::Binding::uniform(0, 3, params),
            lumen_gpu::Binding::sampler(0, 4, sampler),
            lumen_gpu::Binding::storage_texture(0, output_binding, target),
        ]);
        builder.compute_pass(lumen_gpu::ComputePassDesc {
            label: Some(format!("{label}:{}:{entry}", node_id.0)),
            owner: Some(owner),
            program,
            bindings,
            dispatch: compiler::dispatch_for(target_size).into(),
        });
    };

    pass("horizontal_main", &[source], horizontal, size);
    pass("quarter_down_main", &[source], quarter[0], quarter_size);
    pass(
        "quarter_horizontal_main",
        &[quarter[0]],
        quarter[1],
        quarter_size,
    );
    pass(
        "quarter_vertical_main",
        &[quarter[1]],
        quarter[2],
        quarter_size,
    );
    pass(
        "sixteenth_down_main",
        &[quarter[0]],
        sixteenth[0],
        sixteenth_size,
    );
    pass(
        "sixteenth_horizontal_main",
        &[sixteenth[0]],
        sixteenth[1],
        sixteenth_size,
    );
    pass(
        "sixteenth_vertical_main",
        &[sixteenth[1]],
        sixteenth[2],
        sixteenth_size,
    );
    pass(
        "resolve_main",
        &[horizontal, quarter[2], sixteenth[2]],
        output,
        size,
    );

    builder.param(
        lumen_gpu::ParamKey { owner, slot: 0 },
        lumen_gpu::ParamTarget::Buffer(params),
    );
    GaussianPasses { output, params }
}
