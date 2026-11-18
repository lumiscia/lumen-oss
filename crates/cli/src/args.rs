use std::{
    env, fs,
    io::{IsTerminal, Read},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use lumen_engine::gpu::{DEFAULT_MAX_TEXT_GLYPHS, MAX_TEXT_GLYPHS_CEILING};
use lumen_engine::render::{
    FONT_DIR_ENV, FrameRange, GpuSettings, HARDWARE_DECODE_ENV, LocalMediaStore, RenderConfig,
    existing_font_dir, is_enabled_flag,
};
use lumen_ffmpeg::{
    BitDepth, EncodeCodec, EncoderQuality, QUALITY_CONFIG_ENV, QualityConfig, QualityRequest,
    parse_bitrate,
};

use crate::preview;

/// Where the reference for every option lives; the help prints one line pointing to it.
const REFERENCE: &str =
    "Reference: https://github.com/lumiscia/lumen-oss/tree/main/crates/cli#readme";

/// Renders, checks and previews Lumen compositions
#[derive(Debug, Parser)]
#[command(
    name = "lumen",
    version,
    arg_required_else_help = true,
    after_help = REFERENCE
)]
pub(super) struct Cli {
    #[command(subcommand)]
    pub(super) command: Command,
}

#[derive(Debug, Subcommand)]
pub(super) enum Command {
    /// Render a composition to a video, an image or an image sequence
    ///
    /// The output's extension picks what is written: .mp4, .mov or .webm is a video of the frames,
    /// .png or .jpg one image, and a name with %d an image per frame.
    #[command(args_override_self = true, after_help = REFERENCE)]
    Render(RenderArgs),

    /// Check a composition without rendering it
    ///
    /// Prints what is wrong with the composition, with fonts resolved as a render would resolve
    /// them. Exits 1 when there are errors; warnings alone exit 0.
    #[command(args_override_self = true, after_help = REFERENCE)]
    Check(CheckArgs),

    /// Write a contact sheet: frames sampled across the timeline in one image
    ///
    /// Renders --count frames spread evenly from the first frame of --frames to the last, and lays
    /// them out --columns to a row on a sheet --width pixels wide, as a .png or .jpg.
    #[command(args_override_self = true, after_help = REFERENCE)]
    Preview(preview::PreviewArgs),
}

/// The composition and where its media and fonts come from.
#[derive(Debug, Args)]
pub(super) struct SourceArgs {
    /// Composition JSON file, or - to read it from stdin
    ///
    /// A file named - is ./- instead. Media is still found under --media-root.
    #[arg(value_name = "COMPOSITION")]
    composition: PathBuf,

    /// Directory media sources must stay inside [default: the current directory]
    #[arg(long, value_name = "DIR")]
    media_root: Option<PathBuf>,

    /// Directory of fonts to look in first
    ///
    /// It must exist. Fonts come from it, then <media-root>/assets/fonts, then the system, matched
    /// by the family name inside each font file.
    #[arg(long, env = FONT_DIR_ENV, value_name = "DIR", value_parser = existing_font_dir)]
    font_dir: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub(super) struct CheckArgs {
    #[command(flatten)]
    pub(super) source: SourceArgs,

    /// How to print what is found
    #[arg(long, value_enum, default_value_t = Format::Text)]
    pub(super) format: Format,
}

#[derive(Debug, Args)]
pub(super) struct RenderArgs {
    /// Output file: .mp4, .mov, .webm, .png, .jpg, or a %d name for an image sequence
    ///
    /// A name with %d or %0Nd, such as frames/shot_%d.png, writes an image per frame. A video is
    /// written beside the output and moved into place when the render succeeds.
    #[arg(short, long, value_name = "PATH")]
    pub(super) output: PathBuf,

    /// Frames to render as START..END, END not included [default: all]
    ///
    /// Either side can be left out, as in 30.. or ..60. A video or sequence starts at time 0.
    #[arg(long, value_name = "START..END", value_parser = parse_frames)]
    pub(super) frames: Option<FrameRange>,

    #[command(flatten)]
    pub(super) source: SourceArgs,

