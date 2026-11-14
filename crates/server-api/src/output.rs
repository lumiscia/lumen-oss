use serde::{Deserialize, Serialize};

/// What a render produces: a `format` and the `frames` of the timeline it covers. Without it a
/// render is an MP4 of the whole timeline.
///
/// Video quality and encoder settings are not part of it: they are the render request's own
/// fields, and apply to the video formats only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "metadata", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RenderOutput {
    #[serde(default)]
    pub format: OutputFormat,
    #[serde(default)]
    pub frames: FrameRange,
    /// `1` to `100`, for the JPEG formats only. Unset, `90`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "metadata", schemars(range(min = 1, max = 100)))]
    pub jpeg_quality: Option<u8>,
}

/// The file a render writes, or for a sequence the archive of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "metadata", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum OutputFormat {
    /// A video of the frames, with the composition's audio cut to the same span: H.264, HEVC or
    /// AV1, and AAC.
    #[default]
    Mp4,
    /// A QuickTime video of the frames in ProRes 4444, with alpha when asked for, and AAC.
    Mov,
    /// A WebM video of the frames in VP9, with alpha when asked for, and Opus.
    Webm,
    /// One frame as a PNG. Transparency is kept: the pixels are straight (not premultiplied)
    /// RGBA, as the renderer's final image has it.
    Png,
    /// One frame as an opaque JPEG. Transparent pixels are composited onto black, as they are in a
    /// video.
    Jpeg,
    /// One PNG per frame, in one ZIP.
    PngSequence,
    /// One opaque JPEG per frame, in one ZIP.
    JpegSequence,
}

impl OutputFormat {
    pub const ALL: [Self; 7] = [
        Self::Mp4,
        Self::Mov,
        Self::Webm,
        Self::Png,
        Self::Jpeg,
        Self::PngSequence,
        Self::JpegSequence,
    ];

    /// The name the API spells it with, such as `png_sequence`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::Mov => "mov",
            Self::Webm => "webm",
            Self::Png => "png",
            Self::Jpeg => "jpeg",
            Self::PngSequence => "png_sequence",
            Self::JpegSequence => "jpeg_sequence",
        }
    }

    /// The `Content-Type` of what a render of this format produces. A sequence is one ZIP.
    pub fn content_type(self) -> &'static str {
        match self {
            Self::Mp4 => "video/mp4",
            Self::Mov => "video/quicktime",
            Self::Webm => "video/webm",
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::PngSequence | Self::JpegSequence => "application/zip",
        }
    }

    /// The file extension, without the dot, of what a render of this format produces.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::Mov => "mov",
            Self::Webm => "webm",
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::PngSequence | Self::JpegSequence => "zip",
        }
    }

    pub fn is_video(self) -> bool {
        matches!(self, Self::Mp4 | Self::Mov | Self::Webm)
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
            Self::Mp4 | Self::Mov | Self::Webm => None,
        }
    }
}

/// The frames of the timeline a render covers, as `start..end` with `end` not included, like the
/// player's `range: { in, out }`. Frame numbers are the composition's, so a ranged render of frames
/// 30 to 60 draws what a full render draws there; its output starts at time 0.
///
/// `start` defaults to 0 and `end` to the timeline's end, except for a still, whose `end` defaults
/// to the frame after `start`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "metadata", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct FrameRange {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<u32>,
}

/// A video codec a render can ask for: `h264`, `hevc` or `av1` in an MP4, `prores` (ProRes 4444)
/// in a MOV, `vp9` in a WebM.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "metadata", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum VideoCodec {
    H264,
    Hevc,
    Av1,
    Prores,
    Vp9,
}

impl VideoCodec {
    /// The name the API spells it with, such as `hevc`.
    pub fn name(self) -> &'static str {
        match self {
            Self::H264 => "h264",
            Self::Hevc => "hevc",
            Self::Av1 => "av1",
            Self::Prores => "prores",
            Self::Vp9 => "vp9",
        }
    }
}

/// Bits per sample of a rendered video: `8` or `10`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub enum VideoBitDepth {
    Eight,
    Ten,
}

impl From<VideoBitDepth> for u8 {
    fn from(depth: VideoBitDepth) -> Self {
        match depth {
            VideoBitDepth::Eight => 8,
            VideoBitDepth::Ten => 10,
        }
    }
}

impl TryFrom<u8> for VideoBitDepth {
    type Error = String;

    fn try_from(bits: u8) -> Result<Self, String> {
        match bits {
            8 => Ok(Self::Eight),
            10 => Ok(Self::Ten),
            _ => Err(format!("bit depth must be 8 or 10, got {bits}")),
        }
    }
}

#[cfg(feature = "metadata")]
impl schemars::JsonSchema for VideoBitDepth {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "VideoBitDepth".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "description": "Bits per sample of a rendered video.",
            "type": "integer",
            "enum": [8, 10]
        })
    }
}
