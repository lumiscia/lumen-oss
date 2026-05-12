use std::time::{Duration, Instant};

use anyhow::{Context, anyhow};
use cosmic_text::{
    Attrs, Buffer, CacheKey, CacheKeyFlags, Family, FontSystem, Metrics, Shaping, SwashCache,
    Weight,
};
use fdsm::{
    bezier::scanline::FillRule,
    correct_error::{ErrorCorrectionConfig, correct_error_msdf},
    generate::generate_msdf,
    render::correct_sign_msdf,
    shape::Shape,
    transform::Transform,
};
use image::Rgb32FImage;
use nalgebra::{Affine2, Matrix3};
use skrifa::{FontRef, GlyphId, MetadataProvider, prelude::Size, raw::TableProvider};

const MAX_GLYPHS: usize = 4096;
const ROBOTO_BYTES: &[u8] = lumen_text::ROBOTO_REGULAR_BYTES;

#[derive(Debug)]
struct Args {
    iterations: usize,
    case: CaseSelection,
    text_repeats: usize,
    px_range: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaseSelection {
    All,
    Layout,
    Raster,
    #[cfg(feature = "experimental-msdf")]
    GpuMsdf,
    Emoji,
    RawRaster,
    RawFdsm,
    RawFdsmBase,
}

impl CaseSelection {
    fn parse(value: &str) -> anyhow::Result<Self> {
        match value {
            "all" => Ok(Self::All),
            "layout" => Ok(Self::Layout),
            "raster" => Ok(Self::Raster),
            "gpu-msdf" => {
                #[cfg(feature = "experimental-msdf")]
                {
                    Ok(Self::GpuMsdf)
                }
                #[cfg(not(feature = "experimental-msdf"))]
                {
                    Err(anyhow!("gpu-msdf requires --features experimental-msdf"))
                }
            }
            "emoji" => Ok(Self::Emoji),
            "raw-raster" => Ok(Self::RawRaster),
            "raw-fdsm" => Ok(Self::RawFdsm),
            "raw-fdsm-base" => Ok(Self::RawFdsmBase),
            _ => Err(anyhow!("unknown case `{value}`")),
        }
    }

