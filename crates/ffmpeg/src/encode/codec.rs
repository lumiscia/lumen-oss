use std::{ffi::CString, fmt, str::FromStr};

use serde::Deserialize;

use crate::ffi::sys;
use crate::gpu::GpuBackend;
use crate::video::VideoCodec;
use crate::{FfmpegError, Result};

use super::audio::AudioEncoderConfig;
use super::preset::EncodeCodec;

/// A video encoder this FFmpeg build has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncoderInfo {
    pub codec: VideoCodec,
    pub family: EncoderFamily,
}

/// The encoders lumen drives differently from the rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EncoderFamily {
    X264,
    X265,
    SvtAv1,
    /// `libvpx-vp9`.
    Vpx,
    /// `prores_ks`.
    ProRes,
    Nvenc,
    Vulkan,
    VideoToolbox,
    /// Takes only VAAPI frames, which lumen has no way to fill.
    Vaapi,
    Other,
}

impl EncoderFamily {
    /// Whether the encoder runs on a GPU or media engine, which none can write alpha on.
    pub fn is_hardware(self) -> bool {
        matches!(self, Self::Nvenc | Self::Vulkan | Self::VideoToolbox)
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

/// An FFmpeg encoder name with the family and codec its spelling says. Every rule that depends
/// on which encoder it is reads those, so the name is only classified here.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Encoder {
    name: String,
    family: EncoderFamily,
    codec: Option<EncodeCodec>,
}

impl Encoder {
    /// Classifies `name`, such as `libx264` or `h264_vulkan`. A name no rule recognises is
    /// [`EncoderFamily::Other`] with no codec; whether the FFmpeg build has it is up to
    /// [`encoder_info`].
    pub fn parse(name: &str) -> Self {
        let family = match name {
            "libx264" => EncoderFamily::X264,
            "libx265" => EncoderFamily::X265,
            "libsvtav1" => EncoderFamily::SvtAv1,
            "libvpx-vp9" => EncoderFamily::Vpx,
            "prores_ks" => EncoderFamily::ProRes,
            _ if name.ends_with("_nvenc") => EncoderFamily::Nvenc,
            _ if name.ends_with("_vulkan") => EncoderFamily::Vulkan,
            _ if name.ends_with("_videotoolbox") => EncoderFamily::VideoToolbox,
            _ if name.ends_with("_vaapi") => EncoderFamily::Vaapi,
            _ => EncoderFamily::Other,
        };
        let codec = match (family, name) {
            (EncoderFamily::X264, _) => Some(EncodeCodec::H264),
            (EncoderFamily::X265, _) => Some(EncodeCodec::Hevc),
            (EncoderFamily::SvtAv1, _) => Some(EncodeCodec::Av1),
            (EncoderFamily::Vpx, _) => Some(EncodeCodec::Vp9),
            (EncoderFamily::ProRes, _) => Some(EncodeCodec::ProRes),
            (_, "libaom-av1") => Some(EncodeCodec::Av1),
            _ if name.starts_with("h264_") => Some(EncodeCodec::H264),
            _ if name.starts_with("hevc_") => Some(EncodeCodec::Hevc),
            _ if name.starts_with("av1_") => Some(EncodeCodec::Av1),
            _ if name.starts_with("vp9_") => Some(EncodeCodec::Vp9),
            _ if name.starts_with("prores") => Some(EncodeCodec::ProRes),
            _ => None,
        };
        Self {
            name: name.to_string(),
            family,
            codec,
        }
    }

    /// The FFmpeg name.
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn family(&self) -> EncoderFamily {
        self.family
    }

    /// The codec the name says the encoder writes, if it says. FFmpeg's registry has the final
    /// word, which [`encoder_info`] reads.
    pub fn codec(&self) -> Option<EncodeCodec> {
        self.codec
    }