    /// Most glyph instances one text node may draw
    ///
    /// Counts glyph, stroke and decoration instances. A text over it fails the render.
    #[arg(
        long,
        env = "LUMEN_MAX_TEXT_GLYPHS",
        value_name = "N",
        default_value_t = DEFAULT_MAX_TEXT_GLYPHS as u32,
        value_parser = clap::value_parser!(u32).range(1..=MAX_TEXT_GLYPHS_CEILING as i64)
    )]
    pub(super) max_text_glyphs: u32,

    #[command(flatten)]
    pub(super) video: VideoArgs,

    #[command(flatten)]
    pub(super) image: ImageArgs,
}

/// Flags only a video output takes.
#[derive(Debug, Args)]
#[command(next_help_heading = "Video options")]
pub(super) struct VideoArgs {
    /// Video codec: h264, hevc or av1 in .mp4, prores in .mov, vp9 in .webm
    #[arg(long, value_name = "CODEC")]
    codec: Option<EncodeCodec>,

    /// Quality preset: draft, standard, high, lossless, or one from --quality-config
    ///
    /// It can't go with --bitrate, --crf, --qp or --preset, which replace the preset's settings.
    #[arg(long, value_name = "NAME", conflicts_with_all = ["bitrate", "crf", "qp", "preset"])]
    quality: Option<String>,

    /// JSON file of quality presets and encoder orders
    #[arg(long, env = QUALITY_CONFIG_ENV, value_name = "PATH")]
    quality_config: Option<PathBuf>,

    /// Bits per sample: 8 or 10 (hevc and av1)
    #[arg(long, value_name = "BITS")]
    bit_depth: Option<BitDepth>,

    /// Keep transparency as an alpha channel (.mov or .webm)
    #[arg(
        long,
        value_name = "BOOL",
        num_args = 0..=1,
        default_missing_value = "true",
        hide_possible_values = true
    )]
    alpha: Option<bool>,

    /// Pin one encoder, with no fallback (also LUMEN_VIDEO_ENCODER)
    #[arg(long, value_name = "NAME")]
    encoder: Option<String>,

    /// Target bits per second, as 14000000, 14000k or 14M
    #[arg(long, value_name = "BPS|Nk|NM", value_parser = parse_bitrate, group = "rate_control")]
    bitrate: Option<u64>,

    /// Constant rate factor
    #[arg(long, value_name = "N", group = "rate_control")]
    crf: Option<u8>,

    /// Constant quantizer
    #[arg(long, value_name = "N", group = "rate_control")]
    qp: Option<u8>,

    /// Encoder speed preset
    #[arg(long, value_name = "NAME")]
    preset: Option<String>,
}

/// Flags only an image output takes.
#[derive(Debug, Args)]
#[command(next_help_heading = "Image options")]
pub(super) struct ImageArgs {
    /// Frame to render as one image, or a comma-separated list of frames for a %d sequence
    /// [default: 0]
    #[arg(
        long,
        value_name = "N[,N...]",
        value_delimiter = ',',
        conflicts_with = "frames"
    )]
    pub(super) frame: Option<Vec<u32>>,

    /// JPEG quality from 1 to 100 [default: 90]
    #[arg(
        long,
        value_name = "N",
        value_parser = clap::value_parser!(u8).range(1..=100)
    )]
    pub(super) jpeg_quality: Option<u8>,
}

