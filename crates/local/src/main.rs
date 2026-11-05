use std::{
    env, fs,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use clap::{ArgGroup, Parser};
use image::{ImageEncoder, codecs::png::PngEncoder};
use lumen_engine::composition::Composition;
use lumen_ffmpeg::{EncoderQuality, RateControl};
use lumen_server::render::{
    FONT_DIR_ENV, LocalMediaStore, Mp4Render, RenderReport, create_gpu_renderer, existing_font_dir,
};
use tracing_subscriber::EnvFilter;

/// Renders a Lumen composition to an .mp4 video, or one frame to a .png image.
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

On .mp4 output, --encoder or LUMEN_VIDEO_ENCODER names the encoder, and a named encoder that \
won't open fails the render. With neither set, Linux uses h264_vulkan and falls back to libx264 \
with a warning when it won't open, which includes --crf and --preset, as h264_vulkan supports \
neither. macOS uses h264_videotoolbox. The default is a 14 Mb/s target. --bitrate, --crf, --qp \
and --preset override it where the encoder supports them (--crf: libx264, NVENC; --qp: libx264, \
NVENC, Vulkan; --preset: libx264 ultrafast..placebo, NVENC p1..p7) and fail otherwise.

LUMEN_HARDWARE_DECODE=1 decodes video on the GPU when the hardware supports it. RUST_LOG sets the \
log filter (default lumen_engine=warn,lumen_server=warn)."
)]
struct Cli {
    /// Composition JSON to render
    #[arg(long, value_name = "PATH")]
    composition: PathBuf,

    /// Where to write the render: .mp4 renders every frame, .png renders one
    #[arg(long, value_name = "PATH.[png|mp4]")]
    output: PathBuf,

    /// Directory that media sources must stay inside [default: the current directory]
    #[arg(long, value_name = "PATH")]
    media_root: Option<PathBuf>,

    /// Directory of fonts to look in before the media root's assets/fonts and the system fonts
    #[arg(long, env = FONT_DIR_ENV, value_name = "PATH", value_parser = existing_font_dir)]
    font_dir: Option<PathBuf>,

    /// Video encoder name, defaulting to LUMEN_VIDEO_ENCODER. .mp4 only
    #[arg(long, value_name = "NAME")]
    encoder: Option<String>,

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

    /// Frame to render [default: 0]. .png only
    #[arg(long, value_name = "N")]
    frame: Option<u32>,
}

enum OutputKind {
    Png,
    Mp4,
}

impl Cli {
    /// An empty `LUMEN_FONT_DIR` counts as unset.
    fn font_dir(&self) -> Option<PathBuf> {
        self.font_dir
            .clone()
            .filter(|dir| !dir.as_os_str().is_empty())
    }

    /// The rate control and preset the command line asked for, or `None` if it asked for neither.
    fn quality(&self) -> Option<EncoderQuality> {
        let rate_control = [
            self.bitrate.map(RateControl::Bitrate),
            self.crf.map(RateControl::Crf),
            self.qp.map(RateControl::Qp),
        ]
        .into_iter()
        .flatten()
        .next();
        (rate_control.is_some() || self.preset.is_some()).then(|| EncoderQuality {
            rate_control: rate_control.unwrap_or_default(),
            preset: self.preset.clone(),
        })
    }

    /// The kind of output asked for, after checking that no flag only the other kind supports was
    /// given. `LUMEN_VIDEO_ENCODER` is a default, so only `--encoder` conflicts with .png.
    fn output_kind(&self) -> Result<OutputKind> {
        let extension = self
            .output
            .extension()
            .and_then(|value| value.to_str())
            .map(|value| value.to_ascii_lowercase())
            .unwrap_or_default();
        match extension.as_str() {
            "png" => {
                if self.encoder.is_some() || self.quality().is_some() {
                    bail!(
                        "--encoder, --bitrate, --crf, --qp and --preset are only supported when output is .mp4"
                    );
                }
                Ok(OutputKind::Png)
            }
            "mp4" => {
                if self.frame.is_some() {
                    bail!("--frame is only supported when output is .png");
                }
                Ok(OutputKind::Mp4)
            }
            _ => bail!(
                "unsupported output extension; use .png or .mp4 (got `{}`)",
                self.output.display()
            ),
        }
    }
}

