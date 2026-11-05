use crate::gpu::params;
use crate::node::motion::MAX_BOUNDED_MAGNITUDE;

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

/// Per-frame inputs of a Gaussian.
pub(crate) struct GaussianSettings {
    /// Blur radius in pixels, as exposed on the node.
    pub(crate) radius: f64,
    /// Texels outside the source read as transparent instead of clamping to the edge.
    pub(crate) transparent_edge: bool,
    /// Shifts the source by this many pixels, rounded to whole pixels.
    pub(crate) offset: [f64; 2],
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

const RECORD_LEVEL_LAYOUT: &[lumen_gpu::LayoutEntry] = &[
    lumen_gpu::LayoutEntry::compute(0, lumen_gpu::LayoutKind::Texture),
    lumen_gpu::LayoutEntry::compute(3, lumen_gpu::LayoutKind::Uniform),
    lumen_gpu::LayoutEntry::compute(4, lumen_gpu::LayoutKind::Sampler),
    lumen_gpu::LayoutEntry::compute(
        5,
        lumen_gpu::LayoutKind::StorageTexture(lumen_gpu::wgpu::TextureFormat::Rgba16Float),
    ),
];
const RECORD_RESOLVE_LAYOUT: &[lumen_gpu::LayoutEntry] = &[
    lumen_gpu::LayoutEntry::compute(0, lumen_gpu::LayoutKind::Texture),
    lumen_gpu::LayoutEntry::compute(1, lumen_gpu::LayoutKind::Texture),
    lumen_gpu::LayoutEntry::compute(2, lumen_gpu::LayoutKind::Texture),
    lumen_gpu::LayoutEntry::compute(3, lumen_gpu::LayoutKind::Uniform),
    lumen_gpu::LayoutEntry::compute(4, lumen_gpu::LayoutKind::Sampler),
    lumen_gpu::LayoutEntry::compute(
        6,
        lumen_gpu::LayoutKind::StorageTexture(lumen_gpu::wgpu::TextureFormat::Rgba8Unorm),
    ),
];

fn record_shader(shifted: bool) -> &'static str {
    static SHADERS: [std::sync::OnceLock<String>; 2] =
        [std::sync::OnceLock::new(), std::sync::OnceLock::new()];
    SHADERS[usize::from(shifted)]
        .get_or_init(|| format!("{}{SHADER_BODY}", Padding::new(shifted).shader_header()))
}

/// Builds every pass a blur may schedule, whatever its radius.
pub(crate) fn programs(
    programs: &mut crate::gpu::frame_graph::Programs<'_>,
    shifted: bool,
) -> crate::Result<()> {
    let shader = record_shader(shifted);
    for entry in [
        "horizontal_main",
        "quarter_down_main",
        "quarter_horizontal_main",
        "quarter_vertical_main",
        "sixteenth_down_main",
        "sixteenth_horizontal_main",
        "sixteenth_vertical_main",
    ] {
        programs.compute("gaussian", shader, entry, RECORD_LEVEL_LAYOUT)?;
    }
    programs.compute("gaussian", shader, "resolve_main", RECORD_RESOLVE_LAYOUT)
}

