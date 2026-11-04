use crate::video::EncodeMode;
use crate::{FfmpegError, Result};

use super::codec::EncoderFamily;

const DEFAULT_BIT_RATE: u64 = 14_000_000;

/// How an encoder trades bitrate against quality.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateControl {
    /// Target average bitrate in bits per second.
    Bitrate(u64),
    /// Constant rate factor; lower is higher quality. Supported by libx264 and NVENC.
    Crf(u8),
    /// Constant quantizer for every frame. Supported by libx264, NVENC and Vulkan.
    Qp(u8),
}

impl Default for RateControl {
    fn default() -> Self {
        Self::Bitrate(DEFAULT_BIT_RATE)
    }
}

/// The quality settings a render asks its encoder for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EncoderQuality {
    pub rate_control: RateControl,
    /// Encoder preset: `ultrafast`..`placebo` for libx264, `p1`..`p7` for NVENC.
    pub preset: Option<String>,
}

const X264_PRESETS: [&str; 10] = [
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
const NVENC_PRESETS: [&str; 7] = ["p1", "p2", "p3", "p4", "p5", "p6", "p7"];

/// Settings an encoder needs for a [`RateControl`] and preset: context fields plus
/// encoder-private AVOptions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QualityOptions {
    pub bit_rate: i64,
    pub options: Vec<(&'static str, String)>,
}

pub(crate) fn quality_options(
    encoder: &str,
    rate_control: RateControl,
    preset: Option<&str>,
    mode: EncodeMode,
) -> Result<QualityOptions> {
    let family = EncoderFamily::of(encoder);
    let unsupported = |what: &str, supported: &str| {
        FfmpegError::new(
            "VideoEncoder::create",
            format!("encoder `{encoder}` does not support {what}; supported: {supported}"),
        )
    };

    let mut options = Vec::new();
    // NVIDIA's Vulkan H.264 encode can stall forever at a GOP boundary with more than one frame in
    // flight on more than one queue, with or without B-frames. Zero-copy encodes use the one
    // encode queue lumen lists on the shared device. CPU-upload encodes create their own device
    // with every queue, and `limit_queues` does nothing on FFmpeg 8.0.
    if encoder == "h264_vulkan" && mode == EncodeMode::CpuUpload {
        options.push(("async_depth", "1".to_string()));
    }
    if let Some(preset) = preset {
        let presets: &[&str] = match family {
            EncoderFamily::X264 => &X264_PRESETS,
            EncoderFamily::Nvenc => &NVENC_PRESETS,
            _ => return Err(unsupported("encoder presets", "libx264, NVENC encoders")),
        };
        if !presets.contains(&preset) {
            return Err(FfmpegError::new(
                "VideoEncoder::create",
                format!(
                    "invalid preset `{preset}` for encoder `{encoder}`; expected one of: {}",
                    presets.join(", ")
                ),
            ));
        }
        options.push(("preset", preset.to_string()));
    }

    let bit_rate = match (rate_control, family) {
        (RateControl::Bitrate(bits_per_second), _) => i64::try_from(bits_per_second)
            .map_err(|_| FfmpegError::new("VideoEncoder::create", "bitrate is too large"))?,
        (RateControl::Crf(crf), EncoderFamily::X264) => {
            options.push(("crf", crf.to_string()));
            0
        }
        (RateControl::Crf(0), EncoderFamily::Nvenc) => {
            let max = if encoder.starts_with("av1_") { 63 } else { 51 };
            return Err(FfmpegError::new(
                "VideoEncoder::create",
                format!("encoder `{encoder}` treats CRF 0 as automatic; use 1-{max}"),
            ));
        }
        (RateControl::Crf(cq), EncoderFamily::Nvenc) => {
            options.push(("rc", "vbr".to_string()));
            options.push(("cq", cq.to_string()));
            0
        }
        (RateControl::Crf(_), _) => {
            return Err(unsupported(
                "CRF rate control",
                "libx264, NVENC encoders (use a quantizer or bitrate instead)",
            ));
        }
        (RateControl::Qp(qp), EncoderFamily::X264) => {
            options.push(("qp", qp.to_string()));
            0
        }
        (RateControl::Qp(qp), EncoderFamily::Nvenc) => {
            options.push(("rc", "constqp".to_string()));
            options.push(("qp", qp.to_string()));
            0
        }
        (RateControl::Qp(qp), EncoderFamily::Vulkan) => {
            options.push(("rc_mode", "cqp".to_string()));
            options.push(("qp", qp.to_string()));
            0
        }
        (RateControl::Qp(_), EncoderFamily::VideoToolbox | EncoderFamily::Other) => {
            return Err(unsupported(
                "constant quantizer rate control",
                "libx264, NVENC and Vulkan encoders",
            ));
        }
    };

    Ok(QualityOptions { bit_rate, options })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::GpuBackend;

    fn options(
        encoder: &str,
        rate_control: RateControl,
        preset: Option<&str>,
    ) -> Result<QualityOptions> {
        quality_options(encoder, rate_control, preset, EncodeMode::CpuUpload)
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
                "h264_nvenc",
                RateControl::Crf(28),
                Some("p5"),
                &[("preset", "p5"), ("rc", "vbr"), ("cq", "28")],
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
                .options
                .iter()
                .map(|(name, value)| (*name, value.as_str()))
                .collect();
            assert_eq!(actual, expected, "{encoder}");
        }
    }

    #[test]
    fn h264_vulkan_limits_frames_in_flight_only_when_it_owns_its_device() {
        let async_depth = |mode| {
            let quality =
                quality_options("h264_vulkan", RateControl::Bitrate(1), None, mode).unwrap();
            quality.options.contains(&("async_depth", "1".to_string()))
        };
        assert!(async_depth(EncodeMode::CpuUpload));
        assert!(!async_depth(EncodeMode::GpuTexture(GpuBackend::Vulkan)));
    }

    #[test]
    fn zero_quality_that_means_automatic_errors() {
        assert!(options("h264_nvenc", RateControl::Crf(0), None).is_err());
        assert!(options("libx264", RateControl::Crf(0), None).is_ok());
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
}
