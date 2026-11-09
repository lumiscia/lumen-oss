use crate::ffi::{self, CodecContextGuard, sys};
use crate::video::EncodeMode;
use crate::{FfmpegError, Result};

use super::codec::{EncoderFamily, video_encoder_by_name};

/// The bitrate `--preset` alone targets with lumen-local's rate-control flags.
const FLAG_DEFAULT_BIT_RATE: u64 = 14_000_000;

/// How an encoder trades bitrate against quality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RateControl {
    /// Target average bitrate in bits per second.
    Bitrate(u64),
    /// Constant rate factor; lower is higher quality. Supported by libx264, libx265, libsvtav1 and
    /// NVENC.
    Crf(u8),
    /// Constant quantizer for every frame. Supported by libx264, libx265, libsvtav1, NVENC and
    /// Vulkan; `av1_vulkan` takes an AV1 qindex, 0-255.
    Qp(u8),
}

/// The settings one encoder runs with. `Default` leaves every setting at the encoder's default.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct EncoderQuality {
    pub rate_control: Option<RateControl>,
    /// Encoder preset: `ultrafast`..`placebo` for libx264 and libx265, `p1`..`p7` for NVENC,
    /// `0`..`13` for libsvtav1.
    pub preset: Option<String>,
    /// FFmpeg AVOptions set after `rate_control` and `preset`, on the codec context or the
    /// encoder's private options.
    pub options: Vec<(String, String)>,
}

impl EncoderQuality {
    /// The quality lumen-local's flags ask for: at most one of `bitrate`, `crf` and `qp`, plus a
    /// preset, or `None` if they ask for none of them. A preset alone targets 14 Mb/s. Errors when
    /// more than one rate control is given.
    pub fn from_options(
        bitrate: Option<u64>,
        crf: Option<u8>,
        qp: Option<u8>,
        preset: Option<String>,
    ) -> std::result::Result<Option<Self>, String> {
        let rate_control = one_rate_control(bitrate, crf, qp)?;
        Ok((rate_control.is_some() || preset.is_some()).then(|| Self {
            rate_control: Some(rate_control.unwrap_or(RateControl::Bitrate(FLAG_DEFAULT_BIT_RATE))),
            preset,
            options: Vec::new(),
        }))
    }

    /// Whether `encoder` supports this rate control and preset, without opening it. Errors with
    /// the message opening it would give.
    pub fn check_encoder(&self, encoder: &str) -> std::result::Result<(), String> {
        portable_options(encoder, self)
            .map(|_| ())
            .map_err(|error| error.message)
    }
}

pub(crate) fn one_rate_control(
    bitrate: Option<u64>,
    crf: Option<u8>,
    qp: Option<u8>,
) -> std::result::Result<Option<RateControl>, String> {
    let mut requested = [
        bitrate.map(RateControl::Bitrate),
        crf.map(RateControl::Crf),
        qp.map(RateControl::Qp),
    ]
    .into_iter()
    .flatten();
    let rate_control = requested.next();
    if requested.next().is_some() {
        return Err("bitrate, crf and qp are mutually exclusive; set only one".to_string());
    }
    Ok(rate_control)
}

/// Parses bits per second, with optional `k` or `M` suffixes (`14M` is 14,000,000).
pub fn parse_bitrate(value: &str) -> std::result::Result<u64, String> {
    let (digits, multiplier) = match value.strip_suffix(['k', 'K']) {
        Some(digits) => (digits, 1_000),
        None => match value.strip_suffix(['m', 'M']) {
            Some(digits) => (digits, 1_000_000),
            None => (value, 1),
        },
    };
    let bitrate = digits
        .parse::<u64>()
        .map_err(|error| format!("invalid bitrate `{value}`: {error}"))?
        .checked_mul(multiplier)
        .ok_or("bitrate is too large")?;
    if bitrate == 0 {
        return Err("bitrate must be greater than zero".to_string());
    }
    Ok(bitrate)
}

