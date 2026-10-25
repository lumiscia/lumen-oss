use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::FfmpegError;
use crate::ffi::sys;
use crate::format::InputContext;
use crate::gpu::{GpuBackend, GpuVideoInput};
use crate::video::{CpuVideoFrame, EncodeMode, PixelFormat, VideoCodec};

use super::{
    muxed::MuxedEncoder,
    quality::RateControl,
    telemetry::{GpuEncodeTelemetry, GpuUploadDescriptor},
    video::VideoEncoderConfig,
};

fn temp_path(name: &str, extension: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!("lumen_ffmpeg_{name}_{unique}.{extension}"))
}

#[cfg(feature = "vulkan")]
#[test]
fn telemetry_tracks_upload_and_encode_events() {
    use ash::vk::Handle;

    use crate::gpu::VulkanVideoFrame;

    // SAFETY: Telemetry only reads frame metadata and never submits work on the fake buffer.
    let frame = unsafe {
        VulkanVideoFrame::new(
            ash::vk::Buffer::from_raw(1),
            1280,
            720,
            None,
            std::sync::Arc::new(()),
        )
    };
    let input = GpuVideoInput::Vulkan(&frame);
    let descriptor = GpuUploadDescriptor::from_frame(&input);
    let mut telemetry = GpuEncodeTelemetry::default();

    telemetry.record_upload_started(&descriptor);
    telemetry.record_upload_finished(&descriptor, Duration::from_micros(12));
    telemetry.record_encode_started(&descriptor);
    telemetry.record_encode_failed(&descriptor, Duration::from_micros(34), "not yet");

    assert_eq!(telemetry.upload_attempts, 1);
    assert_eq!(telemetry.upload_successes, 1);
    assert_eq!(telemetry.encode_attempts, 1);
    assert_eq!(telemetry.encode_failures, 1);
    assert_eq!(telemetry.vulkan_frames, 1);
    assert_eq!(telemetry.last_error.as_deref(), Some("not yet"));
    assert_eq!(telemetry.recent_events.len(), 4);
}

#[cfg(feature = "cuda")]
#[test]
fn cuda_telemetry_tracks_upload_attempts() {
    use crate::gpu::CudaVideoFrame;

    // SAFETY: Telemetry only reads frame metadata and never dereferences the fake device pointer.
    let frame = unsafe { CudaVideoFrame::from_device_ptr(0xABCD, 1920, 1080, 8192, Some(12)) };
    let input = GpuVideoInput::Cuda(&frame);
    let descriptor = GpuUploadDescriptor::from_frame(&input);
    let mut telemetry = GpuEncodeTelemetry::default();

    telemetry.record_upload_started(&descriptor);
    telemetry.record_upload_failed(&descriptor, Duration::from_micros(25), "not imported");

    assert_eq!(telemetry.upload_attempts, 1);
    assert_eq!(telemetry.upload_failures, 1);
    assert_eq!(telemetry.cuda_frames, 1);
    assert_eq!(telemetry.last_error.as_deref(), Some("not imported"));
    assert_eq!(telemetry.recent_events.len(), 2);
}

#[cfg(feature = "vulkan")]
#[test]
fn cpu_upload_encoder_rejects_gpu_input() {
    use ash::vk::Handle;

    use crate::gpu::VulkanVideoFrame;

    let path = temp_path("gpu_failure", "mp4");
    let Ok(mut encoder) = MuxedEncoder::create(
        path.to_string_lossy().to_string(),
        VideoEncoderConfig::cpu_rgba(16, 16, 30, VideoCodec::H264),
    ) else {
        eprintln!("H.264 encoder unavailable; skipping GPU input mismatch test");
        return;
    };
    // SAFETY: The CPU encoder rejects the frame before any work could read the fake buffer.
    let frame = unsafe {
        VulkanVideoFrame::new(
            ash::vk::Buffer::from_raw(1),
            16,
            16,
            None,
            std::sync::Arc::new(()),
        )
    };
    let input = GpuVideoInput::Vulkan(&frame);

    let error = encoder
        .write_gpu_frame(&input)
        .expect_err("CPU encoder should reject GPU input");
    assert_eq!(error.backend, Some(GpuBackend::Vulkan));
    assert_eq!(encoder.gpu_telemetry().upload_attempts, 0);
    assert_eq!(encoder.gpu_telemetry().upload_failures, 0);

    let _ = fs::remove_file(path);
}

#[cfg(feature = "vulkan")]
#[test]
fn vulkan_texture_encoder_without_device_errors_at_create() {
    let path = temp_path("gpu_create_failure", "mp4");
    let mut config = VideoEncoderConfig::cpu_rgba(16, 16, 30, VideoCodec::H264);
    config.mode = EncodeMode::GpuTexture(GpuBackend::Vulkan);

    let error = match MuxedEncoder::create(path.to_string_lossy().to_string(), config) {
        Ok(_) => panic!("Vulkan texture encode without a shared device should fail at create"),
        Err(error) => error,
    };
    assert_eq!(error.operation, "VideoEncoder::create");
    assert_eq!(error.backend, Some(GpuBackend::Vulkan));
    assert!(error.message.contains("vulkan_device"), "{error}");

    let _ = fs::remove_file(path);
}