pub(super) fn parse_frames(raw: &str) -> Result<FrameRange, String> {
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
pub(super) enum Format {
    /// One line per problem
    Text,
    /// `{ "ok", "diagnostics" }`, as `POST /renders/check` answers
    Json,
}

impl SourceArgs {
    /// The composition's JSON, from stdin when the argument is `-`.
    pub(super) fn read_composition(&self) -> Result<String> {
        if self.composition != Path::new("-") {
            return fs::read_to_string(&self.composition).with_context(|| {
                format!(
                    "failed to read composition file {}",
                    self.composition.display()
                )
            });
        }
        let mut stdin = std::io::stdin();
        if stdin.is_terminal() {
            bail!("expected a composition on stdin, but stdin is a terminal; pipe one in");
        }
        let mut raw = String::new();
        stdin
            .read_to_string(&mut raw)
            .context("failed to read the composition from stdin")?;
        if raw.trim().is_empty() {
            bail!("expected a composition on stdin, but it was empty");
        }
        Ok(raw)
    }

    /// What messages call the composition: its path, or `<stdin>`.
    pub(super) fn composition_name(&self) -> String {
        if self.composition == Path::new("-") {
            "<stdin>".to_owned()
        } else {
            self.composition.display().to_string()
        }
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

    pub(super) fn media_store(&self) -> Result<LocalMediaStore> {
        Ok(LocalMediaStore::new(
            media_root(self.media_root.as_deref())?,
            &self.render_config(),
        ))
    }
}

impl VideoArgs {
    /// The rate control and preset the command line asked for, or `None` if it asked for neither.
    fn quality(&self) -> Option<EncoderQuality> {
        EncoderQuality::from_options(self.bitrate, self.crf, self.qp, self.preset.clone())
            .expect("clap allows at most one rate control")
    }

    /// The quality settings the flags ask for. `LUMEN_VIDEO_ENCODER` is not one of them.
    pub(super) fn quality_request(&self) -> QualityRequest {
        QualityRequest {
            preset: self.quality.clone(),
            codec: self.codec,
            bit_depth: self.bit_depth,
            alpha: self.alpha,
            settings: self.quality(),
            encoder: self.encoder.clone(),
        }
    }

    pub(super) fn quality_config(&self) -> Result<QualityConfig> {
        let config_path = self
            .quality_config
            .as_deref()
            .filter(|path| !path.as_os_str().is_empty());
        QualityConfig::load(config_path).map_err(|error| anyhow!(error))
    }
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
pub(super) mod tests {
    use std::path::Path;

    use clap::{CommandFactory, Parser};

    use lumen_ffmpeg::RateControl;

    use lumen_engine::render::{FrameRange, OutputFormat};

    use super::{Cli, Command, EncoderQuality, RenderArgs, parse_frames};

    fn parse_cli(args: &[&str]) -> Result<Command, clap::Error> {
        Cli::try_parse_from(["lumen"].into_iter().chain(args.iter().copied()))
            .map(|cli| cli.command)
    }

    /// `lumen render in.json` with `flags`.
    pub(crate) fn parse(flags: &[&str]) -> Result<RenderArgs, clap::Error> {
        match parse_cli(&[&["render", "in.json"], flags].concat())? {
            Command::Render(args) => Ok(args),
            _ => unreachable!("parsed `render`"),
        }
    }

    #[test]
    fn each_subcommand_takes_the_composition_and_its_own_flags() {
        use clap::error::ErrorKind;

        let Command::Render(render) =
            parse_cli(&["render", "in.json", "-o", "o.mp4", "--codec", "hevc"]).unwrap()
        else {
            panic!("parsed `render`");
        };
        assert_eq!(render.source.composition, Path::new("in.json"));
        assert_eq!(render.output, Path::new("o.mp4"));

        let Command::Check(check) =
            parse_cli(&["check", "in.json", "--format", "json", "--media-root", "."]).unwrap()
        else {
            panic!("parsed `check`");
        };
        assert_eq!(check.source.composition, Path::new("in.json"));
        assert_eq!(check.format, super::Format::Json);

        let kind = |args: &[&str]| parse_cli(args).unwrap_err().kind();
        assert_eq!(
            kind(&["render", "in.json"]),
            ErrorKind::MissingRequiredArgument
        );
        assert_eq!(
            kind(&["render", "-o", "o.mp4"]),
            ErrorKind::MissingRequiredArgument
        );
        for render_only in [["--codec", "hevc"], ["--output", "o.mp4"], ["--frame", "3"]] {
            assert_eq!(
                kind(&[&["check", "in.json"], &render_only[..]].concat()),
                ErrorKind::UnknownArgument,
                "{render_only:?}"
            );
        }
        assert_eq!(
            kind(&["render", "in.json", "-o", "o.mp4", "--format", "json"]),
            ErrorKind::UnknownArgument
        );
        for removed in [
            &["--composition", "in.json", "--output", "o.mp4"][..],
            &["--composition", "in.json", "--check"],
            &["render", "--composition", "in.json", "-o", "o.mp4"],
        ] {
            assert_eq!(kind(removed), ErrorKind::UnknownArgument, "{removed:?}");
        }
    }

    #[test]
    fn no_arguments_prints_the_help() {
        let error = parse_cli(&[]).unwrap_err();
        assert_eq!(
            error.kind(),
            clap::error::ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        );
        let help = error.to_string();
        assert!(help.contains("render") && help.contains("check"), "{help}");
    }

    #[test]
    fn cli_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    /// The long flags `packages/lumen-cli/src/flags.ts` lists for `subcommand`.
    fn listed_flags(subcommand: &str) -> Vec<String> {
        const LIST: &str = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../packages/lumen-cli/src/flags.ts"
        ));
        let start = format!("{subcommand}: [");
        let list = LIST
            .split_once(&start)
            .and_then(|(_, rest)| rest.split_once(']'))
            .unwrap_or_else(|| panic!("flags.ts has no `{start}` list"))
            .0;
        list.split('"')
            .skip(1)
            .step_by(2)
            .map(String::from)
            .collect()
    }

    #[test]
    fn the_npm_wrapper_lists_every_long_flag_of_each_subcommand() {
        let cli = Cli::command();
        for name in ["render", "check", "preview"] {
            let subcommand = cli.find_subcommand(name).expect("a subcommand");
            let mut actual: Vec<String> = subcommand
                .get_arguments()
                .filter_map(|arg| arg.get_long())
                .filter(|long| !matches!(*long, "help" | "version"))
                .map(String::from)
                .collect();
            actual.sort();
            let listed = listed_flags(name);
            let missing: Vec<_> = actual
                .iter()
                .filter(|flag| !listed.contains(flag))
                .collect();
            let extra: Vec<_> = listed
                .iter()
                .filter(|flag| !actual.contains(flag))
                .collect();
            assert!(
                missing.is_empty() && extra.is_empty() && listed == actual,
                "`lumen {name}` and packages/lumen-cli/src/flags.ts disagree: missing from the \
                 file {missing:?}, extra in the file {extra:?} (the list must be sorted); update \
                 packages/lumen-cli/src/flags.ts"
            );
        }
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
        let quality = |flags: &[&str]| parse(flags).unwrap().video.quality().unwrap();
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
        assert!(parse(&["--output", "o.mp4", "--codec", "vp8"]).is_err());
        assert!(parse(&["--output", "o.mp4", "--bit-depth", "12"]).is_err());
        assert!(parse(&["--output", "o.mp4", "--crf", "300"]).is_err());
        assert!(parse(&["--output", "o.mp4", "--bitrate", "0"]).is_err());
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
    fn mov_and_webm_are_videos_that_can_ask_for_alpha() {
        let format = |output: &str| {
            parse(&["--output", output])
                .unwrap()
                .plan()
                .map(|plan| plan.output.format)
        };
        assert_eq!(format("o.mov").unwrap(), OutputFormat::Mov);
        assert_eq!(format("o.WEBM").unwrap(), OutputFormat::Webm);
        assert!(
            format("o_%d.webm")
                .unwrap_err()
                .to_string()
                .contains("not .webm")
        );
        let alpha = |flags: &[&str]| {
            parse(&[&["--output", "o.mov"], flags].concat())
                .unwrap()
                .video
                .alpha
        };
        assert_eq!(alpha(&[]), None);
        assert_eq!(alpha(&["--alpha"]), Some(true));
        assert_eq!(alpha(&["--alpha", "false"]), Some(false));
    }

    /// Runs `parsed_font_dir` in a child process, since clap reads `LUMEN_FONT_DIR` from the real
    /// environment and a test must not change that for the others.
    fn font_dir_with(env_dir: Option<&str>, flags: &[&str]) -> String {
        let mut child = std::process::Command::new(std::env::current_exe().expect("test binary"));
        child
            .args(["--exact", "args::tests::parsed_font_dir", "--nocapture"])
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
        println!("font_dir={:?}", cli.source.font_dir());
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
