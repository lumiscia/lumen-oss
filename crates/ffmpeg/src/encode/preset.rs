use std::{collections::BTreeMap, env, fmt, path::Path, str::FromStr};

use serde::Deserialize;

use super::choice::{EncoderCandidate, VideoEncoderChoice};
use super::codec::{BitDepth, EncoderFamily, check_alpha, check_bit_depth, encoder_info};
use super::quality::{
    EncoderQuality, RateControl, check_raw_options, one_rate_control, parse_bitrate,
    portable_options,
};
use crate::video::VideoCodec;

/// Names the operator config file.
pub const QUALITY_CONFIG_ENV: &str = "LUMEN_QUALITY_CONFIG";
const ENCODER_ENV: &str = "LUMEN_VIDEO_ENCODER";

/// The presets every host defines.
pub const BUILT_IN_PRESETS: [&str; 4] = ["draft", "standard", "high", "lossless"];
const DEFAULT_PRESET: &str = "standard";
const DEFAULT_CODEC: EncodeCodec = EncodeCodec::H264;

/// A video codec a render can ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EncodeCodec {
    H264,
    Hevc,
    Av1,
    /// ProRes 4444, 10-bit 4:4:4 with or without alpha.
    #[serde(rename = "prores")]
    ProRes,
    Vp9,
}

impl EncodeCodec {
    pub const ALL: [Self; 5] = [Self::H264, Self::Hevc, Self::Av1, Self::ProRes, Self::Vp9];

    pub fn name(self) -> &'static str {
        match self {
            Self::H264 => "h264",
            Self::Hevc => "hevc",
            Self::Av1 => "av1",
            Self::ProRes => "prores",
            Self::Vp9 => "vp9",
        }
    }

    pub fn of(codec: VideoCodec) -> Option<Self> {
        match codec {
            VideoCodec::H264 => Some(Self::H264),
            VideoCodec::Hevc => Some(Self::Hevc),
            VideoCodec::Av1 => Some(Self::Av1),
            VideoCodec::ProRes => Some(Self::ProRes),
            VideoCodec::Vp9 => Some(Self::Vp9),
            _ => None,
        }
    }

    /// The bit depth a render of this codec has when nothing asks for one: 10 for ProRes, which
    /// has no other, and 8 for the rest.
    pub fn default_bit_depth(self) -> BitDepth {
        match self {
            Self::ProRes => BitDepth::Ten,
            _ => BitDepth::Eight,
        }
    }

    /// Whether lumen can write this codec with an alpha channel.
    pub fn carries_alpha(self) -> bool {
        matches!(self, Self::ProRes | Self::Vp9)
    }

    /// The encoders tried without a `codecs` entry in the operator config: the platform's
    /// hardware encoder, then the software one.
    pub fn default_encoders(self) -> &'static [&'static str] {
        match self {
            Self::H264 if cfg!(target_os = "macos") => &["h264_videotoolbox", "libx264"],
            Self::Hevc if cfg!(target_os = "macos") => &["hevc_videotoolbox", "libx265"],
            Self::H264 if cfg!(target_os = "linux") => &["h264_vulkan", "libx264"],
            Self::Hevc if cfg!(target_os = "linux") => &["hevc_vulkan", "libx265"],
            Self::Av1 if cfg!(target_os = "linux") => &["av1_vulkan", "libsvtav1"],
            Self::H264 => &["libx264"],
            Self::Hevc => &["libx265"],
            Self::Av1 => &["libsvtav1"],
            Self::ProRes => &["prores_ks"],
            Self::Vp9 => &["libvpx-vp9"],
        }
    }
}

impl fmt::Display for EncodeCodec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for EncodeCodec {
    type Err = String;

    fn from_str(name: &str) -> Result<Self, String> {
        Self::ALL
            .into_iter()
            .find(|codec| codec.name() == name)
            .ok_or_else(|| {
                format!("unknown codec `{name}`; expected h264, hevc, av1, prores or vp9")
            })
    }
}

/// One built-in preset entry. `av1_vulkan`'s quantizer is an AV1 qindex, 0-255.
struct BuiltIn {
    rate_control: Option<RateControl>,
    preset: Option<&'static str>,
    options: &'static [(&'static str, &'static str)],
}

const fn set(rate_control: RateControl, preset: Option<&'static str>) -> BuiltIn {
    BuiltIn {
        rate_control: Some(rate_control),
        preset,
        options: &[],
    }
}

const fn raw(options: &'static [(&'static str, &'static str)]) -> BuiltIn {
    BuiltIn {
        rate_control: None,
        preset: None,
        options,
    }
}

use RateControl::{Bitrate, Crf, Qp};