    /// Whether this is `family` writing `codec`, such as `av1_vulkan`.
    pub(crate) fn is(&self, family: EncoderFamily, codec: EncodeCodec) -> bool {
        self.family == family && self.codec == Some(codec)
    }
}

impl From<&str> for Encoder {
    fn from(name: &str) -> Self {
        Self::parse(name)
    }
}

impl From<String> for Encoder {
    fn from(name: String) -> Self {
        Self::parse(&name)
    }
}

impl From<&Encoder> for Encoder {
    fn from(encoder: &Encoder) -> Self {
        encoder.clone()
    }
}

impl fmt::Display for Encoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name)
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
/// 8-bit, ProRes always 10-bit, VideoToolbox has no 10-bit path, and a software encoder must take
/// 10-bit YUV 4:2:0. Vulkan and NVENC encoders find out whether their device can when they open.
pub fn check_bit_depth(encoder: &Encoder, depth: BitDepth) -> std::result::Result<(), String> {
    let codec = match video_encoder_by_name(encoder) {
        Ok(codec) => codec,
        // An encoder this build lacks is skipped or refused where it is looked up.
        Err(_) if depth == BitDepth::Eight => return Ok(()),
        Err(error) => return Err(error.message),
    };
    let encodes = VideoCodec::from_av_codec_id(unsafe { (*codec).id });
    if encodes == VideoCodec::ProRes {
        return match depth {
            BitDepth::Ten => Ok(()),
            BitDepth::Eight => Err(format!(
                "ProRes 4444 is always 10-bit, so `{encoder}` can't encode {depth}"
            )),
        };
    }
    if depth == BitDepth::Eight {
        return Ok(());
    }
    if encodes == VideoCodec::H264 {
        return Err(format!(
            "H.264 output is always 8-bit, so `{encoder}` can't encode {depth}; use hevc or av1"
        ));
    }
    match encoder.family() {
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

/// Errors with why `encoder` can't write video with an alpha channel at `depth`, without opening
/// it. Only software encoders do: `prores_ks` as 10-bit 4:4:4 with alpha, and `libvpx-vp9` as
/// 8-bit 4:2:0 with alpha.
pub fn check_alpha(encoder: &Encoder, depth: BitDepth) -> std::result::Result<(), String> {
    if encoder.family().is_hardware() {
        return Err(format!(
            "`{encoder}` is a hardware encoder, and none can write alpha; use prores_ks or libvpx-vp9"
        ));
    }
    let codec = video_encoder_by_name(encoder).map_err(|error| error.message)?;
    let encodes = VideoCodec::from_av_codec_id(unsafe { (*codec).id });
    let format = match encodes {
        VideoCodec::ProRes => sys::AVPixelFormat::AV_PIX_FMT_YUVA444P10LE,
        VideoCodec::Vp9 if depth == BitDepth::Ten => {
            return Err(format!(
                "VP9 with alpha is 8-bit, so `{encoder}` can't encode it at {depth}"
            ));
        }
        VideoCodec::Vp9 => sys::AVPixelFormat::AV_PIX_FMT_YUVA420P,
        _ => {
            return Err(format!(
                "`{encoder}` encodes {encodes:?}, which lumen can't write alpha in; use prores (.mov) or vp9 (.webm)"
            ));
        }
    };
    if pixel_formats(codec).is_none_or(|formats| formats.contains(&format)) {
        Ok(())
    } else {
        Err(format!(
            "`{encoder}` doesn't take the frames with alpha lumen encodes from"
        ))
    }
}

/// The format a software encoder of `codec` takes frames in: 10-bit YUV 4:4:4 for ProRes and YUV
/// 4:2:0 for the rest, with an alpha plane when `alpha` is set.
pub(crate) fn cpu_input_format(
    codec: VideoCodec,
    depth: BitDepth,
    alpha: bool,
) -> sys::AVPixelFormat {
    use sys::AVPixelFormat::*;
    match (codec, depth, alpha) {
        (VideoCodec::ProRes, _, false) => AV_PIX_FMT_YUV444P10LE,
        (VideoCodec::ProRes, _, true) => AV_PIX_FMT_YUVA444P10LE,
        (_, _, true) => AV_PIX_FMT_YUVA420P,
        (_, BitDepth::Eight, false) => AV_PIX_FMT_YUV420P,
        (_, BitDepth::Ten, false) => AV_PIX_FMT_YUV420P10LE,
    }
}

/// Looks `encoder` up in this FFmpeg build.
pub fn encoder_info(encoder: &Encoder) -> Result<EncoderInfo> {
    let codec = video_encoder_by_name(encoder)?;
    Ok(EncoderInfo {
        codec: VideoCodec::from_av_codec_id(unsafe { (*codec).id }),
        family: encoder.family(),
    })
}

pub(crate) fn video_encoder_by_name(encoder: &Encoder) -> Result<*const sys::AVCodec> {
    let name = encoder.name();
    if encoder.family() == EncoderFamily::Vaapi {
        return Err(FfmpegError::new(
            "encoder_info",
            format!("VAAPI encoder `{name}` is not supported; use a Vulkan encoder or libx264"),
        ));
    }
    let codec = encoder_by_name(name, |avcodec_major| {
        missing_encoder_message(encoder, avcodec_major)
    })?;
    if unsafe { (*codec).type_ } != sys::AVMediaType::AVMEDIA_TYPE_VIDEO {
        return Err(FfmpegError::new(
            "encoder_info",
            format!("`{name}` is not a video encoder"),
        ));
    }
    // Vulkan encoders take frames from their own hardware frames context.
    if encoder.family() != EncoderFamily::Vulkan {
        let (format, described) = match VideoCodec::from_av_codec_id(unsafe { (*codec).id }) {
            VideoCodec::ProRes => (
                sys::AVPixelFormat::AV_PIX_FMT_YUV444P10LE,
                "10-bit YUV 4:4:4",
            ),
            _ => (sys::AVPixelFormat::AV_PIX_FMT_YUV420P, "YUV 4:2:0"),
        };
        // An encoder that lists no formats takes any.
        if pixel_formats(codec).is_some_and(|formats| !formats.contains(&format)) {
            return Err(FfmpegError::new(
                "encoder_info",
                format!("`{name}` doesn't take the {described} frames lumen encodes from"),
            ));
        }
    }
    Ok(codec)
}

fn pixel_formats(codec: *const sys::AVCodec) -> Option<&'static [sys::AVPixelFormat]> {
    #[cfg(ffmpeg_supported_config)]
    unsafe {
        supported_config(codec, sys::AVCodecConfig::AV_CODEC_CONFIG_PIX_FORMAT)
    }
    #[cfg(not(ffmpeg_supported_config))]
    unsafe {
        terminated_list((*codec).pix_fmts, sys::AVPixelFormat::AV_PIX_FMT_NONE)
    }
}

fn sample_formats(codec: *const sys::AVCodec) -> Option<&'static [sys::AVSampleFormat]> {
    #[cfg(ffmpeg_supported_config)]
    unsafe {
        supported_config(codec, sys::AVCodecConfig::AV_CODEC_CONFIG_SAMPLE_FORMAT)
    }
    #[cfg(not(ffmpeg_supported_config))]
    unsafe {
        terminated_list(
            (*codec).sample_fmts,
            sys::AVSampleFormat::AV_SAMPLE_FMT_NONE,
        )
    }
}

