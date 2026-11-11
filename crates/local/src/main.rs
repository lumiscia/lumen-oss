use std::{
    env, fs,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use clap::{ArgGroup, Parser, ValueEnum};
use lumen_engine::{
    composition::Composition,
    diagnostic::{Diagnostic, Severity},
    gpu::{DEFAULT_MAX_TEXT_GLYPHS, MAX_TEXT_GLYPHS_CEILING},
    media::{AssetKind, MediaStore},
};
use lumen_ffmpeg::{
    EncodeCodec, EncoderQuality, QUALITY_CONFIG_ENV, QualityConfig, QualityRequest,
    VideoEncoderChoice, parse_bitrate,
};
use lumen_server::render::{
    CancelToken, FONT_DIR_ENV, FrameRange, GpuSettings, HARDWARE_DECODE_ENV, ImageRender,
    LocalMediaStore, Mp4Render, OutputFormat, RenderConfig, RenderOutput, RenderProgress,
    RenderReport, existing_font_dir, frame_number_width, is_enabled_flag, resolve_encoder,
};
use tracing_subscriber::EnvFilter;

/// Renders a Lumen composition to an .mp4 video, one frame to a .png or .jpg image, or a range of
/// frames to numbered images.
#[derive(Debug, Parser)]
#[command(
    name = "lumen-local",
    version,
    args_override_self = true,
    group(ArgGroup::new("rate_control").args(["bitrate", "crf", "qp"])),
    after_help = "\
Media sources in the composition resolve inside --media-root, which defaults to the current \
directory. Fonts come from --font-dir, which must exist, then <media-root>/assets/fonts, then the \
system, matched by the family name inside each font file. A family none of them has warns once \
and renders in the bundled font.

On .mp4 output, --quality names a preset: draft, standard (the default), high, lossless, or one \
--quality-config defines. --codec picks h264 (the default), hevc or av1. Both win over the \
composition's render_settings, which win over the config's defaults. Each codec tries its encoders \
in order, warning when one won't open: on Linux the Vulkan encoder, then libx264, libx265 or \
libsvtav1, and on macOS VideoToolbox, then the same CPU encoders. A preset with no settings for an encoder skips it, so \
lossless uses libx264 or libx265. --encoder or LUMEN_VIDEO_ENCODER pins one encoder, which either \
opens or fails the render.

--bitrate, --crf, --qp and --preset replace the preset with those settings on every encoder tried, \
and can't be combined with --quality. --preset alone, with no rate control, targets 14 Mb/s. Each fails on an encoder that \
doesn't support it (--crf: libx264, libx265, libsvtav1, NVENC; --qp: those and Vulkan; --preset: \
libx264 and libx265 ultrafast..placebo, NVENC p1..p7, libsvtav1 0..13), which moves on to the next.

--output picks what is written. .mp4 is a video of the frames, with the audio cut to the same span. \
.png and .jpg (or .jpeg) are one image, frame 0 unless --frame says another; a PNG keeps \
transparency and a JPEG is opaque. A file name with %d or %0Nd, such as frames/shot_%d.png, writes \
one image per frame instead, numbered with the composition's frame numbers and padded to at least \
four digits (%d) or N digits (%0Nd), wider if the last frame needs it. --frames START..END renders \
those frames of the composition, END not included; either side can be left out, and an mp4 or \
sequence starts at time 0.

--check reads the composition without rendering it and prints what is wrong with it, with fonts \
resolved as a render would resolve them. It exits 1 when there are errors; warnings alone exit 0.

LUMEN_HARDWARE_DECODE=1 decodes video on the GPU when the hardware supports it. RUST_LOG sets the \
log filter (default lumen_engine=warn,lumen_server=warn).

A text node draws at most --max-text-glyphs glyph, stroke and decoration instances (16384 by \
default, up to 1048576), which bounds the work its text can ask for; whitespace is not drawn."
)]
struct Cli {
    /// Composition JSON to render
    #[arg(long, value_name = "PATH")]
    composition: PathBuf,

