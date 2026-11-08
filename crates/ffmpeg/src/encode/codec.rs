use std::ffi::CString;

use crate::ffi::sys;
use crate::gpu::GpuBackend;
use crate::video::VideoCodec;
use crate::{FfmpegError, Result};

use super::audio::AudioEncoderConfig;

/// A video encoder this FFmpeg build has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncoderInfo {
    pub codec: VideoCodec,
    pub family: EncoderFamily,
}

/// The encoders lumen drives differently from the rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncoderFamily {
    X264,
    X265,
    SvtAv1,
    Nvenc,
    Vulkan,
    VideoToolbox,
    Other,
}

impl EncoderFamily {
    /// The family of an encoder by its FFmpeg name.
    pub fn of(encoder: &str) -> Self {
        if encoder == "libx264" {
            Self::X264
        } else if encoder == "libx265" {
            Self::X265
        } else if encoder == "libsvtav1" {
            Self::SvtAv1
        } else if encoder.ends_with("_nvenc") {
            Self::Nvenc
        } else if encoder.ends_with("_vulkan") {
            Self::Vulkan
        } else if encoder.ends_with("_videotoolbox") {
            Self::VideoToolbox
        } else {
            Self::Other
        }
    }

    pub(crate) fn takes_textures_from(self, backend: GpuBackend) -> bool {
        matches!(
            (self, backend),
            (Self::Nvenc, GpuBackend::Cuda)
                | (Self::Vulkan, GpuBackend::Vulkan)
                | (Self::VideoToolbox, GpuBackend::Metal)
        )
    }
}

/// Looks up a video encoder by its FFmpeg name, such as `libx264` or `h264_vulkan`.
pub fn encoder_info(name: &str) -> Result<EncoderInfo> {
    let codec = video_encoder_by_name(name)?;
    Ok(EncoderInfo {
        codec: VideoCodec::from_av_codec_id(unsafe { (*codec).id }),
        family: EncoderFamily::of(name),
    })
}

pub(crate) fn video_encoder_by_name(name: &str) -> Result<*const sys::AVCodec> {
    // These take only VAAPI frames, and lumen has no way to fill those.
    if name.ends_with("_vaapi") {
        return Err(FfmpegError::new(
            "encoder_info",
            format!("VAAPI encoder `{name}` is not supported; use a Vulkan encoder or libx264"),
        ));
    }
    let codec = encoder_by_name(name)?;
    if unsafe { (*codec).type_ } != sys::AVMediaType::AVMEDIA_TYPE_VIDEO {
        return Err(FfmpegError::new(
            "encoder_info",
            format!("`{name}` is not a video encoder"),
        ));
    }
    Ok(codec)
}

pub(crate) fn encoder_by_name(name: &str) -> Result<*const sys::AVCodec> {
    let c_name = CString::new(name).map_err(|_| {
        FfmpegError::new(
            "avcodec_find_encoder_by_name",
            "encoder name contains NUL byte",
        )
    })?;
    let codec = unsafe { sys::avcodec_find_encoder_by_name(c_name.as_ptr()) };
    if codec.is_null() {
        Err(FfmpegError::new(
            "avcodec_find_encoder_by_name",
            missing_encoder_message(name, unsafe { sys::avcodec_version() } >> 16),
        ))
    } else {
        Ok(codec)
    }
}

/// libavcodec 62 is FFmpeg 8.0, the first release with `av1_vulkan`.
fn missing_encoder_message(name: &str, avcodec_major: u32) -> String {
    if name == "av1_vulkan" && avcodec_major < 62 {
        format!(
            "`av1_vulkan` needs FFmpeg 8.0 or newer, but this FFmpeg has libavcodec {avcodec_major}"
        )
    } else {
        format!("requested encoder `{name}` is unavailable")
    }
}

pub(crate) fn find_audio_encoder(config: &AudioEncoderConfig) -> Result<*const sys::AVCodec> {
    if let Some(name) = config.encoder_name.as_deref() {
        return encoder_by_name(name);
    }
    let codec = unsafe { sys::avcodec_find_encoder(sys::AVCodecID::AV_CODEC_ID_AAC) };
    if codec.is_null() {
        Err(FfmpegError::new(
            "avcodec_find_encoder",
            "requested AAC encoder is unavailable",
        ))
    } else {
        Ok(codec)
    }
}

pub(crate) fn select_audio_sample_format(
    codec: *const sys::AVCodec,
) -> Result<sys::AVSampleFormat> {
    use sys::AVSampleFormat::{AV_SAMPLE_FMT_FLT, AV_SAMPLE_FMT_FLTP, AV_SAMPLE_FMT_NONE};

    unsafe {
        let formats = (*codec).sample_fmts;
        if formats.is_null() {
            return Ok(AV_SAMPLE_FMT_FLTP);
        }
        let mut index = 0;
        let mut fallback = AV_SAMPLE_FMT_NONE;
        loop {
            let format = *formats.add(index);
            if format == AV_SAMPLE_FMT_NONE {
                break;
            }
            if format == AV_SAMPLE_FMT_FLTP {
                return Ok(format);
            }
            if format == AV_SAMPLE_FMT_FLT {
                fallback = format;
            }
            index += 1;
        }
        if fallback != AV_SAMPLE_FMT_NONE {
            Ok(fallback)
        } else {
            Err(FfmpegError::new(
                "AudioEncoder::create",
                "AAC encoder does not support f32 input",
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoder_names_resolve_to_the_codec_ffmpeg_registered_them_under() {
        for (name, codec) in [
            ("libx264", VideoCodec::H264),
            ("libx265", VideoCodec::Hevc),
            ("libsvtav1", VideoCodec::Av1),
            ("libaom-av1", VideoCodec::Av1),
            ("h264_nvenc", VideoCodec::H264),
            ("hevc_nvenc", VideoCodec::Hevc),
            ("av1_nvenc", VideoCodec::Av1),
            ("h264_vulkan", VideoCodec::H264),
            ("hevc_vulkan", VideoCodec::Hevc),
            ("av1_vulkan", VideoCodec::Av1),
            ("h264_videotoolbox", VideoCodec::H264),
            ("hevc_videotoolbox", VideoCodec::Hevc),
        ] {
            // A build only registers some of these.
            if let Ok(info) = encoder_info(name) {
                assert_eq!(info.codec, codec, "{name}");
            }
        }
        assert_eq!(encoder_info("libx264").unwrap().codec, VideoCodec::H264);
    }

    #[test]
    fn names_that_cannot_encode_video_are_refused() {
        for (name, expected) in [
            ("definitely_not_an_encoder", "unavailable"),
            ("h264", "unavailable"),
            ("h265", "unavailable"),
            ("aac", "not a video encoder"),
            ("h264_vaapi", "VAAPI"),
        ] {
            let error = encoder_info(name).unwrap_err();
            assert!(error.message.contains(expected), "{name}: {error}");
        }
    }

    #[test]
    fn av1_vulkan_below_ffmpeg_8_says_so() {
        assert!(missing_encoder_message("av1_vulkan", 61).contains("FFmpeg 8.0"));
        // FFmpeg 8.0 or newer without the encoder is a build problem, not a version one.
        assert!(!missing_encoder_message("av1_vulkan", 62).contains("FFmpeg 8.0"));
        assert!(!missing_encoder_message("h264_vulkan", 61).contains("FFmpeg 8.0"));
    }
}