const X26X_PRESETS: [&str; 10] = [
    "ultrafast",
    "superfast",
    "veryfast",
    "faster",
    "fast",
    "medium",
    "slow",
    "slower",
    "veryslow",
    "placebo",
];
const HEVC_NVENC_CQ_MAX_RATE: u64 = 100_000_000;
const NVENC_PRESETS: [&str; 7] = ["p1", "p2", "p3", "p4", "p5", "p6", "p7"];
const SVT_AV1_PRESETS: [&str; 14] = [
    "0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13",
];

/// Settings an encoder needs for an [`EncoderQuality`]: the context bitrate, the private
/// AVOptions `rate_control` and `preset` map to, then the raw `options`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QualityOptions {
    pub bit_rate: i64,
    pub private: Vec<(&'static str, String)>,
    pub options: Vec<(String, String)>,
}

pub(crate) fn quality_options(
    encoder: &str,
    quality: &EncoderQuality,
    mode: EncodeMode,
) -> Result<QualityOptions> {
    let (bit_rate, mut private) = portable_options(encoder, quality)?;
    // NVIDIA's Vulkan H.264 encode can stall forever at a GOP boundary with more than one frame in
    // flight on more than one queue, with or without B-frames. Zero-copy encodes use the one
    // encode queue lumen lists on the shared device. CPU-upload encodes create their own device
    // with every queue, and `limit_queues` does nothing on FFmpeg 8.0.
    if encoder == "h264_vulkan" && mode == EncodeMode::CpuUpload {
        private.push(("async_depth", "1".to_string()));
    }
    Ok(QualityOptions {
        bit_rate,
        private,
        options: quality.options.clone(),
    })
}