    /// Where to write the render: .mp4 is a video, .png or .jpg one image, and a name with %d
    /// (frame_%04d.png) an image per frame
    #[arg(
        long,
        value_name = "PATH.[mp4|png|jpg]",
        required_unless_present = "check"
    )]
    output: Option<PathBuf>,

    /// Check the composition without rendering it, exiting 1 if it has errors
    #[arg(long, conflicts_with_all = ["output", "frame"])]
    check: bool,

    /// How --check prints what it finds
    #[arg(long, value_enum, default_value_t = Format::Text, requires = "check")]
    format: Format,

    /// Directory that media sources must stay inside [default: the current directory]
    #[arg(long, value_name = "PATH")]
    media_root: Option<PathBuf>,

    /// Directory of fonts to look in before the media root's assets/fonts and the system fonts
    #[arg(long, env = FONT_DIR_ENV, value_name = "PATH", value_parser = existing_font_dir)]
    font_dir: Option<PathBuf>,

    /// Video encoder name, defaulting to LUMEN_VIDEO_ENCODER. .mp4 only
    #[arg(long, value_name = "NAME")]
    encoder: Option<String>,

    /// Quality preset name. .mp4 only
    #[arg(long, value_name = "NAME", conflicts_with_all = ["bitrate", "crf", "qp", "preset"])]
    quality: Option<String>,

    /// Video codec: h264, hevc or av1. .mp4 only
    #[arg(long, value_name = "CODEC")]
    codec: Option<EncodeCodec>,

    /// JSON file of quality presets and encoder orders, checked before rendering
    #[arg(long, env = QUALITY_CONFIG_ENV, value_name = "PATH")]
    quality_config: Option<PathBuf>,

    /// Target bits per second, as 14000000, 14000k or 14M. .mp4 only
    #[arg(long, value_name = "BPS|Nk|NM", value_parser = parse_bitrate)]
    bitrate: Option<u64>,

    /// Constant rate factor. .mp4 only
    #[arg(long, value_name = "N")]
    crf: Option<u8>,

    /// Constant quantizer. .mp4 only
    #[arg(long, value_name = "N")]
    qp: Option<u8>,

    /// Encoder speed preset. .mp4 only
    #[arg(long, value_name = "NAME")]
    preset: Option<String>,

    /// Frame to render [default: 0]. One image only
    #[arg(long, value_name = "N", conflicts_with = "frames")]
    frame: Option<u32>,

    /// Frames to render as START..END, END not included, either side optional [default: all].
    /// A video or sequence starts at time 0
    #[arg(long, value_name = "START..END", value_parser = parse_frames)]
    frames: Option<FrameRange>,

    /// JPEG quality from 1 to 100 [default: 90]. .jpg only
    #[arg(
        long,
        value_name = "N",
        value_parser = clap::value_parser!(u8).range(1..=100)
    )]
    jpeg_quality: Option<u8>,

    /// Most glyph, stroke and decoration instances one text node may draw. A text over it fails
    /// the render
    #[arg(
        long,
        env = "LUMEN_MAX_TEXT_GLYPHS",
        value_name = "N",
        default_value_t = DEFAULT_MAX_TEXT_GLYPHS as u32,
        value_parser = clap::value_parser!(u32).range(1..=MAX_TEXT_GLYPHS_CEILING as i64)
    )]
    max_text_glyphs: u32,
}

/// What to write, and where each frame goes.
struct OutputPlan {
    output: RenderOutput,
    files: Files,
}

enum Files {
    /// The output path, written whole.
    One,
    /// A name per frame.
    Sequence(SequencePattern),
}

/// A file name with one `%d` or `%0Nd` that a sequence replaces with each frame's number.
#[derive(Debug, PartialEq, Eq)]
struct SequencePattern {
    before: String,
    /// The `N` of `%0Nd`. Without one, the numbers are as wide as [`frame_number_width`] says.
    width: Option<usize>,
    after: String,
}

impl SequencePattern {
    /// The pattern in `file_name`, or `None` for a name without a `%`.
    fn parse(file_name: &str) -> Result<Option<Self>> {
        let Some(at) = file_name.find('%') else {
            return Ok(None);
        };
        let (before, rest) = (&file_name[..at], &file_name[at + 1..]);
        let spec_len = rest.find('d').unwrap_or(rest.len());
        let (spec, after) = (
            &rest[..spec_len],
            rest.get(spec_len + 1..).unwrap_or_default(),
        );
        let width = match spec {
            "" => None,
            digits
                if digits.starts_with('0') && digits.bytes().all(|byte| byte.is_ascii_digit()) =>
            {
                match digits.parse::<usize>() {
                    Ok(width) if width <= MAX_PATTERN_WIDTH => Some(width),
                    _ => bail!(
                        "the frame number in `{file_name}` is padded to more than \
                         {MAX_PATTERN_WIDTH} digits"
                    ),
                }
            }
            _ => bail!(
                "unsupported frame number pattern in `{file_name}`; use %d or %0Nd, such as %05d"
            ),
        };
        if !rest.contains('d') || after.contains('%') {
            bail!("a sequence file name has one %d or %0Nd pattern (got `{file_name}`)");
        }
        Ok(Some(Self {
            before: before.to_string(),
            width,
            after: after.to_string(),
        }))
    }

    fn file_name(&self, frame: u32, last_frame: u32) -> String {
        let width = match self.width {
            Some(width) => width.max(last_frame.to_string().len()),
            None => frame_number_width(last_frame),
        };
        format!("{}{frame:0width$}{}", self.before, self.after)
    }
}

/// The widest a `%0Nd` pattern may pad a frame number.
const MAX_PATTERN_WIDTH: usize = 20;