#[test]
fn vulkan_encoder_uploads_cpu_frames() {
    let (width, height) = (256, 144);
    let path = temp_path("vulkan_upload", "mkv");
    let mut config = VideoEncoderConfig::cpu_rgba(width, height, 30, VideoCodec::Av1);
    config.encoder_name = Some("av1_vulkan".to_string());
    let mut encoder = match MuxedEncoder::create(path.to_string_lossy().to_string(), config) {
        Ok(encoder) => encoder,
        Err(error) if vulkan_encode_unavailable(&error) => {
            eprintln!("av1_vulkan unavailable ({error}); skipping Vulkan upload test");
            return;
        }
        Err(error) => panic!("av1_vulkan is available but failed to initialise: {error}"),
    };
    for index in 0..10u8 {
        let stride = width as usize * 4;
        encoder
            .write_video_frame(&CpuVideoFrame {
                width,
                height,
                stride,
                pixel_format: PixelFormat::Rgba8,
                pts: None,
                data: vec![index.wrapping_mul(25); stride * height as usize],
            })
            .expect("upload and encode frame");
    }
    encoder.finish().expect("finish encode");

    let info = InputContext::open(path.to_string_lossy().to_string())
        .expect("open encoded file")
        .media_info();
    let stream = &info.video_streams[0];
    assert_eq!(stream.codec, VideoCodec::Av1);
    assert_eq!((stream.width, stream.height), (width, height));

    let _ = fs::remove_file(path);
}

fn noisy_frame(width: u32, height: u32, seed: u32) -> CpuVideoFrame {
    let stride = width as usize * 4;
    let data = (0..stride * height as usize)
        .map(|index| {
            (index as u32)
                .wrapping_mul(2_654_435_761)
                .wrapping_add(seed) as u8
                >> 3
        })
        .collect();
    CpuVideoFrame {
        width,
        height,
        stride,
        pixel_format: PixelFormat::Rgba8,
        pts: None,
        data,
    }
}

fn encode_noise(path: &Path, config: VideoEncoderConfig) -> Result<(), FfmpegError> {
    let (width, height) = (config.width, config.height);
    let mut encoder = MuxedEncoder::create(path.to_string_lossy().to_string(), config)?;
    for index in 0..10 {
        encoder.write_video_frame(&noisy_frame(width, height, index))?;
    }
    encoder.finish()
}

#[test]
fn libx264_encodes_with_crf_and_preset() {
    let mut sizes = Vec::new();
    for crf in [18, 40] {
        let path = temp_path("x264_crf", "mp4");
        let mut config = VideoEncoderConfig::cpu_rgba(128, 72, 30, VideoCodec::H264);
        config.encoder_name = Some("libx264".to_string());
        config.rate_control = Some(RateControl::Crf(crf));
        config.preset = Some("ultrafast".to_string());
        match encode_noise(&path, config) {
            Ok(()) => {}
            Err(error) if error.operation == "avcodec_find_encoder_by_name" => {
                eprintln!("libx264 unavailable ({error}); skipping CRF test");
                return;
            }
            Err(error) => panic!("libx264 CRF encode failed: {error}"),
        }
        let info = InputContext::open(path.to_string_lossy().to_string())
            .expect("open encoded file")
            .media_info();
        assert_eq!(info.video_streams[0].codec, VideoCodec::H264);
        sizes.push(fs::metadata(&path).expect("encoded file").len());
        let _ = fs::remove_file(path);
    }
    assert!(sizes[0] > sizes[1], "lower CRF should be larger: {sizes:?}");
}

#[test]
fn unsupported_rate_control_errors_at_create() {
    let path = temp_path("unsupported_crf", "mp4");
    let mut config = VideoEncoderConfig::cpu_rgba(16, 16, 30, VideoCodec::H264);
    config.encoder_name = Some("libx264".to_string());
    config.rate_control = Some(RateControl::Crf(23));
    config.preset = Some("p4".to_string());

    let error = match MuxedEncoder::create(path.to_string_lossy().to_string(), config) {
        Ok(_) => panic!("an NVENC preset should be rejected for libx264"),
        Err(error) => error,
    };
    if error.operation == "avcodec_find_encoder_by_name" {
        return;
    }
    assert!(error.message.contains("invalid preset"), "{error}");
    let _ = fs::remove_file(path);
}

#[test]
fn vulkan_encoder_supports_qp_and_rejects_crf() {
    let path = temp_path("vulkan_qp", "mkv");
    let mut config = VideoEncoderConfig::cpu_rgba(256, 144, 30, VideoCodec::Av1);
    config.encoder_name = Some("av1_vulkan".to_string());
    config.rate_control = Some(RateControl::Crf(23));
    let error = match MuxedEncoder::create(path.to_string_lossy().to_string(), config.clone()) {
        Ok(_) => panic!("av1_vulkan has no CRF mode"),
        Err(error) => error,
    };
    if vulkan_encode_unavailable(&error) {
        eprintln!("av1_vulkan unavailable ({error}); skipping Vulkan QP test");
        return;
    }
    assert!(error.message.contains("CRF"), "{error}");

    config.rate_control = Some(RateControl::Qp(100));
    match encode_noise(&path, config) {
        Ok(()) => {}
        Err(error) if vulkan_encode_unavailable(&error) => {
            eprintln!("av1_vulkan unavailable ({error}); skipping Vulkan QP test");
            return;
        }
        Err(error) => panic!("av1_vulkan QP encode failed: {error}"),
    }
    let _ = fs::remove_file(path);
}

/// Missing encoder, no Vulkan device, or a device without video encode support.
fn vulkan_encode_unavailable(error: &FfmpegError) -> bool {
    error.operation == "avcodec_find_encoder_by_name"
        || error.operation == "av_hwdevice_ctx_create"
        || error.code == Some(sys::AVERROR(libc::ENOSYS))
}
