use std::ffi::CString;

use crate::ffi::sys;
use crate::gpu::GpuBackend;
use crate::video::{EncodeMode, VideoCodec};
use crate::{FfmpegError, Result};

use super::audio::AudioEncoderConfig;
use super::hardware::hardware_texture_encoder_name;
use super::video::VideoEncoderConfig;

pub(crate) fn find_encoder(config: &VideoEncoderConfig) -> Result<*const sys::AVCodec> {
    let encoder_name = match config.mode {
        EncodeMode::GpuTexture(backend) => {
            Some(hardware_encoder_name(config.codec, backend)?.to_string())
        }
        EncodeMode::CpuUpload => config.encoder_name.clone(),
    };

    if let Some(name) = encoder_name.as_deref() {
        encoder_by_name(name).map_err(|error| error.with_codec(config.codec))
    } else {
        let codec = unsafe { sys::avcodec_find_encoder(config.codec.to_av_codec_id()) };
        if codec.is_null() {
            Err(
                FfmpegError::new("avcodec_find_encoder", "requested encoder is unavailable")
                    .with_codec(config.codec),
            )
        } else {
            Ok(codec)
        }
    }
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
            format!("requested encoder `{name}` is unavailable"),
        ))
    } else {
        Ok(codec)
    }
}

pub(crate) fn hardware_encoder_name(
    codec: VideoCodec,
    backend: GpuBackend,
) -> Result<&'static str> {
    hardware_texture_encoder_name(backend, codec).ok_or_else(|| {
        FfmpegError::new(
            "VideoEncoder::create",
            if matches!(backend, GpuBackend::Vulkan) {
                format!("{backend:?} hardware texture encode is not available yet")
            } else {
                format!("{backend:?} hardware encode is unavailable for {codec:?}")
            },
        )
        .with_backend(backend)
        .with_codec(codec)
    })
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