fn parse_frames(raw: &str) -> Result<FrameRange, String> {
    let invalid = || format!("expected START..END, such as 30..60, 30.. or ..60, got `{raw}`");
    let (start, end) = raw.split_once("..").ok_or_else(invalid)?;
    let frame = |text: &str| match text {
        "" => Ok(None),
        text => text.parse::<u32>().map(Some).map_err(|_| invalid()),
    };
    Ok(FrameRange {
        start: frame(start)?,
        end: frame(end)?,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Format {
    /// One line per problem
    Text,
    /// `{ "ok", "diagnostics" }`, as `POST /renders/check` answers
    Json,
}

impl Cli {
    /// Where to write the render, which clap requires unless checking.
    fn output(&self) -> &Path {
        self.output
            .as_deref()
            .expect("clap requires --output to render")
    }

    /// An empty `LUMEN_FONT_DIR` counts as unset.
    fn font_dir(&self) -> Option<PathBuf> {
        self.font_dir
            .clone()
            .filter(|dir| !dir.as_os_str().is_empty())
    }

    /// The font directory, and `LUMEN_HARDWARE_DECODE` and the GPU settings from the environment.
    fn render_config(&self) -> RenderConfig {
        RenderConfig {
            font_dir: self.font_dir(),
            hardware_decode: env::var(HARDWARE_DECODE_ENV)
                .is_ok_and(|value| is_enabled_flag(&value)),
            gpu: GpuSettings::from_env(),
            temp_dir: Some(std::env::temp_dir()),
        }
    }

    /// The rate control and preset the command line asked for, or `None` if it asked for neither.
    fn quality(&self) -> Option<EncoderQuality> {
        EncoderQuality::from_options(self.bitrate, self.crf, self.qp, self.preset.clone())
            .expect("clap allows at most one rate control")
    }

    /// The encoders an .mp4 render of `composition` tries.
    fn encoder_choice(&self, composition: &Composition) -> Result<VideoEncoderChoice> {
        let config_path = self
            .quality_config
            .as_deref()
            .filter(|path| !path.as_os_str().is_empty());
        let config = QualityConfig::load(config_path).map_err(|error| anyhow!(error))?;
        let request = QualityRequest {
            preset: self.quality.clone(),
            codec: self.codec,
            settings: self.quality(),
            encoder: self.encoder.clone(),
        }
        .with_env_encoder();
        resolve_encoder(&config, composition, request).map_err(|error| anyhow!(error))
    }

    /// What the command line asks to write, after checking that no flag only another kind of
    /// output supports was given. `LUMEN_VIDEO_ENCODER` is a default, so only `--encoder`
    /// conflicts with an image.
    fn plan(&self) -> Result<OutputPlan> {
        let output = self.output();
        let extension = output
            .extension()
            .and_then(|value| value.to_str())
            .map(|value| value.to_ascii_lowercase())
            .unwrap_or_default();
        let file_name = output
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        let pattern = SequencePattern::parse(file_name)?;
        let format = match (extension.as_str(), pattern.is_some()) {
            ("mp4", false) => OutputFormat::Mp4,
            ("mp4", true) => bail!("a %d pattern writes images, so use .png or .jpg, not .mp4"),
            ("png", false) => OutputFormat::Png,
            ("png", true) => OutputFormat::PngSequence,
            ("jpg" | "jpeg", false) => OutputFormat::Jpeg,
            ("jpg" | "jpeg", true) => OutputFormat::JpegSequence,
            _ => bail!(
                "unsupported output extension; use .mp4, .png, .jpg or .jpeg (got `{}`)",
                output.display()
            ),
        };
        if !format.is_video()
            && (self.encoder.is_some()
                || self.quality.is_some()
                || self.codec.is_some()
                || self.quality().is_some())
        {
            bail!(
                "--encoder, --quality, --codec, --bitrate, --crf, --qp and --preset are only supported when output is .mp4"
            );
        }
        if format.is_video() && self.frame.is_some() {
            bail!("--frame is only supported when output is an image; use --frames for a video");
        }
        if format.is_sequence() && self.frame.is_some() {
            bail!("--frame renders one image; use --frames START..END for a sequence");
        }
        if self.jpeg_quality.is_some() && !format.is_jpeg() {
            bail!("--jpeg-quality is only supported when output is .jpg");
        }
        if !format.is_video()
            && !format.is_sequence()
            && let Some(FrameRange {
                start: Some(start),
                end: Some(end),
            }) = self.frames
            && end.saturating_sub(start) > 1
        {
            bail!(
                "{} renders one frame, but --frames {start}..{end} has {}; name the output like frames/shot_%d.{} for a sequence",
                output.display(),
                end - start,
                extension
            );
        }
        let frames = match (self.frames, self.frame) {
            (Some(frames), _) => frames,
            (None, Some(frame)) => FrameRange {
                start: Some(frame),
                end: None,
            },
            (None, None) => FrameRange::default(),
        };
        Ok(OutputPlan {
            output: RenderOutput {
                format,
                frames,
                jpeg_quality: self.jpeg_quality,
            },
            files: pattern.map_or(Files::One, Files::Sequence),
        })
    }
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("lumen_engine=warn,lumen_server=warn")),
        )
        .with_writer(std::io::stderr)
        .init();
    lumen_ffmpeg::raise_open_file_limit();
    let cli = Cli::parse();
    let result = if cli.check {
        check(&cli)
    } else {
        run(&cli).map(|()| true)
    };
    match result {
        Ok(true) => {}
        Ok(false) => std::process::exit(1),
        Err(error) => {
            eprintln!("{}", failure_line(&error));
            std::process::exit(1);
        }
    }
}

/// The line a failed run prints: the engine's diagnostic code, when the engine failed, and the
/// whole chain of causes.
fn failure_line(error: &anyhow::Error) -> String {
    let engine = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<lumen_engine::error::LumenError>());
    match engine {
        Some(engine) => format!("lumen-local failed ({}): {error:#}", engine.code()),
        None => format!("lumen-local failed: {error:#}"),
    }
}