/// Records the blur of `source` into a texture of `output`'s size, scheduling only the levels
/// this radius reads.
///
/// `output` must cover everything the blur writes, which is the source's data spread by
/// [`blur_bounds`]; the texels it leaves out are read as transparent, which they are only
/// when no blur reaches them.
///
/// Small radii blur at full resolution. Larger ones are box-reduced by 4 or 16, blurred there
/// with a bounded kernel and sampled back bilinearly, so the cost stays flat as the radius grows.
/// Neighboring levels are crossfaded around the switch. The reduced levels reduce straight from
/// `source`, so they never depend on each other.
pub(crate) fn record_passes(
    cx: &mut crate::gpu::frame_graph::FrameCx<'_>,
    source: crate::gpu::frame_graph::Raster,
    output: lumen_gpu::TextureDomain,
    shifted: bool,
    settings: &GaussianSettings,
) -> crate::Result<lumen_gpu::TexRef> {
    use lumen_gpu::Res;

    let region = output.data_rect;
    let size = output.storage_size;
    let sigma = sigma_for_radius(settings.radius);
    let padding = Padding::new(shifted);
    let shader = record_shader(shifted);
    let flag = |value: bool| if value { 1.0 } else { 0.0 };
    let params = params::BlurParams {
        values: [sigma, flag(settings.transparent_edge), 0.0, 0.0],
        offset: [
            settings.offset[0].round() as f32,
            settings.offset[1].round() as f32,
            0.0,
            0.0,
        ],
        region: [region.x, region.y, 0, 0],
        source: [
            source.domain.data_rect.x,
            source.domain.data_rect.y,
            source.domain.display_size().width as i32,
            source.domain.display_size().height as i32,
        ],
    };
    let uniform = cx.rec.uniform(&params);
    let sampler = cx.sampler(1, || lumen_gpu::wgpu::SamplerDescriptor {
        mag_filter: lumen_gpu::wgpu::FilterMode::Linear,
        min_filter: lumen_gpu::wgpu::FilterMode::Linear,
        ..Default::default()
    });
    let sampler = Res::Sampler(sampler);

    let level = |cx: &mut crate::gpu::frame_graph::FrameCx<'_>,
                 entry: &'static str,
                 input: lumen_gpu::TexRef,
                 target: lumen_gpu::TexRef,
                 target_size: lumen_gpu::Size|
     -> crate::Result<()> {
        let program = cx.program("gaussian", shader, entry, RECORD_LEVEL_LAYOUT)?;
        cx.dispatch(
            program,
            &[
                (0, Res::Tex(input)),
                (3, uniform),
                (4, sampler),
                (5, Res::Tex(target)),
            ],
            params::dispatch_for(target_size),
        );
        Ok(())
    };

    let unused = cx.unused_float();
    let mut full = unused;
    if f64::from(sigma) < QUARTER_BAND.1 {
        let padded_size = lumen_gpu::Size::new(size.width, size.height + 2 * padding.full);
        full = cx.float_target(padded_size);
        level(cx, "horizontal_main", source.tex, full, padded_size)?;
    }
    let reduced = |cx: &mut crate::gpu::frame_graph::FrameCx<'_>,
                   used: bool,
                   factor: u32,
                   pad: u32,
                   entries: [&'static str; 3]|
     -> crate::Result<lumen_gpu::TexRef> {
        if !used {
            return Ok(unused);
        }
        let level_size = padded(region, factor, pad);
        let a = cx.float_target(level_size);
        let b = cx.float_target(level_size);
        level(cx, entries[0], source.tex, a, level_size)?;
        level(cx, entries[1], a, b, level_size)?;
        level(cx, entries[2], b, a, level_size)?;
        Ok(a)
    };
    let quarter = reduced(
        cx,
        f64::from(sigma) > QUARTER_BAND.0 && f64::from(sigma) < SIXTEENTH_BAND.1,
        QUARTER_FACTOR,
        padding.quarter,
        [
            "quarter_down_main",
            "quarter_horizontal_main",
            "quarter_vertical_main",
        ],
    )?;
    let sixteenth = reduced(
        cx,
        f64::from(sigma) > SIXTEENTH_BAND.0,
        SIXTEENTH_FACTOR,
        padding.sixteenth,
        [
            "sixteenth_down_main",
            "sixteenth_horizontal_main",
            "sixteenth_vertical_main",
        ],
    )?;
    let target = cx.storage_target(size);
    let program = cx.program("gaussian", shader, "resolve_main", RECORD_RESOLVE_LAYOUT)?;
    cx.dispatch(
        program,
        &[
            (0, Res::Tex(full)),
            (1, Res::Tex(quarter)),
            (2, Res::Tex(sixteenth)),
            (3, uniform),
            (4, sampler),
            (6, Res::Tex(target)),
        ],
        params::dispatch_for(size),
    );
    Ok(target)
}
