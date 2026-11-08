use std::{collections::BTreeMap, env, fmt, path::Path, str::FromStr};

use serde::Deserialize;

use super::choice::{EncoderCandidate, VideoEncoderChoice};
use super::codec::encoder_info;
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
}

impl EncodeCodec {
    pub const ALL: [Self; 3] = [Self::H264, Self::Hevc, Self::Av1];

    pub fn name(self) -> &'static str {
        match self {
            Self::H264 => "h264",
            Self::Hevc => "hevc",
            Self::Av1 => "av1",
        }
    }

    pub fn of(codec: VideoCodec) -> Option<Self> {
        match codec {
            VideoCodec::H264 => Some(Self::H264),
            VideoCodec::Hevc => Some(Self::Hevc),
            VideoCodec::Av1 => Some(Self::Av1),
            _ => None,
        }
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
            .ok_or_else(|| format!("unknown codec `{name}`; expected h264, hevc or av1"))
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

/// Every built-in preset, by encoder. `crf` is `cq` on NVENC. `lossless` lists only encoders
/// that are truly lossless, so a codec's candidates skip to one of those.
const BUILT_IN: [(&str, &[(&str, BuiltIn)]); 4] = [
    (
        "draft",
        &[
            ("libx264", set(Crf(28), Some("veryfast"))),
            ("h264_nvenc", set(Crf(28), Some("p2"))),
            ("h264_vulkan", set(Qp(30), None)),
            ("h264_videotoolbox", set(Bitrate(5_000_000), None)),
            ("libx265", set(Crf(30), Some("fast"))),
            ("hevc_nvenc", set(Crf(30), Some("p2"))),
            ("hevc_vulkan", set(Qp(32), None)),
            ("hevc_videotoolbox", set(Bitrate(3_000_000), None)),
            ("libsvtav1", set(Crf(40), Some("12"))),
            ("av1_nvenc", set(Crf(38), Some("p2"))),
            ("av1_vulkan", set(Qp(170), None)),
        ],
    ),
    (
        "standard",
        &[
            ("libx264", set(Crf(23), None)),
            ("h264_nvenc", set(Crf(23), Some("p5"))),
            ("h264_vulkan", set(Qp(24), None)),
            ("h264_videotoolbox", set(Bitrate(12_000_000), None)),
            ("libx265", set(Crf(25), None)),
            ("hevc_nvenc", set(Crf(25), Some("p5"))),
            ("hevc_vulkan", set(Qp(26), None)),
            ("hevc_videotoolbox", set(Bitrate(8_000_000), None)),
            ("libsvtav1", set(Crf(32), Some("8"))),
            ("av1_nvenc", set(Crf(32), Some("p5"))),
            ("av1_vulkan", set(Qp(130), None)),
        ],
    ),
    (
        "high",
        &[
            ("libx264", set(Crf(18), Some("slow"))),
            ("h264_nvenc", set(Crf(19), Some("p6"))),
            ("h264_vulkan", set(Qp(20), None)),
            ("h264_videotoolbox", set(Bitrate(20_000_000), None)),
            ("libx265", set(Crf(20), None)),
            ("hevc_nvenc", set(Crf(21), Some("p6"))),
            ("hevc_vulkan", set(Qp(22), None)),
            ("hevc_videotoolbox", set(Bitrate(14_000_000), None)),
            ("libsvtav1", set(Crf(26), Some("6"))),
            ("av1_nvenc", set(Crf(28), Some("p6"))),
            ("av1_vulkan", set(Qp(100), None)),
        ],
    ),
    (
        "lossless",
        &[
            ("libx264", set(Qp(0), None)),
            ("h264_nvenc", raw(&[("tune", "lossless")])),
            ("libx265", raw(&[("x265-params", "lossless=1")])),
            ("hevc_nvenc", raw(&[("tune", "lossless")])),
        ],
    ),
];

#[derive(Debug, Clone, PartialEq, Eq)]
struct Preset {
    /// The codec used when neither the request nor the composition names one.
    codec: Option<EncodeCodec>,
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
    /// Settings every candidate uses in place of a preset's, from lumen-local's rate-control flags.
    pub settings: Option<EncoderQuality>,
    /// The one encoder to use, with no fallback. `LUMEN_VIDEO_ENCODER` when `None`.
    pub encoder: Option<String>,
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
                    built_in.encoders.extend(encoders);
                }
                None => {
                    config.presets.insert(
                        name,
                        Preset {
                            codec: preset.codec,
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

    /// The encoders a render tries for `request`, in order, each with its preset's settings.
    /// Errors when the preset isn't defined, when no encoder of the codec has an entry in it or is
    /// in this FFmpeg build, or when the pinned encoder isn't in the build or doesn't fit the
    /// request's codec or the preset.
    pub fn resolve(&self, request: &QualityRequest) -> Result<VideoEncoderChoice, String> {
        self.resolve_with(
            request,
            env::var(ENCODER_ENV).ok().as_deref(),
            encoder_codec,
        )
    }

    fn resolve_with(
        &self,
        request: &QualityRequest,
        from_env: Option<&str>,
        codec_of: impl Fn(&str) -> Result<EncodeCodec, String>,
    ) -> Result<VideoEncoderChoice, String> {
        let pinned = [request.encoder.as_deref(), from_env]
            .into_iter()
            .flatten()
            .map(str::trim)
            .find(|name| !name.is_empty());
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
            return Ok(VideoEncoderChoice::single(encoder, quality));
        }

        let codec = request.codec.or(preset.codec).unwrap_or(self.default_codec);
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
        VideoEncoderChoice::new(candidates).ok_or_else(|| {
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
            "`{encoder}` encodes {:?}; presets cover h264, hevc and av1",
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
        let quality = EncoderQuality {
            rate_control: one_rate_control(bitrate, self.crf, self.qp)?,
            preset: self.preset,
            options: self.options.into_iter().collect(),
        };
        let (_, portable) = portable_options(encoder, &quality).map_err(|error| error.message)?;
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
                    "options.{key} is also set by this entry's crf, qp, bitrate or preset"
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
        } else {
            Ok(EncodeCodec::Av1)
        }
    }

    impl QualityConfig {
        fn resolve_with_env(
            &self,
            request: &QualityRequest,
            from_env: Option<&str>,
        ) -> Result<VideoEncoderChoice, String> {
            self.resolve_with(request, from_env, codec_by_name)
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
    fn a_codec_needs_one_candidate_in_this_build() {
        let config = QualityConfig::built_in();
        let hevc = request(None, Some(EncodeCodec::Hevc));
        let error = config
            .resolve_with(&hevc, None, |_| Err("not built".to_string()))
            .unwrap_err();
        assert!(error.contains("libx265"), "{error}");
        // A missing candidate before an available one is left to fall through at look-up.
        let only_software = |encoder: &str| match encoder {
            "libx265" => Ok(EncodeCodec::Hevc),
            _ => Err("not built".to_string()),
        };
        let choice = config.resolve_with(&hevc, None, only_software).unwrap();
        assert_eq!(encoders(&choice), EncodeCodec::Hevc.default_encoders());
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
                json!({ "codecs": { "vp9": { "encoders": ["libx264"] } } }),
                "unknown variant `vp9`",
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