/// Prints what is wrong with the composition, and whether it renders.
fn check(cli: &Cli) -> Result<bool> {
    let raw = fs::read_to_string(&cli.composition).with_context(|| {
        format!(
            "failed to read composition file {}",
            cli.composition.display()
        )
    })?;
    let media_store =
        LocalMediaStore::new(media_root(cli.media_root.as_deref())?, &cli.render_config());
    let known_font = |family: &str| media_store.resolve(family, AssetKind::Font).is_ok();
    let diagnostics =
        lumen_engine::json::check(&raw, Some(&known_font), Some(media_store.assets()));
    let ok = !diagnostics.iter().any(Diagnostic::is_error);
    match cli.format {
        Format::Json => println!(
            "{}",
            serde_json::json!({ "ok": ok, "diagnostics": diagnostics })
        ),
        Format::Text => {
            for diagnostic in &diagnostics {
                println!("{}", diagnostic_line(diagnostic));
            }
            let errors = diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.is_error())
                .count();
            println!(
                "{}: {errors} errors, {} warnings",
                cli.composition.display(),
                diagnostics.len() - errors
            );
        }
    }
    Ok(ok)
}

fn diagnostic_line(diagnostic: &Diagnostic) -> String {
    let severity = match diagnostic.severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
    };
    let path = if diagnostic.path.is_empty() {
        "/"
    } else {
        &diagnostic.path
    };
    format!(
        "{severity} {} at {path}: {}",
        diagnostic.code, diagnostic.message
    )
}

fn print_warnings(diagnostics: &[Diagnostic]) {
    for diagnostic in diagnostics {
        eprintln!("{}", diagnostic_line(diagnostic));
    }
}

fn run(cli: &Cli) -> Result<()> {
    let plan = cli.plan()?;
    let composition = load_composition(&cli.composition)?;
    let media_root = media_root(cli.media_root.as_deref())?;
    let media_store =
        LocalMediaStore::new(media_root, &cli.render_config()).explaining_missing_sources();

    if plan.output.format.is_video() {
        let choice = cli.encoder_choice(&composition)?;
        return render_to_partial(cli.output(), |partial| {
            render_mp4(
                &composition,
                media_store,
                partial,
                choice,
                &plan.output,
                cli.max_text_glyphs,
            )
        });
    }
    render_images(
        &composition,
        media_store,
        cli.output(),
        &plan,
        cli.max_text_glyphs,
    )
}

/// Renders to a new file beside `output` and moves it into place only on success, so a failed render
/// keeps any earlier output. A process killed mid-render leaves the temporary file behind.
fn render_to_partial(output: &Path, render: impl FnOnce(&Path) -> Result<()>) -> Result<()> {
    let destination = resolve_output_destination(output)?;
    let directory = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(directory)
        .with_context(|| format!("failed to create output dir {}", directory.display()))?;

    let existing = fs::metadata(&destination).ok();
    let mut partial = tempfile::Builder::new();
    partial.prefix(".lumen-local-").suffix(".partial.mp4");
    // Without this the rename would leave a new output readable only by its owner, and a render
    // replacing a private output would be readable by others while it runs.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = existing
            .as_ref()
            .map_or(0o666, |existing| existing.permissions().mode() & 0o777);
        partial.permissions(fs::Permissions::from_mode(mode));
    }
    let partial = partial
        .tempfile_in(directory)
        .with_context(|| format!("failed to create a file in {}", directory.display()))?
        .into_temp_path();

    render(&partial)?;

    if let Some(existing) = existing {
        fs::set_permissions(&partial, existing.permissions())
            .with_context(|| format!("failed to set permissions for {}", output.display()))?;
    }
    partial
        .persist(&destination)
        .with_context(|| format!("failed to write output {}", output.display()))?;
    println!("output written to {}", output.display());
    Ok(())
}

/// The file a render to `output` replaces. Renaming onto a symlink would replace the link, so it
/// resolves to the link's target, and anything other than a regular file is refused.
fn resolve_output_destination(output: &Path) -> Result<PathBuf> {
    if fs::symlink_metadata(output).is_err() {
        return Ok(output.to_path_buf());
    }
    let destination = output
        .canonicalize()
        .with_context(|| format!("failed to resolve output path {}", output.display()))?;
    let metadata = fs::metadata(&destination)
        .with_context(|| format!("failed to inspect output path {}", output.display()))?;
    if !metadata.is_file() {
        return Err(anyhow!(
            "output path {} already exists and is not a regular file",
            output.display()
        ));
    }
    Ok(destination)
}

fn load_composition(path: &Path) -> Result<Composition> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read composition file {}", path.display()))?;

    lumen_engine::json::parse(&raw)
        .with_context(|| format!("failed to parse composition {}", path.display()))
}

