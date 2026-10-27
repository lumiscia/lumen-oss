use crate::gpu::{BoundFrame, CompileContext, RasterHandle, compiler};
use crate::node::{NodeId, motion::MAX_BOUNDED_MAGNITUDE};

/// `radius` is the half-width of the discrete box blur it replaces. A box of
/// `2r + 1` texels has variance `r(r + 1) / 3`, and the Gaussian matches it.
fn sigma_for_radius(radius: f64) -> f32 {
    let radius = radius.max(0.0);
    ((radius * (radius + 1.0) / 3.0).sqrt()).min(MAX_SIGMA) as f32
}

/// Beyond this the reduced chain would need more than ~65 taps per pass.
const MAX_SIGMA: f64 = 340.0;

/// Sigma bands in which neighboring levels are crossfaded: the quarter level
/// fades in over `QUARTER_BAND` and replaces the sixteenth over `SIXTEENTH_BAND`.
const QUARTER_BAND: (f64, f64) = (18.0, 22.0);
const SIXTEENTH_BAND: (f64, f64) = (72.0, 88.0);

const QUARTER_FACTOR: u32 = 4;
const SIXTEENTH_FACTOR: u32 = 16;

/// Variance per axis that box reduction and bilinear sampling add, as a fraction
/// of the square of the reduction factor. Mirrored in `gaussian.wgsl`.
const RESAMPLE_VARIANCE: f64 = 0.25;

const SHADER_BODY: &str = concat!(
    include_str!("../../gpu/shaders/alpha.wgsl"),
    include_str!("gaussian.wgsl")
);

/// How far past the canvas, in texels of its own level, each level is padded.
#[derive(Debug, Clone, Copy)]
struct Padding {
    full: u32,
    quarter: u32,
    sixteenth: u32,
}

/// Texels a Gaussian of `sigma` reads on each side, including the last tap
/// pair's offset and the bilinear footprint.
fn kernel_reach(sigma: f64) -> u32 {
    (sigma * 3.0).ceil() as u32 + 3
}

fn reduced_sigma(sigma: f64, factor: u32) -> f64 {
    let factor = f64::from(factor);
    (sigma * sigma - RESAMPLE_VARIANCE * factor * factor)
        .max(0.0)
        .sqrt()
        / factor
}

/// Whether `value` is a number small enough for bounds to be worked out from.
fn within_limit(value: f64) -> bool {
    value.abs() < MAX_BOUNDED_MAGNITUDE
}

/// Canvas pixels past the pixels a blur of `radius` reads that it can still write, at its
/// widest across the levels it may blur on, or `None` when `radius` is too large or not a
/// number to be bounded. A reduced level reaches its kernel and one block more on each side
/// for the box reduction and one for the bilinear read back.
fn blur_reach(radius: f64) -> Option<u32> {
    if !within_limit(radius) {
        return None;
    }
    let sigma = f64::from(sigma_for_radius(radius));
    let level = |factor: u32| factor * (kernel_reach(reduced_sigma(sigma, factor)) + 2);
    let mut reach = kernel_reach(sigma);
    if sigma > QUARTER_BAND.0 {
        reach = reach.max(level(QUARTER_FACTOR));
    }
    if sigma > SIXTEENTH_BAND.0 {
        reach = reach.max(level(SIXTEENTH_FACTOR));
    }
    // One more pixel covers rounding in the sigma and positions the shader works from.
    Some(reach + 1)
}

/// The canvas `[min_x, min_y, max_x, max_y]` that a blur of `radius` writes for a source whose
/// pixels are within `data` and shifted by `offset` whole pixels. `None` when it cannot be
/// bounded.
pub(crate) fn blur_bounds(
    data: lumen_gpu::RectI,
    offset: [f64; 2],
    radius: f64,
) -> Option<[f32; 4]> {
    if !offset.into_iter().all(within_limit) {
        return None;
    }
    let reach = f64::from(blur_reach(radius)?);
    let [dx, dy] = offset.map(f64::round);
    let (x, y) = (f64::from(data.x) + dx, f64::from(data.y) + dy);
    Some(
        [
            x - reach,
            y - reach,
            x + f64::from(data.width) + reach,
            y + f64::from(data.height) + reach,
        ]
        .map(|value| value as f32),
    )
}

