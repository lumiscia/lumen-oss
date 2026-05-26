use crate::gpu::GpuBackend;
use crate::video::VideoCodec;

use super::codec::encoder_by_name;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuTextureEncodeSupport {
    pub backend: GpuBackend,
    pub codec: VideoCodec,
    pub encoder_name: Option<&'static str>,
    pub available: bool,
    pub direct_texture_path: bool,
    pub reason: Option<String>,
}

pub(crate) fn hardware_texture_encoder_name(
    backend: GpuBackend,
    codec: VideoCodec,
) -> Option<&'static str> {
    match (backend, codec) {
        (GpuBackend::Cuda, VideoCodec::H264) => Some("h264_nvenc"),
        (GpuBackend::Cuda, VideoCodec::Hevc) => Some("hevc_nvenc"),
        (GpuBackend::Cuda, VideoCodec::Av1) => Some("av1_nvenc"),
        (GpuBackend::Metal, VideoCodec::H264) => Some("h264_videotoolbox"),
        (GpuBackend::Metal, VideoCodec::Hevc) => Some("hevc_videotoolbox"),
        _ => None,
    }
}

pub fn gpu_texture_encode_support(
    codec: VideoCodec,
    backend: GpuBackend,
) -> GpuTextureEncodeSupport {
    let encoder_name = hardware_texture_encoder_name(backend, codec);
    let texture_path_wired = matches!(backend, GpuBackend::Cuda | GpuBackend::Metal);
    let encoder_available =
        texture_path_wired && encoder_name.is_some_and(|name| encoder_by_name(name).is_ok());
    let reason = if !encoder_available {
        Some(match (texture_path_wired, encoder_name) {
            (false, _) => format!("{backend:?} hardware texture encode is not available yet"),
            (true, Some(name)) => format!("FFmpeg encoder `{name}` is unavailable"),
            (true, None) => format!("{backend:?} texture encode is unavailable for {codec:?}"),
        })
    } else {
        None
    };

    GpuTextureEncodeSupport {
        backend,
        codec,
        encoder_name,
        available: encoder_available,
        direct_texture_path: false,
        reason,
    }
}