/// The context bitrate and private AVOptions `rate_control` and `preset` map to on `encoder`.
/// A quantizer or rate factor sets the bitrate to 0, so the encoder doesn't also target one.
pub(crate) fn portable_options(
    encoder: &str,
    quality: &EncoderQuality,
) -> Result<(i64, Vec<(&'static str, String)>)> {
    let family = EncoderFamily::of(encoder);
    let error = |message: String| FfmpegError::new("VideoEncoder::create", message);
    let unsupported = |what: &str, supported: &str| {
        error(format!(
            "encoder `{encoder}` does not support {what}; supported: {supported}"
        ))
    };
    let check_range = |what: &str, value: u8, min: u8, max: u8| {
        if (min..=max).contains(&value) {
            Ok(())
        } else {
            Err(error(format!(
                "invalid {what} {value} for encoder `{encoder}`; use {min}-{max}"
            )))
        }
    };

    let mut options = Vec::new();
    if let Some(preset) = quality.preset.as_deref() {
        let presets: &[&str] = match family {
            EncoderFamily::X264 | EncoderFamily::X265 => &X26X_PRESETS,
            EncoderFamily::Nvenc => &NVENC_PRESETS,
            EncoderFamily::SvtAv1 => &SVT_AV1_PRESETS,
            _ => {
                return Err(unsupported(
                    "encoder presets",
                    "libx264, libx265, libsvtav1, NVENC encoders",
                ));
            }
        };
        if !presets.contains(&preset) {
            return Err(error(format!(
                "invalid preset `{preset}` for encoder `{encoder}`; expected one of: {}",
                presets.join(", ")
            )));
        }
        options.push(("preset", preset.to_string()));
    }

    let bit_rate = match (quality.rate_control, family) {
        (None, _) => 0,
        (Some(RateControl::Bitrate(bits_per_second)), _) => {
            i64::try_from(bits_per_second).map_err(|_| error("bitrate is too large".to_string()))?
        }
        (Some(RateControl::Crf(crf)), EncoderFamily::X264) => {
            options.push(("crf", crf.to_string()));
            0
        }
        (Some(RateControl::Crf(crf)), EncoderFamily::X265) => {
            check_range("CRF", crf, 0, 51)?;
            options.push(("crf", crf.to_string()));
            0
        }
        (Some(RateControl::Crf(crf)), EncoderFamily::SvtAv1) => {
            check_range("CRF", crf, 0, 63)?;
            options.push(("crf", crf.to_string()));
            0
        }
        (Some(RateControl::Crf(cq)), EncoderFamily::Nvenc) => {
            let max = if encoder.starts_with("av1_") { 63 } else { 51 };
            if cq == 0 || cq > max {
                return Err(error(format!(
                    "invalid CRF {cq} for encoder `{encoder}` (0 means automatic); use 1-{max}"
                )));
            }
            options.push(("rc", "vbr".to_string()));
            options.push(("cq", cq.to_string()));
            // Without a maximum, HEVC NVENC caps VBR near 10 Mb/s at 1080p whatever `cq` asks
            // for. NVENC signals the level from `maxrate`; 100 Mb/s gives level 5.0 at 1080p.
            if encoder.starts_with("hevc_") {
                options.push(("maxrate", HEVC_NVENC_CQ_MAX_RATE.to_string()));
            }
            0
        }
        (Some(RateControl::Crf(_)), _) => {
            return Err(unsupported(
                "CRF rate control",
                "libx264, libx265, libsvtav1, NVENC encoders (use a quantizer or bitrate instead)",
            ));
        }
        (Some(RateControl::Qp(qp)), EncoderFamily::X264) => {
            options.push(("qp", qp.to_string()));
            0
        }
        (Some(RateControl::Qp(qp)), EncoderFamily::X265) => {
            check_range("QP", qp, 0, 51)?;
            options.push(("qp", qp.to_string()));
            0
        }
        (Some(RateControl::Qp(qp)), EncoderFamily::SvtAv1) => {
            check_range("QP", qp, 0, 63)?;
            options.push(("qp", qp.to_string()));
            0
        }
        (Some(RateControl::Qp(qp)), EncoderFamily::Nvenc) => {
            if !encoder.starts_with("av1_") {
                check_range("QP", qp, 0, 51)?;
            }
            options.push(("rc", "constqp".to_string()));
            options.push(("qp", qp.to_string()));
            0
        }
        (Some(RateControl::Qp(qp)), EncoderFamily::Vulkan) => {
            options.push(("rc_mode", "cqp".to_string()));
            options.push(("qp", qp.to_string()));
            0
        }
        (Some(RateControl::Qp(_)), EncoderFamily::VideoToolbox | EncoderFamily::Other) => {
            return Err(unsupported(
                "constant quantizer rate control",
                "libx264, libx265, libsvtav1, NVENC and Vulkan encoders",
            ));
        }
    };

    Ok((bit_rate, options))
}

/// Sets the private AVOptions, then the raw ones, which may name a codec context option such as
/// `g` or an option of the encoder.
pub(crate) fn apply_quality_options(
    context: *mut sys::AVCodecContext,
    quality: &QualityOptions,
    encoder: &str,
) -> Result<()> {
    for (name, value) in &quality.private {
        set_option(unsafe { (*context).priv_data }, 0, name, value, encoder)?;
    }
    for (name, value) in &quality.options {
        set_option(
            context.cast(),
            sys::AV_OPT_SEARCH_CHILDREN,
            name,
            value,
            encoder,
        )?;
    }
    Ok(())
}

fn set_option(
    object: *mut libc::c_void,
    flags: i32,
    name: &str,
    value: &str,
    encoder: &str,
) -> Result<()> {
    let c_name = ffi::cstring("av_opt_set", name)?;
    let c_value = ffi::cstring("av_opt_set", value)?;
    let code = unsafe { sys::av_opt_set(object, c_name.as_ptr(), c_value.as_ptr(), flags) };
    ffi::check(code, "av_opt_set").map_err(|mut error| {
        let reason = if code == sys::AVERROR_OPTION_NOT_FOUND {
            "no such option".to_string()
        } else if code == sys::AVERROR(libc::ERANGE) {
            "value out of range".to_string()
        } else {
            error.message.clone()
        };
        error.message = format!("failed to set `{name}={value}` on encoder `{encoder}`: {reason}");
        error
    })
}

/// Checks raw `options` against `encoder`'s AVOptions on a context that is never opened, so a
/// bad key, an unparsable value or one outside the option's range errors without any hardware.
pub(crate) fn check_raw_options(encoder: &str, options: &[(String, String)]) -> Result<()> {
    let codec = video_encoder_by_name(encoder)?;
    let context = CodecContextGuard(unsafe { sys::avcodec_alloc_context3(codec) });
    if context.0.is_null() {
        return Err(FfmpegError::new(
            "avcodec_alloc_context3",
            "failed to allocate encoder context",
        ));
    }
    for (name, value) in options {
        set_option(
            context.0.cast(),
            sys::AV_OPT_SEARCH_CHILDREN,
            name,
            value,
            encoder,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::GpuBackend;

    fn quality(rate_control: Option<RateControl>, preset: Option<&str>) -> EncoderQuality {
        EncoderQuality {
            rate_control,
            preset: preset.map(str::to_string),
            options: Vec::new(),
        }
    }

    fn options(
        encoder: &str,
        rate_control: RateControl,
        preset: Option<&str>,
    ) -> Result<QualityOptions> {
        quality_options(
            encoder,
            &quality(Some(rate_control), preset),
            EncodeMode::CpuUpload,
        )
    }

    #[test]
    fn options_allow_one_rate_control() {
        let flags = |bitrate, crf, qp, preset: Option<&str>| {
            EncoderQuality::from_options(bitrate, crf, qp, preset.map(str::to_string))
        };
        assert_eq!(flags(None, None, None, None), Ok(None));
        assert_eq!(
            flags(None, Some(23), None, Some("fast")),
            Ok(Some(quality(Some(RateControl::Crf(23)), Some("fast"))))
        );
        assert_eq!(
            flags(None, None, None, Some("fast"))
                .unwrap()
                .unwrap()
                .rate_control,
            Some(RateControl::Bitrate(14_000_000))
        );
        assert!(flags(None, Some(23), Some(20), None).is_err());
        assert!(flags(Some(1), Some(23), None, None).is_err());
        assert!(flags(Some(1), None, Some(20), None).is_err());
    }

    #[test]
    fn parse_bitrate_accepts_suffixes() {
        assert_eq!(parse_bitrate("14000000"), Ok(14_000_000));
        assert_eq!(parse_bitrate("500k"), Ok(500_000));
        assert_eq!(parse_bitrate("14M"), Ok(14_000_000));
        assert!(parse_bitrate("0").is_err());
        assert!(parse_bitrate("fast").is_err());
        assert!(parse_bitrate("18446744073709551615M").is_err());
    }

    #[test]
    fn rate_control_maps_to_each_encoders_options() {
        for (encoder, rate_control, preset, expected) in [
            (
                "libx264",
                RateControl::Crf(23),
                Some("fast"),
                &[("preset", "fast"), ("crf", "23")][..],
            ),
            ("libx264", RateControl::Qp(20), None, &[("qp", "20")]),
            (
                "libx265",
                RateControl::Crf(24),
                Some("slow"),
                &[("preset", "slow"), ("crf", "24")],
            ),
            (
                "libsvtav1",
                RateControl::Crf(32),
                Some("8"),
                &[("preset", "8"), ("crf", "32")],
            ),
            (
                "h264_nvenc",
                RateControl::Crf(28),
                Some("p5"),
                &[("preset", "p5"), ("rc", "vbr"), ("cq", "28")],
            ),
            (
                "hevc_nvenc",
                RateControl::Crf(24),
                Some("p5"),
                &[
                    ("preset", "p5"),
                    ("rc", "vbr"),
                    ("cq", "24"),
                    ("maxrate", "100000000"),
                ],
            ),
            (
                "av1_nvenc",
                RateControl::Qp(30),
                None,
                &[("rc", "constqp"), ("qp", "30")],
            ),
            (
                "av1_vulkan",
                RateControl::Qp(100),
                None,
                &[("rc_mode", "cqp"), ("qp", "100")],
            ),
        ] {
            let quality = options(encoder, rate_control, preset).unwrap();
            assert_eq!(quality.bit_rate, 0, "{encoder}");
            let actual: Vec<_> = quality
                .private
                .iter()
                .map(|(name, value)| (*name, value.as_str()))
                .collect();
            assert_eq!(actual, expected, "{encoder}");
        }
    }

    #[test]
    fn h264_vulkan_limits_frames_in_flight_only_when_it_owns_its_device() {
        let async_depth = |mode| {
            let quality = quality_options("h264_vulkan", &EncoderQuality::default(), mode).unwrap();
            quality.private.contains(&("async_depth", "1".to_string()))
        };
        assert!(async_depth(EncodeMode::CpuUpload));
        assert!(!async_depth(EncodeMode::GpuTexture(GpuBackend::Vulkan)));
    }

    #[test]
    fn quality_outside_an_encoders_range_errors() {
        assert!(options("h264_nvenc", RateControl::Crf(0), None).is_err());
        assert!(options("libx264", RateControl::Crf(0), None).is_ok());
        assert!(options("hevc_nvenc", RateControl::Crf(52), None).is_err());
        assert!(options("av1_nvenc", RateControl::Crf(63), None).is_ok());
        assert!(options("av1_nvenc", RateControl::Crf(64), None).is_err());
        assert!(options("h264_nvenc", RateControl::Qp(52), None).is_err());
        assert!(options("av1_nvenc", RateControl::Qp(255), None).is_ok());
        assert!(options("libx265", RateControl::Crf(52), None).is_err());
        assert!(options("libsvtav1", RateControl::Crf(0), None).is_ok());
        assert!(options("libsvtav1", RateControl::Crf(64), None).is_err());
        assert!(options("libsvtav1", RateControl::Qp(64), None).is_err());
    }

    #[test]
    fn check_encoder_gives_the_error_opening_would() {
        let crf = quality(Some(RateControl::Crf(20)), None);
        assert_eq!(crf.check_encoder("libx264"), Ok(()));
        assert_eq!(
            crf.check_encoder("h264_vulkan"),
            Err(quality_options("h264_vulkan", &crf, EncodeMode::CpuUpload)
                .unwrap_err()
                .message)
        );
    }

    #[test]
    fn unsupported_combinations_error() {
        for (encoder, rate_control, preset, expected) in [
            ("av1_vulkan", RateControl::Crf(23), None, "CRF"),
            ("h264_videotoolbox", RateControl::Qp(23), None, "quantizer"),
            (
                "libx264",
                RateControl::Bitrate(1),
                Some("p4"),
                "invalid preset",
            ),
            (
                "h264_nvenc",
                RateControl::Bitrate(1),
                Some("fast"),
                "invalid preset",
            ),
            (
                "libsvtav1",
                RateControl::Bitrate(1),
                Some("slow"),
                "invalid preset",
            ),
            (
                "av1_vulkan",
                RateControl::Bitrate(1),
                Some("fast"),
                "presets",
            ),
        ] {
            let error = options(encoder, rate_control, preset).unwrap_err();
            assert!(
                error.message.contains(expected) && error.message.contains(encoder),
                "{encoder} {rate_control:?} {preset:?}: {}",
                error.message
            );
        }
    }

    #[test]
    fn raw_options_are_checked_without_opening_the_encoder() {
        let check = |name: &str, value: &str| {
            check_raw_options("libx264", &[(name.to_string(), value.to_string())])
        };
        assert_eq!(check("tune", "film"), Ok(()));
        assert_eq!(check("g", "48"), Ok(()));
        let unknown = check("not_an_option", "1").unwrap_err();
        assert!(unknown.message.contains("no such option"), "{unknown}");
        let out_of_range = check("crf", "-5").unwrap_err();
        assert!(
            out_of_range.message.contains("out of range"),
            "{out_of_range}"
        );
        assert!(check("g", "many").is_err());
    }
}