/// The domain of a raster displayed at `display` that is written within `bounds`, or across
/// the display without them.
pub(crate) fn domain_within(
    display: lumen_gpu::Size,
    bounds: Option<lumen_gpu::RectI>,
) -> lumen_gpu::TextureDomain {
    match bounds {
        Some(bounds) => lumen_gpu::TextureDomain::bounded(display, bounds),
        None => lumen_gpu::TextureDomain::full_frame(display),
    }
}

impl Padding {
    /// Without a shift the canvas edge extension is all the padding needs: one
    /// block of edge-extended (or transparent) texels. A shifted source can put
    /// content anywhere within the kernel reach of the canvas, so every level is
    /// padded by the reach of the largest sigma it blurs.
    fn new(shifted: bool) -> Self {
        if !shifted {
            return Self {
                full: 0,
                quarter: 1,
                sixteenth: 1,
            };
        }
        Self {
            full: kernel_reach(QUARTER_BAND.1),
            quarter: kernel_reach(reduced_sigma(SIXTEENTH_BAND.1, QUARTER_FACTOR)),
            sixteenth: kernel_reach(reduced_sigma(MAX_SIGMA, SIXTEENTH_FACTOR)),
        }
    }

    /// Constants `gaussian.wgsl` expects before its own source.
    fn shader_header(self) -> String {
        format!(
            "const GAUSSIAN_QUARTER_FROM: f32 = {:?};\n\
             const GAUSSIAN_QUARTER_FULL: f32 = {:?};\n\
             const GAUSSIAN_SIXTEENTH_FROM: f32 = {:?};\n\
             const GAUSSIAN_SIXTEENTH_FULL: f32 = {:?};\n\
             const GAUSSIAN_FULL_PAD: f32 = {:?};\n\
             const GAUSSIAN_QUARTER_PAD: f32 = {:?};\n\
             const GAUSSIAN_SIXTEENTH_PAD: f32 = {:?};\n",
            QUARTER_BAND.0,
            QUARTER_BAND.1,
            SIXTEENTH_BAND.0,
            SIXTEENTH_BAND.1,
            f64::from(self.full),
            f64::from(self.quarter),
            f64::from(self.sixteenth),
        )
    }
}

/// Where a compiled Gaussian reads its per-frame settings from, and what the
/// source may do.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct GaussianOptions {
    /// Param slot of the node that holds the Gaussian's settings buffer.
    pub(crate) param_slot: u32,
    /// The settings may carry a nonzero [`GaussianSettings::offset`].
    pub(crate) shifted: bool,
}

/// Compiled Gaussian passes: write [`GaussianSettings`] to `params` every frame
/// and read the premultiplied RGBA result from `output`.
#[derive(Debug, Clone)]
pub(crate) struct GaussianPasses {
    pub(crate) output: lumen_gpu::TextureId,
    params: lumen_gpu::BufferId,
    shifted: bool,
    /// Canvas pixel of the output's first texel.
    region: [i32; 2],
    /// Canvas pixel of the source's first texel, and the source's display size.
    source: [i32; 4],
}

/// Per-frame inputs of a compiled Gaussian.
pub(crate) struct GaussianSettings {
    /// Blur radius in pixels, as exposed on the node.
    pub(crate) radius: f64,
    /// The source is straight alpha and gets premultiplied before filtering.
    pub(crate) straight_source: bool,
    /// Texels outside the source read as transparent instead of clamping to the edge.
    pub(crate) transparent_edge: bool,
    /// Shifts the source by this many pixels, rounded to whole pixels. Must be
    /// zero unless the passes were compiled with [`GaussianOptions::shifted`].
    pub(crate) offset: [f64; 2],
}

impl GaussianPasses {
    pub(crate) fn write(&self, bound: &mut BoundFrame, settings: &GaussianSettings) {
        debug_assert!(
            self.shifted || settings.offset == [0.0; 2],
            "offset needs passes compiled with `shifted`"
        );
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
            region: [self.region[0], self.region[1], 0, 0],
            source: self.source,
        };
        bound.write_buffer(self.params, 0, bytemuck::bytes_of(&params));
    }
}

/// Texture size of the level reduced by `factor` that covers `region`, padded on every side.
/// Blocks of the level stay aligned to the canvas, so it starts at the block `region` is in.
fn padded(region: lumen_gpu::RectI, factor: u32, pad: u32) -> lumen_gpu::Size {
    let blocks = |origin: i32, extent: u32| {
        let origin = origin.max(0) as u32;
        (origin + extent).div_ceil(factor) - origin / factor
    };
    lumen_gpu::Size::new(
        blocks(region.x, region.width).max(1) + 2 * pad,
        blocks(region.y, region.height).max(1) + 2 * pad,
    )
}