/// Every built-in preset, by encoder. `crf` is `cq` on NVENC. Each level matches libx264's VMAF
/// at that level (crf 28 veryfast, 23 medium, 18 slow) on a natural clip and a motion-graphics
/// render, averaged: NVENC and Vulkan measured on an NVIDIA L4, `av1_vulkan` on an AMD 780M.
/// VideoToolbox is uncalibrated. `prores_ks` is ProRes 4444 at a fixed quantiser (8, 4, 2), and
/// `libvpx-vp9` is constant quality with `cpu-used` as its speed; both were set from size, speed
/// and PSNR against a lossless encode of a 1080p motion-graphics render on an AMD 780M host.
/// `lossless` lists only encoders that are truly lossless, so a codec's candidates skip to one of
/// those.
const BUILT_IN: [(&str, &[(&str, BuiltIn)]); 4] = [
    (
        "draft",
        &[
            ("libx264", set(Crf(28), Some("veryfast"))),
            ("h264_nvenc", set(Crf(34), Some("p2"))),
            ("h264_vulkan", set(Qp(36), None)),
            ("h264_videotoolbox", set(Bitrate(5_000_000), None)),
            ("libx265", set(Crf(30), Some("fast"))),
            ("hevc_nvenc", set(Crf(34), Some("p2"))),
            ("hevc_vulkan", set(Qp(36), None)),
            ("hevc_videotoolbox", set(Bitrate(3_000_000), None)),
            ("libsvtav1", set(Crf(48), Some("12"))),
            ("av1_nvenc", set(Crf(42), Some("p2"))),
            ("av1_vulkan", set(Qp(175), None)),
            ("prores_ks", set(Qp(8), None)),
            ("libvpx-vp9", set(Crf(40), Some("5"))),
        ],
    ),
    (
        "standard",
        &[
            ("libx264", set(Crf(23), None)),
            ("h264_nvenc", set(Crf(26), Some("p5"))),
            ("h264_vulkan", set(Qp(30), None)),
            ("h264_videotoolbox", set(Bitrate(12_000_000), None)),
            ("libx265", set(Crf(24), None)),
            ("hevc_nvenc", set(Crf(24), Some("p5"))),
            ("hevc_vulkan", set(Qp(28), None)),
            ("hevc_videotoolbox", set(Bitrate(8_000_000), None)),
            ("libsvtav1", set(Crf(39), Some("8"))),
            ("av1_nvenc", set(Crf(30), Some("p5"))),
            ("av1_vulkan", set(Qp(130), None)),
            ("prores_ks", set(Qp(4), None)),
            ("libvpx-vp9", set(Crf(32), Some("3"))),
        ],
    ),
    (
        "high",
        &[
            ("libx264", set(Crf(18), Some("slow"))),
            ("h264_nvenc", set(Crf(20), Some("p6"))),
            ("h264_vulkan", set(Qp(26), None)),
            ("h264_videotoolbox", set(Bitrate(20_000_000), None)),
            ("libx265", set(Crf(18), None)),
            ("hevc_nvenc", set(Crf(20), Some("p6"))),
            ("hevc_vulkan", set(Qp(22), None)),
            ("hevc_videotoolbox", set(Bitrate(14_000_000), None)),
            ("libsvtav1", set(Crf(30), Some("6"))),
            ("av1_nvenc", set(Crf(21), Some("p6"))),
            ("av1_vulkan", set(Qp(100), None)),
            ("prores_ks", set(Qp(2), None)),
            ("libvpx-vp9", set(Crf(24), Some("2"))),
        ],
    ),
    (
        "lossless",
        &[
            ("libx264", set(Qp(0), None)),
            ("h264_nvenc", raw(&[("tune", "lossless")])),
            ("libx265", raw(&[("x265-params", "lossless=1")])),
            ("hevc_nvenc", raw(&[("tune", "lossless")])),
            ("libvpx-vp9", raw(&[("lossless", "1")])),
        ],
    ),
];

#[derive(Debug, Clone, PartialEq, Eq)]
struct Preset {
    /// The codec used when neither the request nor the composition names one.
    codec: Option<EncodeCodec>,
    /// The bit depth used when neither the request nor the composition names one.
    bit_depth: Option<BitDepth>,
    encoders: BTreeMap<String, EncoderQuality>,
}

/// The quality presets a host defines and the encoders it tries for each codec: the built-ins,
/// changed by an operator config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QualityConfig {
    default_preset: String,
    default_codec: EncodeCodec,
    encoders: BTreeMap<EncodeCodec, Vec<String>>,
    presets: BTreeMap<String, Preset>,
}

/// What one render asks for. The caller puts the composition's choices where the request has none.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QualityRequest {
    pub preset: Option<String>,
    pub codec: Option<EncodeCodec>,
    /// The codec's [default](EncodeCodec::default_bit_depth) unless the request or its preset asks
    /// for another.
    pub bit_depth: Option<BitDepth>,
    /// An alpha channel, which only ProRes and VP9 hold and only their software encoders write.
    /// Unset is opaque.
    pub alpha: Option<bool>,
    /// Settings every candidate uses in place of a preset's, from lumen-local's rate-control flags.
    pub settings: Option<EncoderQuality>,
    /// The one encoder to use, with no fallback.
    pub encoder: Option<String>,
}

impl QualityRequest {
    /// Pins the encoder `LUMEN_VIDEO_ENCODER` names unless the request already pins one. Only
    /// binaries call this: [`QualityConfig::resolve`] reads no environment.
    pub fn with_env_encoder(self) -> Self {
        self.with_encoder_fallback(env::var(ENCODER_ENV).ok().as_deref())
    }

    fn with_encoder_fallback(mut self, fallback: Option<&str>) -> Self {
        let pinned = [self.encoder.as_deref(), fallback]
            .into_iter()
            .flatten()
            .map(str::trim)
            .find(|name| !name.is_empty())
            .map(str::to_string);
        self.encoder = pinned;
        self
    }
}

impl Default for QualityConfig {
    fn default() -> Self {
        Self::built_in()
    }
}