/// Writes each frame of the render as a file: the output path for a still, and for a sequence the
/// output's name with the frame's number in the pattern.
fn render_images(
    composition: &Composition,
    media_store: LocalMediaStore,
    output: &Path,
    plan: &OutputPlan,
    max_text_glyphs: u32,
) -> Result<()> {
    let mut render = ImageRender::open(composition, media_store, &plan.output, CancelToken::new())?;
    render.set_max_text_glyphs(max_text_glyphs as usize);
    let last_frame = render.frames().end - 1;
    let directory = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    if let Some(directory) = directory {
        fs::create_dir_all(directory)
            .with_context(|| format!("failed to create output dir {}", directory.display()))?;
    }
    let report = render.run(
        &mut |image| {
            let path = match &plan.files {
                Files::One => output.to_path_buf(),
                Files::Sequence(pattern) => {
                    output.with_file_name(pattern.file_name(image.frame, last_frame))
                }
            };
            fs::write(&path, &image.bytes)
                .with_context(|| format!("failed to write {}", path.display()))
        },
        &mut print_progress,
    )?;
    print_warnings(&report.diagnostics);
    match plan.files {
        Files::One => println!(
            "render complete output={} frame={}",
            output.display(),
            last_frame
        ),
        Files::Sequence(_) => println!(
            "render complete output={} frames={}",
            output.display(),
            report.frames
        ),
    }
    Ok(())
}

fn print_progress(progress: RenderProgress) {
    if progress.frame == 1
        || progress.frame == progress.total_frames
        || (progress.frame - 1).is_multiple_of(60)
    {
        println!(
            "progress frame={}/{}",
            progress.frame, progress.total_frames
        );
    }
}

fn render_mp4(
    composition: &Composition,
    media_store: LocalMediaStore,
    output: &Path,
    choice: VideoEncoderChoice,
    render_output: &RenderOutput,
    max_text_glyphs: u32,
) -> Result<()> {
    let mut render = Mp4Render::open(
        composition,
        media_store,
        choice,
        output,
        render_output,
        CancelToken::new(),
    )?;
    render.set_max_text_glyphs(max_text_glyphs as usize);
    println!(
        "video_encoder name={} path={}",
        render.encoder(),
        render.path()
    );
    let report = render.run(&mut print_progress)?;
    print_warnings(&report.diagnostics);
    println!("{}", render_summary(&report));
    Ok(())
}

/// The summary line, with passes per frame: those skipped because their output was kept from an
/// earlier frame, and those recorded.
fn render_summary(report: &RenderReport) -> String {
    let millis = |duration: Duration| duration.as_millis();
    let timing = &report.timing;
    let frames = f64::from(report.frames.max(1));
    format!(
        "render complete frames={} audio_mix_ms={} bind_ms={} upload_ms={} render_ms={} flush_ms={} readback_ms={} encode_send_ms={} encode_finish_ms={} total_ms={} held_bytes={} skipped_passes={:.1} submitted_passes={:.1}",
        report.frames,
        millis(timing.audio_mix),
        millis(timing.bind),
        millis(timing.upload),
        millis(timing.render),
        millis(timing.flush),
        millis(timing.readback),
        millis(timing.encode_send),
        millis(timing.encode_finish),
        millis(timing.total),
        report.held_bytes,
        report.skipped_passes as f64 / frames,
        report.submitted_passes as f64 / frames,
    )
}