/// Compiles a separable Gaussian from the full-frame `source` into a new premultiplied
/// `rgba8unorm` texture of `size`, using the node's param slot 0 and an unshifted source.
pub(crate) fn compile_passes(
    ctx: &mut CompileContext<'_>,
    node_id: NodeId,
    label: &str,
    source: RasterHandle,
    size: lumen_gpu::Size,
) -> GaussianPasses {
    compile_passes_with(
        ctx,
        node_id,
        label,
        source,
        lumen_gpu::TextureDomain::full_frame(size),
        GaussianOptions::default(),
    )
}

/// [`compile_passes`] with a caller-chosen param slot and optional source shift, writing the
/// pixels of `output`'s data and reading `source` through its domain.
///
/// `output` must cover everything the blur writes, which is the source's data spread by
/// [`blur_bounds`]; the texels it leaves out are read as transparent, which they are only
/// when no blur reaches them.
///
/// Small radii blur at full resolution. Larger ones are box-reduced by 4 or 16,
/// blurred there with a bounded kernel and sampled back bilinearly, so the cost
/// stays flat as the radius grows. Neighboring levels are crossfaded around the
/// switch. Every pass is always scheduled; the ones a radius does not need
/// return immediately.
///
/// Scratch is one full-resolution horizontal texture, a ping-pong pair per
/// reduced level, and the output. The reduced levels reduce straight from
/// `source`, so they never depend on each other.
pub(crate) fn compile_passes_with(
    ctx: &mut CompileContext<'_>,
    node_id: NodeId,
    label: &str,
    source: RasterHandle,
    output: lumen_gpu::TextureDomain,
    options: GaussianOptions,
) -> GaussianPasses {
    use lumen_gpu::wgpu::{ShaderStages, StorageTextureAccess, TextureFormat};

    let owner = lumen_gpu::NodeKey(node_id.0);
    let region = output.data_rect;
    let size = output.storage_size;
    let source_display = source.domain.display_size();
    let source_origin = source.domain.data_rect;
    let source = source.texture;
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
    let padding = Padding::new(options.shifted);
    let shader = format!("{}{SHADER_BODY}", padding.shader_header());
    let float = |builder: &mut lumen_gpu::RenderPlanBuilder, name: &str, size| {
        texture(builder, name, size, TextureFormat::Rgba16Float)
    };
    let horizontal = float(
        builder,
        "horizontal",
        lumen_gpu::Size::new(size.width, size.height + 2 * padding.full),
    );
    let quarter_size = padded(region, QUARTER_FACTOR, padding.quarter);
    let quarter = [
        float(builder, "quarter-a", quarter_size),
        float(builder, "quarter-b", quarter_size),
    ];
    let sixteenth_size = padded(region, SIXTEENTH_FACTOR, padding.sixteenth);
    let sixteenth = [
        float(builder, "sixteenth-a", sixteenth_size),
        float(builder, "sixteenth-b", sixteenth_size),
    ];
    let output = builder.texture_for(
        owner,
        Some(format!("{label}:{}:output", node_id.0)),
        lumen_gpu::TextureDesc::storage(size, TextureFormat::Rgba8Unorm).with_domain(output),
    );
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
                shader: shader.clone(),
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

    let padded_height = lumen_gpu::Size::new(size.width, size.height + 2 * padding.full);
    pass("horizontal_main", &[source], horizontal, padded_height);
    for (prefix, [a, b], level_size) in [
        ("quarter", quarter, quarter_size),
        ("sixteenth", sixteenth, sixteenth_size),
    ] {
        pass(&format!("{prefix}_down_main"), &[source], a, level_size);
        pass(&format!("{prefix}_horizontal_main"), &[a], b, level_size);
        pass(&format!("{prefix}_vertical_main"), &[b], a, level_size);
    }
    pass(
        "resolve_main",
        &[horizontal, quarter[0], sixteenth[0]],
        output,
        size,
    );

    builder.param(
        lumen_gpu::ParamKey {
            owner,
            slot: options.param_slot,
        },
        lumen_gpu::ParamTarget::Buffer(params),
    );
    GaussianPasses {
        output,
        params,
        shifted: options.shifted,
        region: [region.x, region.y],
        source: [
            source_origin.x,
            source_origin.y,
            source_display.width as i32,
            source_display.height as i32,
        ],
    }
}
