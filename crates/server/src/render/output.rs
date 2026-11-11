use std::ops::Range;

use serde::{Deserialize, Serialize};

use super::{ErrorCode, ServerRenderError};

/// The JPEG quality used when [`RenderOutput::jpeg_quality`] is unset.
pub const DEFAULT_JPEG_QUALITY: u8 = 90;

/// The most frames a sequence can hold in an archive, which is what a ZIP's central directory counts
/// in 16 bits.
pub const MAX_ARCHIVE_FRAMES: u32 = u16::MAX as u32;

/// What a render produces: a [`format`](Self::format) and the [`frames`](Self::frames) of the
/// timeline it covers. The default is an MP4 of the whole timeline.
///
/// Video quality and encoder settings are not part of it: they stay in
/// [`RenderOptions::encoder`](super::RenderOptions::encoder) and apply to [`OutputFormat::Mp4`]
/// only. Build one with `..RenderOutput::default()` so that fields added later don't break it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RenderOutput {
    #[serde(default)]
    pub format: OutputFormat,
    #[serde(default)]
    pub frames: FrameRange,
    /// `1` to `100`, for the JPEG formats only. Unset, [`DEFAULT_JPEG_QUALITY`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jpeg_quality: Option<u8>,
}

/// The file a render writes, or for a sequence the archive of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum OutputFormat {
    /// A video of the frames, with the composition's audio cut to the same span.
    #[default]
    Mp4,
    /// One frame as a PNG. Transparency is kept: the pixels are straight (not premultiplied)
    /// RGBA, as the renderer's final image has it.
    Png,
    /// One frame as an opaque JPEG. Transparent pixels are composited onto black, as they are in a
    /// video.
    Jpeg,
    /// One PNG per frame.
    PngSequence,
    /// One opaque JPEG per frame.
    JpegSequence,
}

impl OutputFormat {
    /// The `Content-Type` of what a render of this format produces. A sequence is one ZIP.
    pub fn content_type(self) -> &'static str {
        match self {
            Self::Mp4 => "video/mp4",
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::PngSequence | Self::JpegSequence => "application/zip",
        }
    }

    /// The file extension, without the dot, of what a render of this format produces.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::PngSequence | Self::JpegSequence => "zip",
        }
    }

    pub fn is_video(self) -> bool {
        matches!(self, Self::Mp4)
    }

    /// One frame as one image.
    pub fn is_still(self) -> bool {
        matches!(self, Self::Png | Self::Jpeg)
    }

    pub fn is_sequence(self) -> bool {
        matches!(self, Self::PngSequence | Self::JpegSequence)
    }

    pub fn is_jpeg(self) -> bool {
        matches!(self, Self::Jpeg | Self::JpegSequence)
    }

    /// The extension of one frame of an image format, which is `None` for video.
    pub fn image_extension(self) -> Option<&'static str> {
        match self {
            Self::Png | Self::PngSequence => Some("png"),
            Self::Jpeg | Self::JpegSequence => Some("jpg"),
            Self::Mp4 => None,
        }
    }
}

/// The frames of the timeline a render covers, as `start..end` with `end` not included, like the
/// player's `range: { in, out }`. Frame numbers are the composition's, so a ranged render of frames
/// 30 to 60 draws what a full render draws there; its output starts at time 0.
///
/// `start` defaults to 0 and `end` to the timeline's end, except for a still, whose `end` defaults
/// to the frame after `start`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrameRange {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<u32>,
}

/// How much of a sequence a render is willing to produce. Each limit is also bounded by what a ZIP
/// holds. The default is that bound: [`MAX_ARCHIVE_FRAMES`] frames and 3 GiB of images.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SequenceLimits {
    /// The most frames. A longer range is refused before any GPU work.
    pub max_frames: u32,
    /// The most bytes the encoded frames add up to. A sequence that passes it ends with
    /// [`ErrorCode::OutputTooLarge`] and leaves nothing behind.
    pub max_bytes: u64,
}

impl Default for SequenceLimits {
    fn default() -> Self {
        Self {
            max_frames: MAX_ARCHIVE_FRAMES,
            max_bytes: 3 * 1024 * 1024 * 1024,
        }
    }
}

impl RenderOutput {
    /// A render of `format` over the whole timeline.
    pub fn of(format: OutputFormat) -> Self {
        Self {
            format,
            ..Self::default()
        }
    }