impl QualityConfig {
    pub fn built_in() -> Self {
        let presets = BUILT_IN
            .iter()
            .map(|(name, entries)| {
                let encoders = entries
                    .iter()
                    .map(|(encoder, entry)| {
                        let quality = EncoderQuality {
                            rate_control: entry.rate_control,
                            preset: entry.preset.map(str::to_string),
                            options: entry
                                .options
                                .iter()
                                .map(|(key, value)| (key.to_string(), value.to_string()))
                                .collect(),
                        };
                        (encoder.to_string(), quality)
                    })
                    .collect();
                let preset = Preset {
                    codec: None,
                    bit_depth: None,
                    encoders,
                };
                (name.to_string(), preset)
            })
            .collect();
        Self {
            default_preset: DEFAULT_PRESET.to_string(),
            default_codec: DEFAULT_CODEC,
            encoders: BTreeMap::new(),
            presets,
        }
    }

    /// The config named by `path`, or by `LUMEN_QUALITY_CONFIG` when `path` is `None`, or the
    /// built-ins when neither names one. An empty variable counts as unset.
    pub fn load(path: Option<&Path>) -> Result<Self, String> {
        let from_env = env::var_os(QUALITY_CONFIG_ENV).filter(|value| !value.is_empty());
        match path.or(from_env.as_deref().map(Path::new)) {
            Some(path) => {
                let json = std::fs::read_to_string(path).map_err(|error| {
                    format!("failed to read quality config {}: {error}", path.display())
                })?;
                Self::from_json(&json)
                    .map_err(|error| format!("quality config {}: {error}", path.display()))
            }
            None => Ok(Self::built_in()),
        }
    }

    /// Reads an operator config and checks every encoder and option in it against this FFmpeg
    /// build. Whether hardware accepts a value is only known when the encoder opens.
    pub fn from_json(json: &str) -> Result<Self, String> {
        let file: ConfigFile = serde_json::from_str(json).map_err(|error| error.to_string())?;
        let mut config = Self::built_in();

        for (codec, CodecFile { encoders }) in file.codecs {
            if encoders.is_empty() {
                return Err(format!("codecs.{codec}.encoders lists no encoders"));
            }
            for encoder in &encoders {
                let encodes = encoder_codec(encoder)
                    .map_err(|error| format!("codecs.{codec}.encoders: {error}"))?;
                if encodes != codec {
                    return Err(format!(
                        "codecs.{codec}.encoders: `{encoder}` encodes {encodes}, not {codec}"
                    ));
                }
            }
            config.encoders.insert(codec, encoders);
        }

        for (name, preset) in file.presets {
            let mut encoders = BTreeMap::new();
            for (encoder, entry) in preset.encoders {
                let at = format!("presets.{name}.encoders.{encoder}");
                let quality = entry
                    .into_quality(&encoder)
                    .map_err(|error| format!("{at}: {error}"))?;
                encoders.insert(encoder, quality);
            }
            match config.presets.get_mut(&name) {
                Some(built_in) => {
                    if preset.codec.is_some() {
                        return Err(format!(
                            "presets.{name}.codec: built-in presets have no codec"
                        ));
                    }
                    if preset.bit_depth.is_some() {
                        return Err(format!(
                            "presets.{name}.bit_depth: built-in presets have no bit depth"
                        ));
                    }
                    built_in.encoders.extend(encoders);
                }
                None => {
                    config.presets.insert(
                        name,
                        Preset {
                            codec: preset.codec,
                            bit_depth: preset.bit_depth,
                            encoders,
                        },
                    );
                }
            }
        }

        if let Some(default_preset) = file.default_preset {
            if !config.presets.contains_key(&default_preset) {
                return Err(format!(
                    "default_preset `{default_preset}` isn't a built-in or configured preset"
                ));
            }
            config.default_preset = default_preset;
        }
        if let Some(default_codec) = file.default_codec {
            config.default_codec = default_codec;
        }
        Ok(config)
    }

    /// The encoders a render tries for `request`, in order, each with its preset's settings, at
    /// the request's bit depth and with alpha when it asks for it. Errors when the preset isn't
    /// defined, when no encoder of the codec has an entry in it, is in this FFmpeg build or can
    /// write the bit depth and alpha, or when the pinned encoder isn't in the build or doesn't fit
    /// the request's codec, bit depth, alpha or the preset. With alpha, hardware encoders are left
    /// out, so a codec goes straight to its software encoder.
    pub fn resolve(&self, request: &QualityRequest) -> Result<VideoEncoderChoice, String> {
        self.resolve_with(request, encoder_codec, |encoder, depth, alpha| {
            check_bit_depth(encoder, depth)?;
            if alpha {
                check_alpha(encoder, depth)?;
            }
            Ok(())
        })
    }

    fn resolve_with(
        &self,
        request: &QualityRequest,
        codec_of: impl Fn(&str) -> Result<EncodeCodec, String>,
        writes: impl Fn(&str, BitDepth, bool) -> Result<(), String>,
    ) -> Result<VideoEncoderChoice, String> {
        let pinned = request
            .encoder
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty());
        let name = request.preset.as_deref().unwrap_or(&self.default_preset);
        let preset = self.presets.get(name).ok_or_else(|| {
            format!(
                "quality preset `{name}` isn't defined on this host; built-in: {}",
                BUILT_IN_PRESETS.join(", ")
            )
        })?;
        let quality_for = |encoder: &str| match &request.settings {
            Some(settings) => Some(settings.clone()),
            None => preset.encoders.get(encoder).cloned(),
        };
        let alpha = request.alpha.unwrap_or(false);
        let bit_depth_for = |codec: EncodeCodec| {
            request
                .bit_depth
                .or(preset.bit_depth)
                .unwrap_or(codec.default_bit_depth())
        };

