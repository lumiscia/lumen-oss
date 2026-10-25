use crate::gpu::{CompileContext, compiler};
use crate::node::NodeId;

/// The blur passes treat `radius` as the half-width of a box blur with the same
/// variance, `radius / sqrt(3)`, so existing compositions keep a similar look.
const SIGMA_PER_RADIUS: f64 = 0.577_350_269_189_625_8;

/// The Gaussian is truncated at three sigma, so this bounds the kernel to about
/// 1000 texels on each side.
const MAX_SIGMA: f64 = 340.0;

/// Converts a user-facing blur radius in pixels to the Gaussian standard deviation.
pub(crate) fn sigma_for_radius(radius: f64) -> f32 {
    (radius.max(0.0) * SIGMA_PER_RADIUS).min(MAX_SIGMA) as f32
}

/// Output and params buffer of a compiled separable Gaussian.
pub(crate) struct GaussianPasses {
    pub(crate) output: lumen_gpu::TextureId,
    pub(crate) params: lumen_gpu::BufferId,
}

/// Compiles the horizontal then vertical pass shared by `blur` and `shadow`.
///
/// `shader` defines `horizontal_main` and `vertical_main` with this layout:
/// `0` input texture, `1` original source texture, `2` params uniform,
/// `3` linear sampler, `4` rgba16float output (horizontal) or `5` rgba8unorm
/// output (vertical). The horizontal pass reads the source and the vertical
/// pass reads the horizontal result, so the intermediate keeps float precision.
pub(crate) fn compile_passes(
    ctx: &mut CompileContext<'_>,
    node_id: NodeId,
    label: &str,
    shader: &str,
    source: lumen_gpu::TextureId,
    size: lumen_gpu::Size,
    params_size: u64,
) -> GaussianPasses {
    use lumen_gpu::wgpu::{ShaderStages, StorageTextureAccess, TextureFormat};

    let owner = lumen_gpu::NodeKey(node_id.0);
    let builder = ctx.builder_mut();
    let intermediate = builder.texture_for(
        owner,
        Some(format!("{label}:{}:horizontal", node_id.0)),
        lumen_gpu::TextureDesc::storage(size, TextureFormat::Rgba16Float),
    );
    let output = builder.texture_for(
        owner,
        Some(format!("{label}:{}:output", node_id.0)),
        lumen_gpu::TextureDesc::storage(size, TextureFormat::Rgba8Unorm),
    );
    let params = builder.buffer_for(
        owner,
        Some(format!("{label}:{}:params", node_id.0)),
        lumen_gpu::BufferDesc::uniform(params_size),
    );
    let sampler = builder.sampler(
        Some(format!("{label}:{}:linear", node_id.0)),
        lumen_gpu::wgpu::SamplerDescriptor {
            mag_filter: lumen_gpu::wgpu::FilterMode::Linear,
            min_filter: lumen_gpu::wgpu::FilterMode::Linear,
            ..Default::default()
        },
    );

    let layout = |output_binding: u32, format: TextureFormat| {
        lumen_gpu::BindGroupLayoutSpec::single(vec![
            lumen_gpu::BindingLayoutEntry::texture(0, ShaderStages::COMPUTE),
            lumen_gpu::BindingLayoutEntry::texture(1, ShaderStages::COMPUTE),
            lumen_gpu::BindingLayoutEntry::uniform(2, ShaderStages::COMPUTE),
            lumen_gpu::BindingLayoutEntry::sampler(3, ShaderStages::COMPUTE),
            lumen_gpu::BindingLayoutEntry::storage_texture(
                output_binding,
                ShaderStages::COMPUTE,
                format,
                StorageTextureAccess::WriteOnly,
            ),
        ])
    };
    let mut pass = |name: &str,
                    entry: &str,
                    layout: Vec<lumen_gpu::BindGroupLayoutSpec>,
                    input: lumen_gpu::TextureId,
                    output_binding: u32,
                    target: lumen_gpu::TextureId| {
        let program = builder.program_for(
            owner,
            lumen_gpu::ProgramDesc::Compute(lumen_gpu::ComputeProgramDesc {
                label: Some(format!("{label}-{name}")),
                shader: shader.to_string(),
                entry: entry.to_string(),
                bind_groups: layout,
            }),
        );
        builder.compute_pass(lumen_gpu::ComputePassDesc {
            label: Some(format!("{label}:{}:{name}", node_id.0)),
            owner: Some(owner),
            program,
            bindings: vec![
                lumen_gpu::Binding::sampled_texture(0, 0, input),
                lumen_gpu::Binding::sampled_texture(0, 1, source),
                lumen_gpu::Binding::uniform(0, 2, params),
                lumen_gpu::Binding::sampler(0, 3, sampler),
                lumen_gpu::Binding::storage_texture(0, output_binding, target),
            ],
            dispatch: compiler::dispatch_for(size).into(),
        });
    };
    pass(
        "horizontal",
        "horizontal_main",
        layout(4, TextureFormat::Rgba16Float),
        source,
        4,
        intermediate,
    );
    pass(
        "vertical",
        "vertical_main",
        layout(5, TextureFormat::Rgba8Unorm),
        intermediate,
        5,
        output,
    );

    builder.param(
        lumen_gpu::ParamKey { owner, slot: 0 },
        lumen_gpu::ParamTarget::Buffer(params),
    );
    GaussianPasses { output, params }
}