/// The values `codec` lists for `config`. `None` when it lists none, which means it takes any.
///
/// # Safety
///
/// `T` must be the element type FFmpeg documents for `config`.
#[cfg(ffmpeg_supported_config)]
unsafe fn supported_config<T>(
    codec: *const sys::AVCodec,
    config: sys::AVCodecConfig,
) -> Option<&'static [T]> {
    use std::ffi::{c_int, c_void};

    let mut values: *const c_void = std::ptr::null();
    let mut count: c_int = 0;
    let result = unsafe {
        sys::avcodec_get_supported_config(
            std::ptr::null(),
            codec,
            config,
            0,
            &mut values,
            &mut count,
        )
    };
    if result < 0 || values.is_null() {
        return None;
    }
    // The list is static data of the codec.
    Some(unsafe {
        std::slice::from_raw_parts(values.cast::<T>(), usize::try_from(count).unwrap_or(0))
    })
}

/// A codec's static list that `end` terminates. `None` when the codec has no list.
#[cfg(not(ffmpeg_supported_config))]
unsafe fn terminated_list<T: PartialEq>(list: *const T, end: T) -> Option<&'static [T]> {
    if list.is_null() {
        return None;
    }
    let mut count = 0;
    while unsafe { *list.add(count) != end } {
        count += 1;
    }
    Some(unsafe { std::slice::from_raw_parts(list, count) })
}

/// Finds the encoder `name`, with `missing` describing it for the libavcodec major version when
/// this build has none.
fn encoder_by_name(name: &str, missing: impl FnOnce(u32) -> String) -> Result<*const sys::AVCodec> {
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
            missing(unsafe { sys::avcodec_version() } >> 16),
        ))
    } else {
        Ok(codec)
    }
}

/// libavcodec 62 is FFmpeg 8.0, the first release with `av1_vulkan`.
fn missing_encoder_message(encoder: &Encoder, avcodec_major: u32) -> String {
    let name = encoder.name();
    if encoder.is(EncoderFamily::Vulkan, EncodeCodec::Av1) && avcodec_major < 62 {
        format!(
            "`av1_vulkan` needs FFmpeg 8.0 or newer, but this FFmpeg has libavcodec {avcodec_major}"
        )
    } else {
        format!("requested encoder `{name}` is unavailable")
    }
}

/// Why an audio encoder this build lacks can't be used.
fn missing_audio_encoder_message(name: &str) -> String {
    if name == OPUS_ENCODER {
        format!(
            "WebM audio is Opus, and this FFmpeg build has no `{name}` encoder; render without audio or to .mov"
        )
    } else {
        format!("requested audio encoder `{name}` is unavailable")
    }
}

/// The Opus encoder WebM audio uses.
pub(crate) const OPUS_ENCODER: &str = "libopus";

/// Errors, with a message naming what to do instead, when this FFmpeg build has no encoder for
/// `config`.
pub fn check_audio_encoder(config: &AudioEncoderConfig) -> std::result::Result<(), String> {
    find_audio_encoder(config)
        .map(|_| ())
        .map_err(|error| error.message)
}