        if let Some(encoder) = pinned {
            let encodes = codec_of(encoder)?;
            if let Some(asked) = request.codec
                && asked != encodes
            {
                return Err(format!(
                    "this host encodes with `{encoder}`, which encodes {encodes}, not {asked}"
                ));
            }
            let quality = quality_for(encoder).ok_or_else(|| {
                format!("quality preset `{name}` has no settings for `{encoder}`, the encoder this host uses")
            })?;
            let bit_depth = bit_depth_for(encodes);
            writes(encoder, bit_depth, alpha)?;
            return Ok(VideoEncoderChoice::single(encoder, quality)
                .with_bit_depth(bit_depth)
                .with_alpha(alpha));
        }

        let codec = request.codec.or(preset.codec).unwrap_or(self.default_codec);
        let bit_depth = bit_depth_for(codec);
        if alpha && !codec.carries_alpha() {
            return Err(format!(
                "{codec} output can't carry alpha; ask for prores (in .mov) or vp9 (in .webm)"
            ));
        }
        match (codec, bit_depth, alpha) {
            (EncodeCodec::H264, BitDepth::Ten, _) => {
                return Err(
                    "H.264 output is always 8-bit; ask for hevc or av1 to encode 10-bit"
                        .to_string(),
                );
            }
            (EncodeCodec::ProRes, BitDepth::Eight, _) => {
                return Err(
                    "ProRes 4444 is always 10-bit; leave bit_depth unset or ask for 10".to_string(),
                );
            }
            (EncodeCodec::Vp9, BitDepth::Ten, true) => {
                return Err(
                    "VP9 with alpha is 8-bit; leave bit_depth unset or ask for 8".to_string(),
                );
            }
            _ => {}
        }
        let encoders = self.encoders(codec);
        let candidates: Vec<_> = encoders
            .iter()
            .filter_map(|encoder| {
                Some(EncoderCandidate {
                    encoder: encoder.clone(),
                    quality: quality_for(encoder)?,
                })
            })
            .collect();
        if !candidates.is_empty()
            && !candidates
                .iter()
                .any(|candidate| codec_of(&candidate.encoder).is_ok())
        {
            return Err(format!(
                "none of the {codec} encoders quality preset `{name}` has settings for are in this FFmpeg build: {}",
                candidates
                    .iter()
                    .map(|candidate| candidate.encoder.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        let mut refusals = Vec::new();
        let candidates: Vec<_> = candidates
            .into_iter()
            .filter(
                |candidate| match writes(&candidate.encoder, bit_depth, alpha) {
                    Ok(()) => true,
                    Err(reason) => {
                        refusals.push(reason);
                        false
                    }
                },
            )
            .collect();
        if candidates.is_empty() && !refusals.is_empty() {
            let with_alpha = if alpha { " with alpha" } else { "" };
            return Err(format!(
                "no {codec} encoder this host tries can encode {bit_depth}{with_alpha}: {}",
                refusals.join("; ")
            ));
        }
        VideoEncoderChoice::new(candidates)
            .map(|choice| choice.with_bit_depth(bit_depth).with_alpha(alpha))
            .ok_or_else(|| {
                format!(
                    "quality preset `{name}` has no settings for any {codec} encoder this host tries: {}",
                    encoders.join(", ")
                )
            })
    }

    /// The encoders tried for `codec`, in order.
    pub fn encoders(&self, codec: EncodeCodec) -> Vec<String> {
        match self.encoders.get(&codec) {
            Some(encoders) => encoders.clone(),
            None => codec
                .default_encoders()
                .iter()
                .map(|encoder| encoder.to_string())
                .collect(),
        }
    }
}

/// The codec `encoder` writes, if this build has it and it's one a render can ask for.
fn encoder_codec(encoder: &str) -> Result<EncodeCodec, String> {
    let info = encoder_info(encoder).map_err(|error| error.message)?;
    EncodeCodec::of(info.codec).ok_or_else(|| {
        format!(
            "`{encoder}` encodes {:?}; presets cover h264, hevc, av1, prores and vp9",
            info.codec
        )
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    default_preset: Option<String>,
    default_codec: Option<EncodeCodec>,
    #[serde(default)]
    codecs: BTreeMap<EncodeCodec, CodecFile>,
    #[serde(default)]
    presets: BTreeMap<String, PresetFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CodecFile {
    encoders: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PresetFile {
    codec: Option<EncodeCodec>,
    bit_depth: Option<BitDepth>,
    #[serde(default)]
    encoders: BTreeMap<String, EncoderFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EncoderFile {
    crf: Option<u8>,
    qp: Option<u8>,
    bitrate: Option<BitrateFile>,
    preset: Option<String>,
    #[serde(default)]
    options: BTreeMap<String, String>,
}

/// Bits per second as a number, or as text with `k` and `M` suffixes, such as `"14M"`.
#[derive(Deserialize)]
#[serde(untagged)]
enum BitrateFile {
    BitsPerSecond(u64),
    Text(String),
}

impl EncoderFile {
    fn into_quality(self, encoder: &str) -> Result<EncoderQuality, String> {
        encoder_codec(encoder)?;
        let bitrate = self
            .bitrate
            .map(|bitrate| match bitrate {
                BitrateFile::BitsPerSecond(bits) => parse_bitrate(&bits.to_string()),
                BitrateFile::Text(text) => parse_bitrate(&text),
            })
            .transpose()?;
        let mut options = self.options;
        // A speed preset lumen knows the values of is checked the same way in either spelling.
        let preset = match self.preset {
            None if matches!(
                EncoderFamily::of(encoder),
                EncoderFamily::X264
                    | EncoderFamily::X265
                    | EncoderFamily::SvtAv1
                    | EncoderFamily::Nvenc
            ) =>
            {
                options.remove("preset")
            }
            None if EncoderFamily::of(encoder) == EncoderFamily::Vpx => options.remove("cpu-used"),
            preset => preset,
        };
        let quality = EncoderQuality {
            rate_control: one_rate_control(bitrate, self.crf, self.qp)?,
            preset,
            options: options.into_iter().collect(),
        };
        let portable = portable_options(encoder, &quality)
            .map_err(|error| error.message)?
            .private;
        let bitrate_key =
            matches!(quality.rate_control, Some(RateControl::Bitrate(_))).then_some("b");
        for (key, _) in &quality.options {
            let portable_key = portable
                .iter()
                .map(|(name, _)| *name)
                .chain(bitrate_key)
                .any(|name| name == key);
            if portable_key {
                return Err(format!(
                    "options.{key} is also set by this entry's crf, qp, bitrate or preset, or always by lumen for `{encoder}`"
                ));
            }
        }
        check_raw_options(encoder, &quality.options).map_err(|error| error.message)?;
        Ok(quality)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn config(value: serde_json::Value) -> Result<QualityConfig, String> {
        QualityConfig::from_json(&value.to_string())
    }

    fn request(preset: Option<&str>, codec: Option<EncodeCodec>) -> QualityRequest {
        QualityRequest {
            preset: preset.map(str::to_string),
            codec,
            ..QualityRequest::default()
        }
    }

    /// The codec each encoder name writes, standing in for this FFmpeg build. `missing_*` isn't in it.
    fn codec_by_name(encoder: &str) -> Result<EncodeCodec, String> {
        if encoder.starts_with("missing") {
            Err(format!("encoder `{encoder}` not found"))
        } else if encoder == "libx264" || encoder.starts_with("h264_") {
            Ok(EncodeCodec::H264)
        } else if encoder == "libx265" || encoder.starts_with("hevc_") {
            Ok(EncodeCodec::Hevc)
        } else if encoder.starts_with("prores") {
            Ok(EncodeCodec::ProRes)
        } else if encoder == "libvpx-vp9" || encoder.starts_with("vp9_") {
            Ok(EncodeCodec::Vp9)
        } else {
            Ok(EncodeCodec::Av1)
        }
    }

    /// Which encoders write 10-bit and alpha, standing in for this FFmpeg build.
    fn depth_by_name(encoder: &str, depth: BitDepth, alpha: bool) -> Result<(), String> {
        let eight_bit_only = encoder == "libx264"
            || encoder.starts_with("h264_")
            || encoder.ends_with("_videotoolbox");
        let codec = codec_by_name(encoder)?;
        if depth == BitDepth::Ten && eight_bit_only {
            Err(format!("`{encoder}` can't encode 10-bit"))
        } else if codec == EncodeCodec::ProRes && depth == BitDepth::Eight {
            Err(format!("`{encoder}` is always 10-bit"))
        } else if alpha && EncoderFamily::of(encoder).is_hardware() {
            Err(format!("`{encoder}` is a hardware encoder"))
        } else if alpha && !codec.carries_alpha() {
            Err(format!("`{encoder}` can't write alpha"))
        } else {
            Ok(())
        }
    }

    impl QualityConfig {
        fn resolve_with_env(
            &self,
            request: &QualityRequest,
            from_env: Option<&str>,
        ) -> Result<VideoEncoderChoice, String> {
            let request = request.clone().with_encoder_fallback(from_env);
            self.resolve_with(&request, codec_by_name, depth_by_name)
        }
    }

    fn encoders(choice: &VideoEncoderChoice) -> Vec<&str> {
        choice
            .candidates()
            .iter()
            .map(|candidate| candidate.encoder.as_str())
            .collect()
    }

    #[test]
    fn every_built_in_entry_is_one_its_encoder_accepts() {
        let config = QualityConfig::built_in();
        assert_eq!(
            config
                .presets
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            {
                let mut names = BUILT_IN_PRESETS.to_vec();
                names.sort();
                names
            }
        );
        for (name, preset) in &config.presets {
            for (encoder, quality) in &preset.encoders {
                assert_eq!(quality.check_encoder(encoder), Ok(()), "{name} {encoder}");
                if encoder_info(encoder).is_ok() {
                    assert_eq!(
                        check_raw_options(encoder, &quality.options).map_err(|e| e.message),
                        Ok(()),
                        "{name} {encoder}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_default_is_standard_h264_on_the_platform_encoders() {
        let choice = QualityConfig::built_in()
            .resolve_with_env(&QualityRequest::default(), None)
            .unwrap();
        assert_eq!(encoders(&choice), EncodeCodec::H264.default_encoders());
        assert_eq!(
            choice.candidates()[0].quality,
            QualityConfig::built_in().presets["standard"].encoders
                [EncodeCodec::H264.default_encoders()[0]]
        );
    }

    #[test]
    fn operator_defaults_apply_only_where_nothing_more_specific_is_named() {
        let config = config(json!({
            "default_preset": "archive",
            "default_codec": "hevc",
            "codecs": { "h264": { "encoders": ["libx264"] } },
            "presets": {
                "archive": {
                    "codec": "h264",
                    "encoders": { "libx264": { "crf": 16 } }
                }
            }
        }))
        .unwrap();
        let resolve = |preset, codec| config.resolve_with_env(&request(preset, codec), None);

        let archive = resolve(None, None).unwrap();
        assert_eq!(encoders(&archive), ["libx264"]);
        assert_eq!(
            archive.candidates()[0].quality.rate_control,
            Some(RateControl::Crf(16))
        );
        // A named preset without a codec takes the operator's default codec.
        let high = resolve(Some("high"), None);
        assert_eq!(
            high.map(|choice| encoders(&choice).len()).ok(),
            Some(EncodeCodec::Hevc.default_encoders().len())
        );
        // A request's codec wins over the preset's.
        assert!(resolve(None, Some(EncodeCodec::Av1)).is_err());
        let error = resolve(Some("archive"), Some(EncodeCodec::Hevc)).unwrap_err();
        assert!(error.contains("hevc"), "{error}");
    }

    #[test]
    fn an_unknown_preset_lists_the_built_ins() {
        let error = QualityConfig::built_in()
            .resolve_with_env(&request(Some("archive"), None), None)
            .unwrap_err();
        assert_eq!(
            error,
            "quality preset `archive` isn't defined on this host; built-in: draft, standard, high, lossless"
        );
    }

    #[test]
    fn candidates_are_the_codecs_encoders_the_preset_has_entries_for() {
        let config = config(json!({
            "codecs": { "h264": { "encoders": ["h264_vulkan", "libx264"] } }
        }))
        .unwrap();
        let lossless = config
            .resolve_with_env(&request(Some("lossless"), None), None)
            .unwrap();
        assert_eq!(encoders(&lossless), ["libx264"]);
        assert_eq!(
            lossless.candidates()[0].quality.rate_control,
            Some(RateControl::Qp(0))
        );
        let high = config
            .resolve_with_env(&request(Some("high"), None), None)
            .unwrap();
        assert_eq!(encoders(&high), ["h264_vulkan", "libx264"]);

        let error = QualityConfig::built_in()
            .resolve_with_env(&request(Some("lossless"), Some(EncodeCodec::Av1)), None)
            .unwrap_err();
        assert!(error.contains("av1"), "{error}");
    }

    #[test]
    fn a_pinned_encoder_has_no_fallback_and_must_fit_the_request() {
        let config = QualityConfig::built_in();
        let pinned = config
            .resolve_with_env(&request(Some("high"), None), Some(" libx264 "))
            .unwrap();
        assert_eq!(encoders(&pinned), ["libx264"]);
        assert_eq!(
            pinned.candidates()[0].quality.preset.as_deref(),
            Some("slow")
        );
        // The request's encoder wins over the environment's.
        let named = QualityRequest {
            encoder: Some("libx264".to_string()),
            ..QualityRequest::default()
        };
        assert_eq!(
            encoders(
                &config
                    .resolve_with_env(&named, Some("h264_vulkan"))
                    .unwrap()
            ),
            ["libx264"]
        );

        let codec = config
            .resolve_with_env(&request(None, Some(EncodeCodec::Hevc)), Some("libx264"))
            .unwrap_err();
        assert!(
            codec.contains("libx264") && codec.contains("hevc"),
            "{codec}"
        );
        let no_entry = config
            .resolve_with_env(&request(Some("lossless"), None), Some("h264_vulkan"))
            .unwrap_err();
        assert!(no_entry.contains("h264_vulkan"), "{no_entry}");
        let missing = config
            .resolve_with_env(&request(None, None), Some("missing_encoder"))
            .unwrap_err();
        assert!(missing.contains("missing_encoder"), "{missing}");
    }

    #[test]
    fn a_request_without_an_encoder_takes_the_fallback() {
        let pin = |encoder: Option<&str>, fallback: Option<&str>| {
            QualityRequest {
                encoder: encoder.map(str::to_string),
                ..QualityRequest::default()
            }
            .with_encoder_fallback(fallback)
            .encoder
        };
        assert_eq!(pin(None, None), None);
        assert_eq!(pin(None, Some(" libx264 ")).as_deref(), Some("libx264"));
        assert_eq!(
            pin(Some("libx265"), Some("libx264")).as_deref(),
            Some("libx265")
        );
        assert_eq!(pin(Some(" "), Some("libx264")).as_deref(), Some("libx264"));
        assert_eq!(pin(None, Some("")), None);
    }

    #[test]
    fn a_codec_needs_one_candidate_in_this_build() {
        let config = QualityConfig::built_in();
        let hevc = request(None, Some(EncodeCodec::Hevc));
        let error = config
            .resolve_with(&hevc, |_| Err("not built".to_string()), depth_by_name)
            .unwrap_err();
        assert!(error.contains("libx265"), "{error}");
        // A missing candidate before an available one is left to fall through at look-up.
        let only_software = |encoder: &str| match encoder {
            "libx265" => Ok(EncodeCodec::Hevc),
            _ => Err("not built".to_string()),
        };
        let choice = config
            .resolve_with(&hevc, only_software, depth_by_name)
            .unwrap();
        assert_eq!(encoders(&choice), EncodeCodec::Hevc.default_encoders());
    }

    #[test]
    fn ten_bit_is_asked_for_and_never_given_by_an_encoder_that_cant() {
        let ten = |codec, encoder: Option<&str>| QualityRequest {
            codec,
            bit_depth: Some(BitDepth::Ten),
            encoder: encoder.map(str::to_string),
            ..QualityRequest::default()
        };
        let config = QualityConfig::built_in();
        let default = config
            .resolve_with_env(&QualityRequest::default(), None)
            .unwrap();
        assert_eq!(default.bit_depth(), BitDepth::Eight);

        let hevc = config
            .resolve_with_env(&ten(Some(EncodeCodec::Hevc), None), None)
            .unwrap();
        assert_eq!(hevc.bit_depth(), BitDepth::Ten);
        assert!(encoders(&hevc).contains(&"libx265"), "{hevc:?}");
        assert!(
            !encoders(&hevc)
                .iter()
                .any(|name| name.ends_with("_videotoolbox"))
        );

        let h264 = config.resolve_with_env(&ten(None, None), None).unwrap_err();
        assert!(h264.contains("always 8-bit"), "{h264}");
        let pinned = config
            .resolve_with_env(&ten(None, Some("hevc_videotoolbox")), None)
            .unwrap_err();
        assert!(pinned.contains("hevc_videotoolbox"), "{pinned}");

        let only_videotoolbox = self::config(json!({
            "codecs": { "hevc": { "encoders": ["hevc_videotoolbox"] } }
        }));
        if let Ok(only_videotoolbox) = only_videotoolbox {
            let error = only_videotoolbox
                .resolve_with_env(&ten(Some(EncodeCodec::Hevc), None), None)
                .unwrap_err();
            assert!(error.contains("can encode 10-bit"), "{error}");
        }
    }

    #[test]
    fn alpha_goes_to_the_software_encoder_of_prores_or_vp9() {
        let alpha = |codec, encoder: Option<&str>, bit_depth| QualityRequest {
            codec,
            bit_depth,
            alpha: Some(true),
            encoder: encoder.map(str::to_string),
            ..QualityRequest::default()
        };
        let mut config = QualityConfig::built_in();
        // A hardware VP9 encoder ahead of libvpx, as an operator could list one.
        config.encoders.insert(
            EncodeCodec::Vp9,
            vec!["vp9_vulkan".into(), "libvpx-vp9".into()],
        );
        let standard = config.presets.get_mut("standard").unwrap();
        standard
            .encoders
            .insert("vp9_vulkan".into(), EncoderQuality::default());

        let prores = config
            .resolve_with_env(&alpha(Some(EncodeCodec::ProRes), None, None), None)
            .unwrap();
        assert_eq!(encoders(&prores), ["prores_ks"]);
        assert_eq!((prores.bit_depth(), prores.alpha()), (BitDepth::Ten, true));
        let vp9 = config
            .resolve_with_env(&alpha(Some(EncodeCodec::Vp9), None, None), None)
            .unwrap();
        assert_eq!(encoders(&vp9), ["libvpx-vp9"]);
        assert_eq!((vp9.bit_depth(), vp9.alpha()), (BitDepth::Eight, true));
        let opaque = config
            .resolve_with_env(&request(None, Some(EncodeCodec::Vp9)), None)
            .unwrap();
        assert_eq!(encoders(&opaque), ["vp9_vulkan", "libvpx-vp9"]);
        assert!(!opaque.alpha());

        let refused = |request: QualityRequest, expected: &str| {
            let error = config.resolve_with_env(&request, None).unwrap_err();
            assert!(error.contains(expected), "{expected}: {error}");
        };
        refused(alpha(None, None, None), "h264 output can't carry alpha");
        refused(
            alpha(Some(EncodeCodec::Hevc), None, None),
            "prores (in .mov)",
        );
        refused(alpha(None, Some("h264_vulkan"), None), "hardware encoder");
        refused(alpha(None, Some("libx264"), None), "can't write alpha");
        refused(
            alpha(Some(EncodeCodec::ProRes), None, Some(BitDepth::Eight)),
            "always 10-bit",
        );
        refused(
            alpha(Some(EncodeCodec::Vp9), None, Some(BitDepth::Ten)),
            "VP9 with alpha is 8-bit",
        );
        let only_hardware = alpha(Some(EncodeCodec::Vp9), Some("vp9_vulkan"), None);
        refused(only_hardware, "hardware encoder");

        let opaque_prores = config
            .resolve_with_env(&request(None, Some(EncodeCodec::ProRes)), None)
            .unwrap();
        assert_eq!(opaque_prores.bit_depth(), BitDepth::Ten);
        assert!(!opaque_prores.alpha());
        let lossless = config
            .resolve_with_env(&request(Some("lossless"), Some(EncodeCodec::ProRes)), None)
            .unwrap_err();
        assert!(
            lossless.contains("no settings for any prores encoder"),
            "{lossless}"
        );
    }

    #[test]
    fn a_configured_preset_can_ask_for_ten_bit() {
        let config = config(json!({
            "presets": {
                "master": {
                    "codec": "hevc",
                    "bit_depth": 10,
                    "encoders": { "libx265": { "crf": 16 } }
                }
            }
        }));
        // This FFmpeg build may not have libx265.
        let Ok(config) = config else { return };
        let choice = config
            .resolve_with_env(&request(Some("master"), None), None)
            .unwrap();
        assert_eq!(choice.bit_depth(), BitDepth::Ten);
        let eight = QualityRequest {
            preset: Some("master".to_string()),
            bit_depth: Some(BitDepth::Eight),
            ..QualityRequest::default()
        };
        assert_eq!(
            config.resolve_with_env(&eight, None).unwrap().bit_depth(),
            BitDepth::Eight
        );
    }

    #[test]
    fn flag_settings_replace_the_preset_on_every_candidate() {
        let settings = EncoderQuality {
            rate_control: Some(RateControl::Crf(20)),
            ..EncoderQuality::default()
        };
        let config = config(json!({
            "default_preset": "archive",
            "default_codec": "av1",
            "presets": { "archive": { "codec": "h264", "encoders": { "libx264": { "crf": 16 } } } }
        }))
        .unwrap();
        let choice = config
            .resolve_with_env(
                &QualityRequest {
                    settings: Some(settings.clone()),
                    ..QualityRequest::default()
                },
                None,
            )
            .unwrap();
        // The preset still picks the codec.
        assert_eq!(encoders(&choice), EncodeCodec::H264.default_encoders());
        assert!(
            choice
                .candidates()
                .iter()
                .all(|candidate| candidate.quality == settings)
        );
    }

    #[test]
    fn a_config_entry_replaces_only_that_encoders_built_in() {
        let config = config(json!({
            "presets": { "standard": { "encoders": { "libx264": { "qp": 21, "options": { "tune": "film" } } } } }
        }))
        .unwrap();
        let standard = &config.presets["standard"].encoders;
        assert_eq!(
            standard["libx264"],
            EncoderQuality {
                rate_control: Some(RateControl::Qp(21)),
                preset: None,
                options: vec![("tune".to_string(), "film".to_string())],
            }
        );
        assert_eq!(
            standard["h264_vulkan"],
            QualityConfig::built_in().presets["standard"].encoders["h264_vulkan"]
        );
    }

    #[test]
    fn invalid_configs_fail_to_load() {
        let malformed = QualityConfig::from_json("{").unwrap_err();
        assert!(malformed.contains("EOF while parsing"), "{malformed}");
        for (value, expected) in [
            (
                json!({ "presets": {}, "speed": 1 }),
                "unknown field `speed`",
            ),
            (
                json!({ "codecs": { "vp8": { "encoders": ["libx264"] } } }),
                "unknown variant `vp8`",
            ),
            (
                json!({ "codecs": { "vp9": { "encoders": ["libx264"] } } }),
                "`libx264` encodes h264, not vp9",
            ),
            (
                json!({ "presets": { "high": { "encoders": { "prores_ks": { "options": { "profile": "hq" } } } } } }),
                "always by lumen for `prores_ks`",
            ),
            (
                json!({ "codecs": { "h264": { "encoders": ["not_an_encoder"] } } }),
                "`not_an_encoder` is unavailable",
            ),
            (
                json!({ "codecs": { "hevc": { "encoders": ["libx264"] } } }),
                "`libx264` encodes h264, not hevc",
            ),
            (
                json!({ "presets": { "x": { "encoders": { "libx264": { "crf": 20, "qp": 20 } } } } }),
                "mutually exclusive",
            ),
            (
                json!({ "presets": { "x": { "encoders": { "libx264": { "preset": "p4" } } } } }),
                "invalid preset",
            ),
            (
                json!({ "presets": { "x": { "encoders": { "libx264": { "options": { "preset": "p4" } } } } } }),
                "invalid preset",
            ),
            (
                json!({ "codecs": { "h264": { "encoders": ["libx264rgb"] } } }),
                "YUV 4:2:0",
            ),
            (
                json!({ "presets": { "x": { "encoders": { "libx264": { "crf": 20, "options": { "crf": "18" } } } } } }),
                "options.crf is also set",
            ),
            (
                json!({ "presets": { "x": { "encoders": { "libx264": { "options": { "not_an_option": "1" } } } } } }),
                "no such option",
            ),
            (
                json!({ "presets": { "x": { "encoders": { "libx264": { "options": { "crf": "-5" } } } } } }),
                "out of range",
            ),
            (
                json!({ "default_preset": "archive" }),
                "default_preset `archive`",
            ),
            (
                json!({ "presets": { "high": { "bit_depth": 10 } } }),
                "built-in presets have no bit depth",
            ),
            (
                json!({ "presets": { "x": { "bit_depth": 12 } } }),
                "bit depth must be 8 or 10",
            ),
        ] {
            let error = config(value.clone()).unwrap_err();
            assert!(error.contains(expected), "{value}: {error}");
        }
    }

    #[test]
    fn a_missing_config_file_names_the_path() {
        let error = QualityConfig::load(Some(Path::new("/no/such/quality.json"))).unwrap_err();
        assert!(error.contains("/no/such/quality.json"), "{error}");
    }
}
