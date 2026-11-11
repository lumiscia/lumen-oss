use std::{ffi::CString, fmt, str::FromStr};

use serde::Deserialize;

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

/// Bits per sample of an encoded video stream.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(try_from = "u8")]
pub enum BitDepth {
    #[default]
    Eight,
    Ten,
}

impl BitDepth {
    pub fn bits(self) -> u8 {
        match self {
            Self::Eight => 8,
            Self::Ten => 10,
        }
    }
}

impl TryFrom<u8> for BitDepth {
    type Error = String;

    fn try_from(bits: u8) -> std::result::Result<Self, String> {
        match bits {
            8 => Ok(Self::Eight),
            10 => Ok(Self::Ten),
            _ => Err(format!("bit depth must be 8 or 10, got {bits}")),
        }
    }
}

impl FromStr for BitDepth {
    type Err = String;

    fn from_str(bits: &str) -> std::result::Result<Self, String> {
        bits.parse::<u8>()
            .map_err(|_| format!("bit depth must be 8 or 10, got `{bits}`"))?
            .try_into()
    }
}

impl fmt::Display for BitDepth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-bit", self.bits())
    }
}

/// Errors with why `encoder` can't write `depth`, without opening it. H.264 output is always
/// 8-bit, VideoToolbox has no 10-bit path, and a software encoder must take 10-bit YUV 4:2:0.
/// Vulkan and NVENC encoders find out whether their device can when they open.
pub fn check_bit_depth(encoder: &str, depth: BitDepth) -> std::result::Result<(), String> {
    if depth == BitDepth::Eight {
        return Ok(());
    }
    let codec = video_encoder_by_name(encoder).map_err(|error| error.message)?;
    if VideoCodec::from_av_codec_id(unsafe { (*codec).id }) == VideoCodec::H264 {
        return Err(format!(
            "H.264 output is always 8-bit, so `{encoder}` can't encode {depth}; use hevc or av1"
        ));
    }
    match EncoderFamily::of(encoder) {
        EncoderFamily::VideoToolbox => Err(format!(
            "{depth} is not supported by this encoder (`{encoder}`)"
        )),
        EncoderFamily::Vulkan | EncoderFamily::Nvenc => Ok(()),
        _ if pixel_formats(codec).is_none_or(|formats| {
            formats.contains(&sys::AVPixelFormat::AV_PIX_FMT_YUV420P10LE)
        }) =>
        {
            Ok(())
        }
        _ => Err(format!(
            "`{encoder}` doesn't take the {depth} YUV 4:2:0 frames lumen encodes from"
        )),
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
    // Vulkan encoders take frames from their own hardware frames context.
    if EncoderFamily::of(name) != EncoderFamily::Vulkan && !takes_yuv420p(codec) {
        return Err(FfmpegError::new(
            "encoder_info",
            format!("`{name}` doesn't take the YUV 4:2:0 frames lumen encodes from"),
        ));
    }
    Ok(codec)
}

/// Whether `codec` lists `yuv420p` among its input formats. One that lists none takes any.
fn takes_yuv420p(codec: *const sys::AVCodec) -> bool {
    pixel_formats(codec)
        .is_none_or(|formats| formats.contains(&sys::AVPixelFormat::AV_PIX_FMT_YUV420P))
}

#[cfg(ffmpeg_supported_config)]
fn pixel_formats(codec: *const sys::AVCodec) -> Option<&'static [sys::AVPixelFormat]> {
    use std::ffi::{c_int, c_void};

    let mut formats: *const c_void = std::ptr::null();
    let mut count: c_int = 0;
    let result = unsafe {
        sys::avcodec_get_supported_config(
            std::ptr::null(),
            codec,
            sys::AVCodecConfig::AV_CODEC_CONFIG_PIX_FORMAT,
            0,
            &mut formats,
            &mut count,
        )
    };
    if result < 0 || formats.is_null() {
        return None;
    }
    // The list is static data of the codec.
    Some(unsafe {
        std::slice::from_raw_parts(
            formats.cast::<sys::AVPixelFormat>(),
            usize::try_from(count).unwrap_or(0),
        )
    })
}

#[cfg(not(ffmpeg_supported_config))]
fn pixel_formats(codec: *const sys::AVCodec) -> Option<&'static [sys::AVPixelFormat]> {
    let formats = unsafe { (*codec).pix_fmts };
    if formats.is_null() {
        return None;
    }
    let mut count = 0;
    while unsafe { *formats.add(count) } != sys::AVPixelFormat::AV_PIX_FMT_NONE {
        count += 1;
    }
    // The list is static data of the codec, ended by `AV_PIX_FMT_NONE`.
    Some(unsafe { std::slice::from_raw_parts(formats, count) })
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
    fn ten_bit_needs_an_encoder_that_writes_it() {
        assert_eq!(check_bit_depth("libx264", BitDepth::Eight), Ok(()));
        let h264 = check_bit_depth("libx264", BitDepth::Ten).unwrap_err();
        assert!(h264.contains("always 8-bit"), "{h264}");
        for encoder in [
            "libx265",
            "libsvtav1",
            "hevc_vulkan",
            "av1_vulkan",
            "hevc_nvenc",
        ] {
            if encoder_info(encoder).is_ok() {
                assert_eq!(check_bit_depth(encoder, BitDepth::Ten), Ok(()), "{encoder}");
            }
        }
        if encoder_info("hevc_videotoolbox").is_ok() {
            let error = check_bit_depth("hevc_videotoolbox", BitDepth::Ten).unwrap_err();
            assert!(error.contains("not supported by this encoder"), "{error}");
        }
        assert_eq!("10".parse(), Ok(BitDepth::Ten));
        assert!("12".parse::<BitDepth>().is_err());
    }

    #[test]
    fn av1_vulkan_below_ffmpeg_8_says_so() {
        assert!(missing_encoder_message("av1_vulkan", 61).contains("FFmpeg 8.0"));
        // FFmpeg 8.0 or newer without the encoder is a build problem, not a version one.
        assert!(!missing_encoder_message("av1_vulkan", 62).contains("FFmpeg 8.0"));
        assert!(!missing_encoder_message("h264_vulkan", 61).contains("FFmpeg 8.0"));
    }
}