fn media_root(override_root: Option<&Path>) -> Result<PathBuf> {
    let root = match override_root {
        Some(path) => path.to_path_buf(),
        None => env::current_dir().context("failed to read current directory")?,
    };
    root.canonicalize()
        .with_context(|| format!("failed to canonicalize media root {}", root.display()))
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::os::unix::fs::FileTypeExt;
    use std::{fs, path::Path};

    use clap::{CommandFactory, Parser};

    use lumen_ffmpeg::RateControl;

    use lumen_server::render::{FrameRange, OutputFormat, RenderOutput};

    use super::{
        Cli, EncoderQuality, SequencePattern, failure_line, parse_frames, render_to_partial,
    };

    #[test]
    fn a_failed_run_prints_the_engine_code_of_the_failure() {
        use lumen_engine::{
            error::{LumenError, MediaFailure},
            media::AssetKind,
        };

        let media: LumenError = MediaFailure::not_found("logo.png", "no such file")
            .wanting(&[AssetKind::Image])
            .into();
        let error = anyhow::Error::new(media).context("failed to render");
        assert_eq!(
            failure_line(&error),
            "lumen-local failed (media.not_found): failed to render: image `logo.png` not found: no such file"
        );
        assert_eq!(
            failure_line(&anyhow::anyhow!("disk full")),
            "lumen-local failed: disk full"
        );
    }

    #[test]
    fn failed_renders_leave_no_partial_file_and_keep_earlier_output() {
        let dir = tempfile::tempdir().expect("temp dir");
        let output = dir.path().join("out.mp4");
        let file_names = || {
            let mut names = fs::read_dir(dir.path())
                .expect("read dir")
                .map(|entry| entry.expect("entry").file_name())
                .collect::<Vec<_>>();
            names.sort();
            names
        };

        let failed = render_to_partial(&output, |partial| {
            fs::write(partial, b"partial")?;
            Err(anyhow::anyhow!("render failed"))
        });
        assert!(failed.is_err());
        assert!(file_names().is_empty(), "{:?}", file_names());

        render_to_partial(&output, |partial| Ok(fs::write(partial, b"first")?)).expect("render");
        assert_eq!(fs::read(&output).expect("output"), b"first");

        let failed = render_to_partial(&output, |partial| {
            fs::write(partial, b"partial")?;
            Err(anyhow::anyhow!("render failed"))
        });
        assert!(failed.is_err());
        assert_eq!(fs::read(&output).expect("output"), b"first");
        assert_eq!(file_names().len(), 1, "{:?}", file_names());
    }

    #[cfg(unix)]
    #[test]
    fn renders_leave_files_named_like_the_temporary_file_alone() {
        let dir = tempfile::tempdir().expect("temp dir");
        let output = dir.path().join("out.mp4");
        let bystander = dir.path().join(".out.partial.mp4");
        let fail = |partial: &Path| -> anyhow::Result<()> {
            fs::write(partial, b"partial")?;
            Err(anyhow::anyhow!("render failed"))
        };

        fs::write(&bystander, b"mine").expect("write bystander");
        assert!(render_to_partial(&output, fail).is_err());
        assert_eq!(fs::read(&bystander).expect("bystander"), b"mine");
        fs::remove_file(&bystander).expect("remove bystander");

        fs::write(&output, b"first").expect("write output");
        std::os::unix::fs::symlink(&output, &bystander).expect("link bystander");
        assert!(render_to_partial(&output, fail).is_err());
        assert_eq!(fs::read(&output).expect("output"), b"first");
    }

    #[test]
    fn concurrent_renders_reserve_their_own_temporary_files() {
        let dir = tempfile::tempdir().expect("temp dir");
        let output = dir.path().join("out.mp4");
        render_to_partial(&output, |outer| {
            assert!(outer.is_file(), "the path is reserved before rendering");
            assert_eq!(outer.extension().expect("extension"), "mp4");
            render_to_partial(&output, |inner| {
                assert_ne!(outer, inner);
                Ok(fs::write(inner, b"inner")?)
            })?;
            Ok(fs::write(outer, b"outer")?)
        })
        .expect("render");
        assert_eq!(fs::read(&output).expect("output"), b"outer");
        assert_eq!(fs::read_dir(dir.path()).expect("read dir").count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn non_regular_outputs_are_refused_before_rendering() {
        let dir = tempfile::tempdir().expect("temp dir");
        let fifo = dir.path().join("fifo.mp4");
        let status = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("run mkfifo");
        assert!(status.success());
        let dangling = dir.path().join("dangling.mp4");
        std::os::unix::fs::symlink(dir.path().join("missing.mp4"), &dangling).expect("link");

        for output in [fifo.as_path(), dangling.as_path(), dir.path()] {
            render_to_partial(output, |_| panic!("must not render {output:?}"))
                .expect_err("must refuse");
        }
        let error = render_to_partial(&fifo, |_| Ok(())).expect_err("must refuse");
        assert!(error.to_string().contains("not a regular file"), "{error}");
        assert!(
            fs::symlink_metadata(&fifo)
                .expect("fifo")
                .file_type()
                .is_fifo()
        );
    }

    #[cfg(unix)]
    #[test]
    fn replacing_an_output_keeps_its_mode_and_symlink() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("temp dir");
        let mode = |path: &Path| fs::metadata(path).expect("metadata").permissions().mode();
        let render = |partial: &Path| -> anyhow::Result<()> { Ok(fs::write(partial, b"new")?) };

        let reference = dir.path().join("reference");
        fs::write(&reference, b"").expect("write reference");
        let fresh = dir.path().join("fresh.mp4");
        render_to_partial(&fresh, render).expect("render");
        assert_eq!(mode(&fresh), mode(&reference));

        let restricted = dir.path().join("restricted.mp4");
        fs::write(&restricted, b"old").expect("write output");
        fs::set_permissions(&restricted, fs::Permissions::from_mode(0o640)).expect("chmod");
        render_to_partial(&restricted, render).expect("render");
        assert_eq!(mode(&restricted) & 0o777, 0o640);

        let private = dir.path().join("private.mp4");
        fs::write(&private, b"old").expect("write output");
        fs::set_permissions(&private, fs::Permissions::from_mode(0o600)).expect("chmod");
        render_to_partial(&private, |partial| {
            assert_eq!(mode(partial) & 0o777, 0o600);
            render(partial)
        })
        .expect("render");

        let target = dir.path().join("target.mp4");
        let link = dir.path().join("link.mp4");
        fs::write(&target, b"old").expect("write target");
        std::os::unix::fs::symlink(&target, &link).expect("link");
        render_to_partial(&link, render).expect("render");
        assert!(
            fs::symlink_metadata(&link)
                .expect("link")
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read(&target).expect("target"), b"new");
    }

    fn parse(flags: &[&str]) -> Result<Cli, clap::Error> {
        let base = ["lumen-local", "--composition", "in.json"];
        Cli::try_parse_from(base.into_iter().chain(flags.iter().copied()))
    }

    #[test]
    fn cli_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn text_glyph_limit_defaults_and_stays_within_the_ceiling() {
        let limit = |flags: &[&str]| {
            parse(&[&["--output", "o.png"], flags].concat()).map(|cli| cli.max_text_glyphs)
        };
        assert_eq!(limit(&[]).unwrap(), 16_384);
        assert_eq!(limit(&["--max-text-glyphs", "65536"]).unwrap(), 65_536);
        for invalid in ["0", "1048577", "many"] {
            assert!(limit(&["--max-text-glyphs", invalid]).is_err(), "{invalid}");
        }
    }

    #[test]
    fn rate_control_flags_conflict_and_validate() {
        let quality = |flags: &[&str]| parse(flags).unwrap().quality().unwrap();
        let output = ["--output", "o.mp4"];
        assert_eq!(
            quality(&[&output[..], &["--crf", "23", "--preset", "fast"]].concat()),
            EncoderQuality {
                rate_control: Some(RateControl::Crf(23)),
                preset: Some("fast".into()),
                options: Vec::new(),
            }
        );
        assert_eq!(
            quality(&[&output[..], &["--bitrate", "8M"]].concat()).rate_control,
            Some(RateControl::Bitrate(8_000_000))
        );
        assert_eq!(
            quality(&[&output[..], &["--qp", "30"]].concat()).rate_control,
            Some(RateControl::Qp(30))
        );

        for pair in [
            ["--crf", "--qp"],
            ["--bitrate", "--crf"],
            ["--bitrate", "--qp"],
        ] {
            let value = |flag: &str| if flag == "--bitrate" { "1M" } else { "20" };
            let error = parse(&[
                "--output",
                "o.mp4",
                pair[0],
                value(pair[0]),
                pair[1],
                value(pair[1]),
            ])
            .unwrap_err();
            assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
        }
        for flag in [
            ["--crf", "20"],
            ["--qp", "20"],
            ["--bitrate", "1M"],
            ["--preset", "fast"],
        ] {
            let error =
                parse(&["--output", "o.mp4", "--quality", "high", flag[0], flag[1]]).unwrap_err();
            assert_eq!(
                error.kind(),
                clap::error::ErrorKind::ArgumentConflict,
                "{flag:?}"
            );
        }
        assert!(parse(&["--output", "o.mp4", "--codec", "vp9"]).is_err());
        assert!(parse(&["--output", "o.mp4", "--crf", "300"]).is_err());
        assert!(parse(&["--output", "o.mp4", "--bitrate", "0"]).is_err());
    }

    #[test]
    fn plan_rejects_flags_the_output_cannot_use() {
        let plan = |flags: &[&str]| parse(flags).unwrap().plan();
        let message = |flags: &[&str]| plan(flags).err().unwrap().to_string();
        assert!(plan(&["--output", "o.mp4", "--crf", "23"]).is_ok());
        assert!(plan(&["--output", "o.png", "--frame", "3"]).is_ok());

        assert!(
            message(&["--output", "o.png", "--encoder", "libx264"])
                .contains("only supported when output is .mp4")
        );
        for quality in [
            ["--bitrate", "14M"],
            ["--crf", "23"],
            ["--preset", "fast"],
            ["--quality", "high"],
            ["--codec", "hevc"],
        ] {
            for output in ["o.png", "o.jpg", "o_%d.png"] {
                assert!(
                    message(&["--output", output, quality[0], quality[1]])
                        .contains("only supported when output is .mp4"),
                    "{output} {quality:?}"
                );
            }
        }
        assert!(
            message(&["--output", "o.mp4", "--frame", "3"])
                .contains("--frame is only supported when output is an image")
        );
        assert!(message(&["--output", "o_%d.png", "--frame", "3"]).contains("use --frames"));
        assert!(
            message(&["--output", "o.png", "--jpeg-quality", "80"])
                .contains("only supported when output is .jpg")
        );
        assert!(message(&["--output", "o.gif"]).contains("unsupported output extension"));
        assert!(message(&["--output", "o_%d.mp4"]).contains("use .png or .jpg"));
        assert_eq!(
            parse(&["--output", "o.png", "--frame", "3", "--frames", "1..2"])
                .unwrap_err()
                .kind(),
            clap::error::ErrorKind::ArgumentConflict
        );
        assert!(parse(&["--output", "o.jpg", "--jpeg-quality", "0"]).is_err());
        assert!(parse(&["--output", "o.jpg", "--jpeg-quality", "101"]).is_err());
    }

    #[test]
    fn the_output_names_the_format_and_the_frames() {
        let output = |flags: &[&str]| parse(flags).unwrap().plan().unwrap().output;
        let still = output(&["--output", "o.png"]);
        assert_eq!(still, RenderOutput::of(OutputFormat::Png));
        assert_eq!(still.resolve(10).unwrap(), 0..1);
        assert_eq!(
            output(&["--output", "o.PNG", "--frame", "3"])
                .resolve(10)
                .unwrap(),
            3..4
        );
        let jpeg = output(&["--output", "o.jpeg", "--jpeg-quality", "70"]);
        assert_eq!(
            (jpeg.format, jpeg.jpeg_quality),
            (OutputFormat::Jpeg, Some(70))
        );
        assert_eq!(
            output(&["--output", "frames/f_%d.jpg", "--frames", "2.."]).format,
            OutputFormat::JpegSequence
        );
        let video = output(&["--output", "o.mp4", "--frames", "..4"]);
        assert_eq!(video.format, OutputFormat::Mp4);
        assert_eq!(video.resolve(10).unwrap(), 0..4);
        assert_eq!(output(&["--output", "o.mp4"]).resolve(10).unwrap(), 0..10);
    }

    #[test]
    fn frame_ranges_parse_with_either_side_open() {
        let range = |raw: &str| parse_frames(raw);
        assert_eq!(
            range("30..60"),
            Ok(FrameRange {
                start: Some(30),
                end: Some(60)
            })
        );
        assert_eq!(
            range("30.."),
            Ok(FrameRange {
                start: Some(30),
                end: None
            })
        );
        assert_eq!(
            range("..60"),
            Ok(FrameRange {
                start: None,
                end: Some(60)
            })
        );
        assert_eq!(range(".."), Ok(FrameRange::default()));
        for invalid in ["30", "a..b", "-1..4", "1...4", "1..2..3", ""] {
            assert!(range(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn sequence_patterns_number_frames_and_pad_to_the_range() {
        let name = |pattern: &str, frame: u32, last: u32| {
            SequencePattern::parse(pattern)
                .unwrap()
                .unwrap()
                .file_name(frame, last)
        };
        assert_eq!(name("shot_%d.png", 7, 59), "shot_0007.png");
        assert_eq!(name("shot_%d.png", 7, 12_000), "shot_00007.png");
        assert_eq!(name("%06d.jpg", 30, 59), "000030.jpg");
        assert_eq!(name("%02d.jpg", 30, 100_000), "000030.jpg");
        assert_eq!(name("a_%d_b.png", 3, 5), "a_0003_b.png");
        assert_eq!(SequencePattern::parse("still.png").unwrap(), None);
        for invalid in [
            "a_%s.png",
            "a_%.png",
            "a_%5d.png",
            "a_%d_%d.png",
            "a_%d%%.png",
            "a_%021d.png",
            "a_%099999999999999999999999d.png",
        ] {
            assert!(SequencePattern::parse(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn quality_config_presets_resolve_against_the_composition() {
        let dir = tempfile::tempdir().expect("temp dir");
        let config = dir.path().join("quality.json");
        fs::write(
            &config,
            r#"{ "presets": { "archive": { "encoders": { "libx264": { "crf": 16 } } } } }"#,
        )
        .expect("write config");
        let composition = lumen_engine::json::parse_value(&serde_json::json!({
            "timeline": { "duration_frames": 1 },
            "render_settings": { "width": 16, "height": 16, "quality": "archive" },
            "nodes": [
                { "id": 1, "type": "background", "params": { "paint": [0, 0, 0, 255], "width": 16, "height": 16 } },
                { "id": 2, "type": "media_output" }
            ],
            "connections": [{ "from_node": 1, "from_port": "output", "to_node": 2, "to_port": "source" }]
        }))
        .expect("composition");
        let choice = |flags: &[&str]| {
            let config = config.to_str().unwrap();
            let base = ["--output", "o.mp4", "--quality-config", config];
            parse(&[&base[..], flags].concat())
                .unwrap()
                .encoder_choice(&composition)
        };

        let archive = choice(&[]).expect("archive resolves");
        assert_eq!(archive.candidates().len(), 1);
        assert_eq!(
            archive.first().quality.rate_control,
            Some(RateControl::Crf(16))
        );
        let error = choice(&["--quality", "cinema"]).unwrap_err();
        assert!(error.to_string().contains("`cinema`"), "{error}");
    }

    /// Runs `parsed_font_dir` in a child process, since clap reads `LUMEN_FONT_DIR` from the real
    /// environment and a test must not change that for the others.
    fn font_dir_with(env_dir: Option<&str>, flags: &[&str]) -> String {
        let mut child = std::process::Command::new(std::env::current_exe().expect("test binary"));
        child
            .args(["--exact", "tests::parsed_font_dir", "--nocapture"])
            .env("LUMEN_TEST_FONT_FLAGS", flags.join(" "))
            .env_remove("LUMEN_FONT_DIR");
        if let Some(env_dir) = env_dir {
            child.env("LUMEN_FONT_DIR", env_dir);
        }
        let output = child.output().expect("run test binary");
        assert!(output.status.success());
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .find_map(|line| line.strip_prefix("font_dir=").map(str::to_string))
            .expect("child output")
    }

    #[test]
    fn parsed_font_dir() {
        let Ok(flags) = std::env::var("LUMEN_TEST_FONT_FLAGS") else {
            return;
        };
        let cli = parse(&flags.split_whitespace().collect::<Vec<_>>()).unwrap();
        println!("font_dir={:?}", cli.font_dir());
    }

    #[test]
    fn cli_font_directory_takes_precedence_over_environment() {
        let (cli_dir, environment_dir) =
            (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (cli_dir, environment_dir) = (
            cli_dir.path().to_str().unwrap(),
            environment_dir.path().to_str().unwrap(),
        );
        let output = ["--output", "o.png"];
        let with = |env_dir, flags: &[&str]| font_dir_with(env_dir, &[&output[..], flags].concat());
        assert_eq!(
            with(Some(environment_dir), &["--font-dir", cli_dir]),
            format!("Some({cli_dir:?})")
        );
        assert_eq!(
            with(Some(environment_dir), &[]),
            format!("Some({environment_dir:?})")
        );
        assert_eq!(with(Some(""), &[]), "None");
        assert_eq!(with(None, &[]), "None");
    }

    #[test]
    fn a_font_directory_that_does_not_exist_fails_the_parse() {
        let error = parse(&["--output", "o.png", "--font-dir", "no-such-fonts"])
            .err()
            .unwrap();
        assert!(
            error
                .to_string()
                .contains("`no-such-fonts` is not a directory")
        );
    }
}