    /// The JPEG quality the render uses.
    pub fn jpeg_quality(&self) -> u8 {
        self.jpeg_quality.unwrap_or(DEFAULT_JPEG_QUALITY)
    }

    /// The frames to render, or [`ErrorCode::InvalidOutput`] for a range that is empty or reaches
    /// past a timeline of `duration_frames`, a JPEG quality outside `1..=100`, a JPEG quality on
    /// another format, or a still of more than one frame. Nothing here touches the GPU.
    pub fn resolve(&self, duration_frames: u32) -> Result<Range<u32>, ServerRenderError> {
        let invalid = |message: String| {
            Err(ServerRenderError::new(
                ErrorCode::InvalidOutput,
                message,
                false,
            ))
        };
        if let Some(quality) = self.jpeg_quality {
            if !self.format.is_jpeg() {
                return invalid("jpeg_quality only applies to the jpeg formats".to_string());
            }
            if !(1..=100).contains(&quality) {
                return invalid(format!("jpeg_quality must be 1 to 100, got {quality}"));
            }
        }
        let start = self.frames.start.unwrap_or(0);
        let end = match (self.frames.end, self.format.is_still()) {
            (Some(end), _) => end,
            (None, true) => start.saturating_add(1),
            (None, false) => duration_frames,
        };
        if start >= end {
            return invalid(format!(
                "the frame range {start}..{end} is empty; `end` is not included"
            ));
        }
        if end > duration_frames {
            return invalid(format!(
                "the frame range {start}..{end} reaches past the timeline's {duration_frames} frames"
            ));
        }
        if self.format.is_still() && end - start != 1 {
            return invalid(format!(
                "a still is one frame, but the frame range {start}..{end} has {}",
                end - start
            ));
        }
        Ok(start..end)
    }

    /// [`resolve`](Self::resolve), and for a sequence that its frames fit `limits`.
    pub fn validate(
        &self,
        duration_frames: u32,
        limits: &SequenceLimits,
    ) -> Result<Range<u32>, ServerRenderError> {
        let frames = self.resolve(duration_frames)?;
        let count = frames.end - frames.start;
        if self.format.is_sequence() && count > limits.max_frames.min(MAX_ARCHIVE_FRAMES) {
            return Err(ServerRenderError::new(
                ErrorCode::OutputTooLarge,
                format!(
                    "the sequence has {count} frames, and at most {} are allowed",
                    limits.max_frames.min(MAX_ARCHIVE_FRAMES)
                ),
                false,
            ));
        }
        Ok(frames)
    }
}

/// How many digits a sequence numbers its frames with so that the names of `last_frame` and every
/// frame before it sort in order: at least four.
pub fn frame_number_width(last_frame: u32) -> usize {
    last_frame.to_string().len().max(4)
}