    fn includes(self, case: BenchCase) -> bool {
        self == Self::All
            || matches!(
                (self, case),
                (Self::Layout, BenchCase::Layout)
                    | (Self::Raster, BenchCase::Raster)
                    | (Self::Emoji, BenchCase::Emoji)
                    | (Self::RawRaster, BenchCase::RawRaster)
                    | (Self::RawFdsm, BenchCase::RawFdsm)
                    | (Self::RawFdsmBase, BenchCase::RawFdsmBase)
            )
            || {
                #[cfg(feature = "experimental-msdf")]
                {
                    matches!((self, case), (Self::GpuMsdf, BenchCase::GpuMsdf))
                }
                #[cfg(not(feature = "experimental-msdf"))]
                {
                    false
                }
            }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BenchCase {
    Layout,
    Raster,
    #[cfg(feature = "experimental-msdf")]
    GpuMsdf,
    Emoji,
    RawRaster,
    RawFdsm,
    RawFdsmBase,
}

impl BenchCase {
    fn name(self) -> &'static str {
        match self {
            Self::Layout => "layout",
            Self::Raster => "raster_atlas",
            #[cfg(feature = "experimental-msdf")]
            Self::GpuMsdf => "gpu_msdf_job_atlas",
            Self::Emoji => "emoji_color_atlas",
            Self::RawRaster => "raw_swash_raster_glyph",
            Self::RawFdsm => "raw_fdsm_msdf_glyph",
            Self::RawFdsmBase => "raw_fdsm_msdf_glyph_base",
        }
    }
}

fn main() -> anyhow::Result<()> {
    let args = parse_args()?;
    let atlas_config = lumen_text::AtlasConfig {
        px_range: args.px_range,
        ..lumen_text::AtlasConfig::default()
    };

    for case in [
        BenchCase::Layout,
        BenchCase::Raster,
        #[cfg(feature = "experimental-msdf")]
        BenchCase::GpuMsdf,
        BenchCase::Emoji,
        BenchCase::RawRaster,
        BenchCase::RawFdsm,
        BenchCase::RawFdsmBase,
    ] {
        if !args.case.includes(case) {
            continue;
        }
        let result = run_case(case, args.iterations, args.text_repeats, atlas_config)?;
        println!(
            "text_bench case={} glyphs={} iterations={} cold_ms={} elapsed_ms={} mean_us={:.2}",
            case.name(),
            result.glyph_count,
            args.iterations,
            result.cold.as_millis(),
            result.elapsed.as_millis(),
            result.elapsed.as_secs_f64() * 1_000_000.0 / args.iterations.max(1) as f64,
        );
    }

    Ok(())
}

#[derive(Debug)]
struct BenchResult {
    cold: Duration,
    elapsed: Duration,
    glyph_count: usize,
}

fn run_case(
    case: BenchCase,
    iterations: usize,
    text_repeats: usize,
    atlas_config: lumen_text::AtlasConfig,
) -> anyhow::Result<BenchResult> {
    if matches!(
        case,
        BenchCase::RawRaster | BenchCase::RawFdsm | BenchCase::RawFdsmBase
    ) {
        return run_raw_case(case, iterations);
    }

    let mut system = lumen_text::TextSystem::new();
    let request = request_for_case(case, text_repeats);

    let cold_started = Instant::now();
    let cold_glyph_count = run_once(case, &mut system, &request, atlas_config);
    let cold = cold_started.elapsed();

    let started = Instant::now();
    let mut glyph_count = cold_glyph_count;
    for _ in 0..iterations {
        glyph_count = run_once(case, &mut system, &request, atlas_config);
    }
    Ok(BenchResult {
        cold,
        elapsed: started.elapsed(),
        glyph_count,
    })
}

fn run_once(
    case: BenchCase,
    system: &mut lumen_text::TextSystem,
    request: &lumen_text::TextLayoutRequest,
    atlas_config: lumen_text::AtlasConfig,
) -> usize {
    let layout = system.layout(request);
    let glyph_count = layout.glyphs.len();
    match case {
        BenchCase::Layout => {
            std::hint::black_box(layout);
        }
        BenchCase::Raster => {
            let atlas = system.render_alpha_atlas(&layout, atlas_config, MAX_GLYPHS);
            std::hint::black_box(atlas);
        }
        BenchCase::Emoji => {
            let atlas = system.render_alpha_atlas(&layout, atlas_config, MAX_GLYPHS);
            std::hint::black_box(atlas);
        }
        #[cfg(feature = "experimental-msdf")]
        BenchCase::GpuMsdf => {
            let atlas =
                system.render_gpu_hybrid_atlas(&layout, atlas_config, MAX_GLYPHS, 32768, 262_144);
            std::hint::black_box(atlas);
        }
        BenchCase::RawRaster | BenchCase::RawFdsm | BenchCase::RawFdsmBase => {
            unreachable!("handled before run_once")
        }
    }
    glyph_count
}

fn request_for_case(case: BenchCase, text_repeats: usize) -> lumen_text::TextLayoutRequest {
    let content = match case {
        BenchCase::Emoji => "🍋✨🚀🎬 ".repeat(text_repeats.max(1)),
        BenchCase::RawRaster | BenchCase::RawFdsm | BenchCase::RawFdsmBase => "A".to_string(),
        _ => "Hello from Lumen. Crisp text, ligatures, layout, and atlas generation. "
            .repeat(text_repeats.max(1)),
    };
    let mut request = lumen_text::TextLayoutRequest::new(content);
    request.font_size = 64.0;
    request.font_weight = 700;
    request.max_width = Some(1400.0);
    if case == BenchCase::Emoji {
        request.font_family = "Apple Color Emoji".to_string();
    }
    request
}

fn parse_args() -> anyhow::Result<Args> {
    let mut iterations = 100;
    let mut case = CaseSelection::All;
    let mut text_repeats = 1;
    let mut px_range = lumen_text::AtlasConfig::default().px_range;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--iterations" => {
                iterations = args
                    .next()
                    .ok_or_else(|| anyhow!("--iterations requires a value"))?
                    .parse::<usize>()
                    .context("--iterations must be a positive integer")?;
            }
            "--text-repeats" => {
                text_repeats = args
                    .next()
                    .ok_or_else(|| anyhow!("--text-repeats requires a value"))?
                    .parse::<usize>()
                    .context("--text-repeats must be a positive integer")?;
            }
            "--px-range" => {
                px_range = args
                    .next()
                    .ok_or_else(|| anyhow!("--px-range requires a value"))?
                    .parse::<u32>()
                    .context("--px-range must be a positive integer")?;
            }
            "--case" => {
                case = CaseSelection::parse(
                    &args
                        .next()
                        .ok_or_else(|| anyhow!("--case requires a value"))?,
                )?;
            }
            "--list" => {
                println!(
                    "cases: all, layout, raster, gpu-msdf, emoji, raw-raster, raw-fdsm, raw-fdsm-base"
                );
                std::process::exit(0);
            }
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            _ => return Err(anyhow!("unknown argument `{arg}`")),
        }
    }
    Ok(Args {
        iterations,
        case,
        text_repeats,
        px_range,
    })
}

fn print_help() {
    println!(
        "usage: lumen-bench-text [--case all|layout|raster|gpu-msdf|emoji|raw-raster|raw-fdsm|raw-fdsm-base] [--iterations N] [--text-repeats N] [--px-range N]"
    );
}