/// Parses bits per second, with optional `k` or `M` suffixes (`14M` is 14,000,000).
fn parse_bitrate(value: &str) -> Result<u64> {
    let (digits, multiplier) = match value.strip_suffix(['k', 'K']) {
        Some(digits) => (digits, 1_000),
        None => match value.strip_suffix(['m', 'M']) {
            Some(digits) => (digits, 1_000_000),
            None => (value, 1),
        },
    };
    let bitrate = digits
        .parse::<u64>()?
        .checked_mul(multiplier)
        .ok_or_else(|| anyhow!("bitrate is too large"))?;
    if bitrate == 0 {
        return Err(anyhow!("bitrate must be greater than zero"));
    }
    Ok(bitrate)
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
    if let Err(error) = run(Cli::parse()) {
        eprintln!("lumen-local failed: {error:#}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    let kind = cli.output_kind()?;
    let composition = load_composition(&cli.composition)?;
    let media_root = media_root(cli.media_root.as_deref())?;
    let font_dir = cli.font_dir();
    let mut media_store =
        LocalMediaStore::new(media_root, font_dir.as_deref()).explaining_missing_sources();

    match kind {
        OutputKind::Png => {
            render_single_png(&composition, &mut media_store, &cli.output, cli.frame)
        }
        OutputKind::Mp4 => {
            let quality = cli.quality().unwrap_or_default();
            render_to_partial(&cli.output, |partial| {
                render_mp4(
                    &composition,
                    media_store,
                    partial,
                    cli.encoder.as_deref(),
                    &quality,
                )
            })
        }
    }
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

fn render_single_png(
    composition: &Composition,
    media_store: &mut LocalMediaStore,
    output: &Path,
    frame_override: Option<u32>,
) -> Result<()> {
    let frame = frame_override.unwrap_or(0);
    if frame >= composition.timeline().duration_frames {
        return Err(anyhow!(
            "requested frame {frame} is out of range for duration {}",
            composition.timeline().duration_frames
        ));
    }

    let mut renderer = create_gpu_renderer(media_store).context("failed to create GPU renderer")?;
    renderer
        .compile(composition)
        .context("failed to compile composition")?;
    let rendered = renderer
        .render_frame(composition, frame, &*media_store)
        .with_context(|| format!("render failed at frame {frame}"))?;
    let size = rendered.domain.storage_size;
    let mut pixels = renderer
        .gpu_renderer()
        .read_texture_rgba8(&rendered.texture, size)
        .with_context(|| format!("failed reading rendered pixels for frame {frame}"))?;
    unpremultiply_rgba8(&mut pixels);

    write_png(output, size.width, size.height, pixels.as_slice())?;

    println!("render complete output={} frame={frame}", output.display());
    Ok(())
}

fn render_mp4(
    composition: &Composition,
    media_store: LocalMediaStore,
    output: &Path,
    encoder: Option<&str>,
    quality: &EncoderQuality,
) -> Result<()> {
    let render = Mp4Render::open(composition, media_store, encoder, quality, output)?;
    println!(
        "video_encoder name={} path={}",
        render.encoder(),
        render.path()
    );
    let report = render.run(&mut |progress| {
        if progress.frame == 1
            || progress.frame == progress.total_frames
            || (progress.frame - 1).is_multiple_of(60)
        {
            println!(
                "progress frame={}/{}",
                progress.frame, progress.total_frames
            );
        }
    })?;
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

/// PNG holds straight alpha, so colour is divided back out of premultiplied pixels. Fully
/// transparent pixels stay zero.
fn unpremultiply_rgba8(pixels: &mut [u8]) {
    for pixel in pixels.as_chunks_mut::<4>().0 {
        let alpha = u32::from(pixel[3]);
        if alpha == 0 || alpha == 255 {
            continue;
        }
        for channel in &mut pixel[..3] {
            *channel = ((u32::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8;
        }
    }
}

fn write_png(output: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<()> {
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create output dir {}", parent.display()))?;
    }

    let mut png = Vec::new();
    let encoder = PngEncoder::new(&mut png);
    encoder
        .write_image(rgba, width, height, image::ExtendedColorType::Rgba8)
        .with_context(|| format!("failed to encode PNG {}", output.display()))?;

    fs::write(output, png).with_context(|| format!("failed to write PNG {}", output.display()))
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

    use super::{Cli, EncoderQuality, RateControl, parse_bitrate, render_to_partial};

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

    #[test]
    fn unpremultiply_restores_straight_colour() {
        let mut pixels = [
            64, 32, 0, 128, // half-covered colour
            0, 0, 0, 0, // transparent stays zero
            10, 20, 30, 255, // opaque is unchanged
        ];
        super::unpremultiply_rgba8(&mut pixels);
        assert_eq!(pixels, [128, 64, 0, 128, 0, 0, 0, 0, 10, 20, 30, 255]);
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
    fn rate_control_flags_conflict_and_validate() {
        let quality = |flags: &[&str]| parse(flags).unwrap().quality().unwrap();
        let output = ["--output", "o.mp4"];
        assert_eq!(
            quality(&[&output[..], &["--crf", "23", "--preset", "fast"]].concat()),
            EncoderQuality {
                rate_control: RateControl::Crf(23),
                preset: Some("fast".into()),
            }
        );
        assert_eq!(
            quality(&[&output[..], &["--bitrate", "8M"]].concat()).rate_control,
            RateControl::Bitrate(8_000_000)
        );
        assert_eq!(
            quality(&[&output[..], &["--qp", "30"]].concat()).rate_control,
            RateControl::Qp(30)
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
        assert!(parse(&["--output", "o.mp4", "--crf", "300"]).is_err());
        assert!(parse(&["--output", "o.mp4", "--bitrate", "0"]).is_err());
    }

    #[test]
    fn output_kind_rejects_flags_the_output_cannot_use() {
        let kind = |flags: &[&str]| parse(flags).unwrap().output_kind();
        assert!(kind(&["--output", "o.mp4", "--crf", "23"]).is_ok());
        assert!(kind(&["--output", "o.png", "--frame", "3"]).is_ok());

        let error = kind(&["--output", "o.png", "--encoder", "libx264"])
            .err()
            .unwrap();
        assert!(
            error
                .to_string()
                .contains("only supported when output is .mp4")
        );
        for quality in [["--bitrate", "14M"], ["--crf", "23"], ["--preset", "fast"]] {
            let error = kind(&["--output", "o.png", quality[0], quality[1]])
                .err()
                .unwrap();
            assert!(
                error
                    .to_string()
                    .contains("only supported when output is .mp4"),
                "{quality:?}"
            );
        }
        let error = kind(&["--output", "o.mp4", "--frame", "3"]).err().unwrap();
        assert!(
            error
                .to_string()
                .contains("only supported when output is .png")
        );
        let error = kind(&["--output", "o.gif"]).err().unwrap();
        assert!(error.to_string().contains("unsupported output extension"));
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

    #[test]
    fn parse_bitrate_accepts_suffixes() {
        assert_eq!(parse_bitrate("14000000").unwrap(), 14_000_000);
        assert_eq!(parse_bitrate("500k").unwrap(), 500_000);
        assert_eq!(parse_bitrate("14M").unwrap(), 14_000_000);
        assert!(parse_bitrate("0").is_err());
        assert!(parse_bitrate("fast").is_err());
        assert!(parse_bitrate("18446744073709551615M").is_err());
    }
}