pub(crate) fn find_audio_encoder(config: &AudioEncoderConfig) -> Result<*const sys::AVCodec> {
    if let Some(name) = config.encoder_name.as_deref() {
        return encoder_by_name(name, |_| missing_audio_encoder_message(name));
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
    use sys::AVSampleFormat::{AV_SAMPLE_FMT_FLT, AV_SAMPLE_FMT_FLTP};

    let Some(formats) = sample_formats(codec) else {
        return Ok(AV_SAMPLE_FMT_FLTP);
    };
    [AV_SAMPLE_FMT_FLTP, AV_SAMPLE_FMT_FLT]
        .into_iter()
        .find(|format| formats.contains(format))
        .ok_or_else(|| {
            FfmpegError::new(
                "AudioEncoder::create",
                "the audio encoder doesn't take f32 samples",
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup(name: &str) -> Result<EncoderInfo> {
        encoder_info(&Encoder::parse(name))
    }

    #[test]
    fn names_parse_to_their_family_and_codec() {
        use EncodeCodec as C;
        use EncoderFamily as F;
        for (name, family, codec) in [
            ("libx264", F::X264, Some(C::H264)),
            ("libx265", F::X265, Some(C::Hevc)),
            ("libsvtav1", F::SvtAv1, Some(C::Av1)),
            ("libaom-av1", F::Other, Some(C::Av1)),
            ("libvpx-vp9", F::Vpx, Some(C::Vp9)),
            ("prores_ks", F::ProRes, Some(C::ProRes)),
            ("h264_nvenc", F::Nvenc, Some(C::H264)),
            ("hevc_nvenc", F::Nvenc, Some(C::Hevc)),
            ("av1_nvenc", F::Nvenc, Some(C::Av1)),
            ("h264_vulkan", F::Vulkan, Some(C::H264)),
            ("hevc_vulkan", F::Vulkan, Some(C::Hevc)),
            ("av1_vulkan", F::Vulkan, Some(C::Av1)),
            ("h264_videotoolbox", F::VideoToolbox, Some(C::H264)),
            ("hevc_videotoolbox", F::VideoToolbox, Some(C::Hevc)),
            ("h264_vaapi", F::Vaapi, Some(C::H264)),
            ("aac", F::Other, None),
            ("missing_encoder", F::Other, None),
        ] {
            let encoder = Encoder::parse(name);
            assert_eq!(encoder.name(), name);
            assert_eq!(encoder.family(), family, "{name}");
            assert_eq!(encoder.codec(), codec, "{name}");
        }
    }

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
            if let Ok(info) = lookup(name) {
                assert_eq!(info.codec, codec, "{name}");
            }
        }
        assert_eq!(lookup("libx264").unwrap().codec, VideoCodec::H264);
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
            let error = lookup(name).unwrap_err();
            assert!(error.message.contains(expected), "{name}: {error}");
        }
    }

    #[test]
    fn ten_bit_needs_an_encoder_that_writes_it() {
        assert_eq!(
            check_bit_depth(&Encoder::parse("libx264"), BitDepth::Eight),
            Ok(())
        );
        let h264 = check_bit_depth(&Encoder::parse("libx264"), BitDepth::Ten).unwrap_err();
        assert!(h264.contains("always 8-bit"), "{h264}");
        for encoder in [
            "libx265",
            "libsvtav1",
            "hevc_vulkan",
            "av1_vulkan",
            "hevc_nvenc",
        ] {
            if lookup(encoder).is_ok() {
                assert_eq!(
                    check_bit_depth(&Encoder::parse(encoder), BitDepth::Ten),
                    Ok(()),
                    "{encoder}"
                );
            }
        }
        if lookup("hevc_videotoolbox").is_ok() {
            let error =
                check_bit_depth(&Encoder::parse("hevc_videotoolbox"), BitDepth::Ten).unwrap_err();
            assert!(error.contains("not supported by this encoder"), "{error}");
        }
        assert_eq!("10".parse(), Ok(BitDepth::Ten));
        assert!("12".parse::<BitDepth>().is_err());
    }

    #[test]
    fn av1_vulkan_below_ffmpeg_8_says_so() {
        let message = |name, major| missing_encoder_message(&Encoder::parse(name), major);
        assert!(message("av1_vulkan", 61).contains("FFmpeg 8.0"));
        // FFmpeg 8.0 or newer without the encoder is a build problem, not a version one.
        assert!(!message("av1_vulkan", 62).contains("FFmpeg 8.0"));
        assert!(!message("h264_vulkan", 61).contains("FFmpeg 8.0"));
    }
}
