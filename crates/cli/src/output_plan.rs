use anyhow::{Result, bail};
use lumen_engine::composition::Composition;
use lumen_engine::render::{
    self, FrameRange, OutputFormat, RenderOutput, RenderPlan, RenderRequest, frame_number_width,
};
use lumen_ffmpeg::{QualityConfig, QualityRequest};

use crate::args::RenderArgs;

/// What to write, and where each frame goes.
pub(super) struct OutputPlan {
    pub(super) output: RenderOutput,
    pub(super) files: Files,
}

pub(super) enum Files {
    /// The output path, written whole.
    One,
    /// A name per frame.
    Sequence(SequencePattern),
}

/// A file name with one `%d` or `%0Nd` that a sequence replaces with each frame's number.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct SequencePattern {
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

    pub(super) fn file_name(&self, frame: u32, last_frame: u32) -> String {
        let width = match self.width {
            Some(width) => width.max(last_frame.to_string().len()),
            None => frame_number_width(last_frame),
        };
        format!("{}{frame:0width$}{}", self.before, self.after)
    }
}

/// The widest a `%0Nd` pattern may pad a frame number.
const MAX_PATTERN_WIDTH: usize = 20;

impl RenderArgs {
    /// What the command line asks to write, after checking that no image flag another kind of
    /// output can't use was given. [`render::plan`] checks the video flags.
    pub(super) fn plan(&self) -> Result<OutputPlan> {
        let output = self.output.as_path();
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
            ("mov", false) => OutputFormat::Mov,
            ("webm", false) => OutputFormat::Webm,
            ("mp4" | "mov" | "webm", true) => {
                bail!("a %d pattern writes images, so use .png or .jpg, not .{extension}")
            }
            ("png", false) => OutputFormat::Png,
            ("png", true) => OutputFormat::PngSequence,
            ("jpg" | "jpeg", false) => OutputFormat::Jpeg,
            ("jpg" | "jpeg", true) => OutputFormat::JpegSequence,
            _ => bail!(
                "unsupported output extension; use .mp4, .mov, .webm, .png, .jpg or .jpeg (got `{}`)",
                output.display()
            ),
        };
        if format.is_video() && self.image.frame.is_some() {
            bail!("--frame is only supported when output is an image; use --frames for a video");
        }
        if format.is_sequence() && self.image.frame.is_some() {
            bail!("--frame renders one image; use --frames START..END for a sequence");
        }
        if self.image.jpeg_quality.is_some() && !format.is_jpeg() {
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
        let frames = match (self.frames, self.image.frame) {
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
                jpeg_quality: self.image.jpeg_quality,
            },
            files: pattern.map_or(Files::One, Files::Sequence),
        })
    }

    /// What `output` renders of `composition`, with the video flags and the quality config.
    pub(super) fn render_plan(
        &self,
        output: &RenderOutput,
        composition: &Composition,
    ) -> Result<RenderPlan> {
        let request = RenderRequest {
            output: output.clone(),
            quality: self.video.quality_request(),
            default_encoder: QualityRequest::default().with_env_encoder().encoder,
        };
        // An image takes no quality config, so one that doesn't load doesn't stop it.
        let config = if output.format.is_video() {
            self.video.quality_config()?
        } else {
            QualityConfig::default()
        };
        Ok(render::plan(composition, &request, &config, None)?)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use lumen_ffmpeg::RateControl;

    use lumen_engine::render::{OutputFormat, RenderOutput};

    use super::SequencePattern;
    use crate::args::tests::parse;

    /// A one-frame composition of a black background.
    fn composition() -> lumen_engine::composition::Composition {
        lumen_engine::json::parse_value(&serde_json::json!({
            "timeline": { "duration_frames": 1 },
            "render_settings": { "width": 16, "height": 16 },
            "nodes": [
                { "id": 1, "type": "background", "params": { "paint": [0, 0, 0, 255], "width": 16, "height": 16 } },
                { "id": 2, "type": "media_output" }
            ],
            "connections": [{ "from_node": 1, "from_port": "output", "to_node": 2, "to_port": "source" }]
        }))
        .expect("composition")
    }

    #[test]
    fn plan_rejects_flags_the_output_cannot_use() {
        let plan = |flags: &[&str]| parse(flags).unwrap().plan();
        let message = |flags: &[&str]| plan(flags).err().unwrap().to_string();
        let render_message = |flags: &[&str]| {
            let args = parse(flags).unwrap();
            let output = args.plan().unwrap().output;
            let error = args.render_plan(&output, &composition()).unwrap_err();
            error.to_string()
        };
        assert!(plan(&["--output", "o.mp4", "--crf", "23"]).is_ok());
        assert!(plan(&["--output", "o.png", "--frame", "3"]).is_ok());

        assert!(
            render_message(&["--output", "o.png", "--encoder", "libx264"])
                .contains("only apply to the mp4, mov and webm formats")
        );
        for quality in [
            ["--bitrate", "14M"],
            ["--crf", "23"],
            ["--preset", "fast"],
            ["--quality", "high"],
            ["--codec", "hevc"],
            ["--bit-depth", "10"],
            ["--alpha", "true"],
        ] {
            for output in ["o.png", "o.jpg", "o_%d.png"] {
                assert!(
                    render_message(&["--output", output, quality[0], quality[1]])
                        .contains("only apply to the mp4, mov and webm formats"),
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
    fn quality_config_presets_resolve() {
        let dir = tempfile::tempdir().expect("temp dir");
        let config = dir.path().join("quality.json");
        fs::write(
            &config,
            r#"{ "presets": { "archive": { "encoders": { "libx264": { "crf": 16 } } } } }"#,
        )
        .expect("write config");
        let composition = composition();
        let choice = |flags: &[&str]| {
            let config = config.to_str().unwrap();
            let base = ["--output", "o.mp4", "--quality-config", config];
            parse(&[&base[..], flags].concat())
                .unwrap()
                .render_plan(&RenderOutput::of(OutputFormat::Mp4), &composition)
                .map(|plan| plan.encoder.expect("a video has encoders"))
        };

        let archive = choice(&["--quality", "archive"]).expect("archive resolves");
        assert_eq!(archive.candidates().len(), 1);
        assert_eq!(
            archive.first().quality.rate_control,
            Some(RateControl::Crf(16))
        );
        let error = choice(&["--quality", "cinema"]).unwrap_err();
        assert!(error.to_string().contains("`cinema`"), "{error}");
        let ten_bit_h264 = choice(&["--bit-depth", "10"]).unwrap_err();
        assert!(
            ten_bit_h264.to_string().contains("always 8-bit"),
            "{ten_bit_h264}"
        );
        let alpha_mp4 = choice(&["--alpha"]).unwrap_err();
        assert!(
            alpha_mp4.to_string().contains("can't hold alpha"),
            "{alpha_mp4}"
        );
    }
}