/// The name of one frame of a sequence inside its archive.
pub(crate) fn archive_frame_name(frame: u32, last_frame: u32, extension: &str) -> String {
    format!(
        "frame_{frame:0width$}.{extension}",
        width = frame_number_width(last_frame)
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn output(value: serde_json::Value) -> RenderOutput {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn the_default_is_the_whole_timeline_as_an_mp4() {
        let default = RenderOutput::default();
        assert_eq!(default.format, OutputFormat::Mp4);
        assert_eq!(default.resolve(120).unwrap(), 0..120);
        assert_eq!(output(json!({})), default);
    }

    #[test]
    fn ranges_end_before_their_end_and_default_to_the_timeline() {
        let range = |frames: serde_json::Value| {
            output(json!({ "frames": frames }))
                .resolve(120)
                .map_err(|e| e.code())
        };
        assert_eq!(range(json!({ "start": 30, "end": 60 })), Ok(30..60));
        assert_eq!(range(json!({ "start": 30 })), Ok(30..120));
        assert_eq!(range(json!({ "end": 24 })), Ok(0..24));
        assert_eq!(range(json!({ "start": 119, "end": 120 })), Ok(119..120));
        for invalid in [
            json!({ "start": 5, "end": 5 }),
            json!({ "start": 6, "end": 5 }),
            json!({ "start": 120 }),
            json!({ "end": 121 }),
            json!({ "end": 0 }),
        ] {
            assert_eq!(
                range(invalid.clone()),
                Err(ErrorCode::InvalidOutput),
                "{invalid}"
            );
        }
    }

    #[test]
    fn a_still_is_the_frame_at_start() {
        let still = |value: serde_json::Value| output(value).resolve(10).map_err(|e| e.code());
        assert_eq!(still(json!({ "format": "png" })), Ok(0..1));
        assert_eq!(
            still(json!({ "format": "jpeg", "frames": { "start": 7 } })),
            Ok(7..8)
        );
        assert_eq!(
            still(json!({ "format": "png", "frames": { "start": 7, "end": 8 } })),
            Ok(7..8)
        );
        assert_eq!(
            still(json!({ "format": "png", "frames": { "start": 7, "end": 9 } })),
            Err(ErrorCode::InvalidOutput)
        );
        assert_eq!(
            still(json!({ "format": "png", "frames": { "start": 10 } })),
            Err(ErrorCode::InvalidOutput)
        );
    }

    #[test]
    fn jpeg_quality_is_one_to_a_hundred_and_only_for_jpeg() {
        let quality = |format: &str, quality: u8| {
            output(json!({ "format": format, "jpeg_quality": quality }))
                .resolve(10)
                .map_err(|e| e.code())
        };
        assert_eq!(quality("jpeg", 1), Ok(0..1));
        assert_eq!(quality("jpeg_sequence", 100), Ok(0..10));
        for (format, value) in [("jpeg", 0), ("jpeg", 101), ("png", 80), ("mp4", 80)] {
            assert_eq!(
                quality(format, value),
                Err(ErrorCode::InvalidOutput),
                "{format} {value}"
            );
        }
        assert_eq!(RenderOutput::default().jpeg_quality(), DEFAULT_JPEG_QUALITY);
    }

    #[test]
    fn sequences_are_bounded_before_rendering() {
        let limits = SequenceLimits {
            max_frames: 10,
            max_bytes: 1,
        };
        let sequence = |end: u32| {
            output(json!({ "format": "png_sequence", "frames": { "end": end } }))
                .validate(100, &limits)
                .map_err(|e| e.code())
        };
        assert_eq!(sequence(10), Ok(0..10));
        assert_eq!(sequence(11), Err(ErrorCode::OutputTooLarge));
        let video = output(json!({ "frames": { "end": 50 } }));
        assert_eq!(video.validate(100, &limits).unwrap(), 0..50);

        let unbounded = SequenceLimits::default();
        let long = output(json!({ "format": "png_sequence" }));
        assert!(long.validate(65_535, &unbounded).is_ok());
        assert_eq!(
            long.validate(65_536, &unbounded).map_err(|e| e.code()),
            Err(ErrorCode::OutputTooLarge)
        );
    }

    #[test]
    fn unknown_fields_and_formats_are_refused() {
        assert!(serde_json::from_value::<RenderOutput>(json!({ "format": "gif" })).is_err());
        assert!(serde_json::from_value::<RenderOutput>(json!({ "encoder": "libx264" })).is_err());
        assert!(
            serde_json::from_value::<RenderOutput>(json!({ "frames": { "from": 1 } })).is_err()
        );
    }

    #[test]
    fn formats_know_their_files() {
        let files: Vec<_> = [
            OutputFormat::Mp4,
            OutputFormat::Png,
            OutputFormat::Jpeg,
            OutputFormat::PngSequence,
            OutputFormat::JpegSequence,
        ]
        .map(|format| (format.content_type(), format.extension()))
        .into();
        assert_eq!(
            files,
            [
                ("video/mp4", "mp4"),
                ("image/png", "png"),
                ("image/jpeg", "jpg"),
                ("application/zip", "zip"),
                ("application/zip", "zip"),
            ]
        );
    }

    #[test]
    fn frame_names_are_wide_enough_to_sort() {
        assert_eq!(archive_frame_name(7, 9, "png"), "frame_0007.png");
        assert_eq!(
            archive_frame_name(99_999, 100_000, "jpg"),
            "frame_099999.jpg"
        );
        assert_eq!(
            archive_frame_name(100_000, 100_000, "jpg"),
            "frame_100000.jpg"
        );
        assert_eq!(frame_number_width(9_999), 4);
        assert_eq!(frame_number_width(10_000), 5);
    }
}