fn run_raw_case(case: BenchCase, iterations: usize) -> anyhow::Result<BenchResult> {
    let mut glyph = RawGlyph::new()?;

    let cold_started = Instant::now();
    match case {
        BenchCase::RawRaster => {
            std::hint::black_box(glyph.raster_once());
        }
        BenchCase::RawFdsm => {
            std::hint::black_box(glyph.fdsm_once());
        }
        BenchCase::RawFdsmBase => {
            std::hint::black_box(glyph.fdsm_base_once());
        }
        _ => unreachable!("only raw cases are accepted here"),
    }
    let cold = cold_started.elapsed();

    let started = Instant::now();
    for _ in 0..iterations {
        match case {
            BenchCase::RawRaster => {
                std::hint::black_box(glyph.raster_once());
            }
            BenchCase::RawFdsm => {
                std::hint::black_box(glyph.fdsm_once());
            }
            BenchCase::RawFdsmBase => {
                std::hint::black_box(glyph.fdsm_base_once());
            }
            _ => unreachable!("only raw cases are accepted here"),
        }
    }

    Ok(BenchResult {
        cold,
        elapsed: started.elapsed(),
        glyph_count: 1,
    })
}

struct RawGlyph {
    font_system: FontSystem,
    key: CacheKey,
}

impl RawGlyph {
    fn new() -> anyhow::Result<Self> {
        let mut font_system = FontSystem::new_with_fonts(std::iter::empty());
        font_system.db_mut().load_font_data(ROBOTO_BYTES.to_vec());
        let mut buffer = Buffer::new(&mut font_system, Metrics::new(64.0, 76.8));
        buffer.set_text(
            &mut font_system,
            "A",
            &Attrs::new()
                .family(Family::Name("Roboto"))
                .weight(Weight(700))
                .cache_key_flags(CacheKeyFlags::DISABLE_HINTING),
            Shaping::Advanced,
            None,
        );
        buffer.shape_until_scroll(&mut font_system, true);
        let key = buffer
            .layout_runs()
            .flat_map(|run| run.glyphs.iter())
            .next()
            .map(|glyph| glyph.physical((0.0, 0.0), 1.0).cache_key)
            .ok_or_else(|| anyhow!("failed to shape raw glyph"))?;

        Ok(Self { font_system, key })
    }

    fn raster_once(&mut self) -> Option<(u32, u32, usize)> {
        let mut swash_cache = SwashCache::new();
        let image = swash_cache
            .get_image(&mut self.font_system, self.key)
            .as_ref()?;
        Some((
            image.placement.width,
            image.placement.height,
            image.data.len(),
        ))
    }

    fn fdsm_once(&self) -> Option<(u32, u32, usize)> {
        let image = generate_raw_fdsm_glyph(ROBOTO_BYTES, 0, self.key, 4, true)?;
        Some((image.width(), image.height(), image.into_raw().len()))
    }

    fn fdsm_base_once(&self) -> Option<(u32, u32, usize)> {
        let image = generate_raw_fdsm_glyph(ROBOTO_BYTES, 0, self.key, 4, false)?;
        Some((image.width(), image.height(), image.into_raw().len()))
    }
}

fn generate_raw_fdsm_glyph(
    font_data: &[u8],
    face_index: u32,
    key: CacheKey,
    px_range: u32,
    correct: bool,
) -> Option<Rgb32FImage> {
    let font = FontRef::from_index(font_data, face_index).ok()?;
    let axes = font
        .axes()
        .location(std::iter::empty::<skrifa::setting::VariationSetting>());
    let glyph_id = GlyphId::from(key.glyph_id);
    let bbox = font
        .glyph_metrics(Size::unscaled(), &axes)
        .bounds(glyph_id)?;
    if bbox.x_min >= bbox.x_max || bbox.y_min >= bbox.y_max {
        return None;
    }

    let font_size = f32::from_bits(key.font_size_bits).max(1.0) as f64;
    let units_per_em = f64::from(font.head().ok()?.units_per_em());
    let scale = font_size / units_per_em;
    let range = f64::from(px_range.max(1));
    let width = ((f64::from(bbox.x_max - bbox.x_min) * scale) + 2.0 * range).ceil() as u32;
    let height = ((f64::from(bbox.y_max - bbox.y_min) * scale) + 2.0 * range).ceil() as u32;
    if width == 0 || height == 0 {
        return None;
    }

    let (mut shape, _) = fdsm_skrifa::load_shape_from_face(&font, glyph_id, &axes).ok()?;
    shape.transform(&Affine2::from_matrix_unchecked(Matrix3::new(
        scale,
        0.0,
        range - f64::from(bbox.x_min) * scale,
        0.0,
        -scale,
        range + f64::from(bbox.y_max) * scale,
        0.0,
        0.0,
        1.0,
    )));
    let colored_shape = Shape::edge_coloring_simple(shape, 0.03, u64::from(key.glyph_id));
    let prepared_colored_shape = colored_shape.prepare();
    let mut image = Rgb32FImage::new(width, height);
    generate_msdf(&prepared_colored_shape, range, &mut image);
    if correct {
        correct_error_msdf(
            &mut image,
            &colored_shape,
            &prepared_colored_shape,
            range,
            &ErrorCorrectionConfig::default(),
        );
        correct_sign_msdf(&mut image, &prepared_colored_shape, FillRule::Nonzero);
    }
    Some(image)
}
