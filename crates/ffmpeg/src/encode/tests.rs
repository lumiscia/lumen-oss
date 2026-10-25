use std::{
    fs,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::FfmpegError;
use crate::ffi::sys;
use crate::format::InputContext;
use crate::gpu::{GpuBackend, GpuVideoInput};
use crate::video::{CpuVideoFrame, EncodeMode, PixelFormat, VideoCodec};

use super::{
    muxed::MuxedEncoder,
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

    let frame = VulkanVideoFrame::new(
        ash::vk::Image::from_raw(1),
        ash::vk::ImageView::from_raw(2),
        ash::vk::DeviceMemory::from_raw(3),
        ash::vk::Format::R8G8B8A8_UNORM,
        ash::vk::Extent3D {
            width: 1280,
            height: 720,
            depth: 1,
        },
    );
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
    let frame = VulkanVideoFrame::new(
        ash::vk::Image::from_raw(1),
        ash::vk::ImageView::from_raw(2),
        ash::vk::DeviceMemory::from_raw(3),
        ash::vk::Format::R8G8B8A8_UNORM,
        ash::vk::Extent3D {
            width: 16,
            height: 16,
            depth: 1,
        },
    );
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
fn unavailable_hardware_texture_encoder_errors_at_create() {
    let path = temp_path("gpu_create_failure", "mp4");
    let mut config = VideoEncoderConfig::cpu_rgba(16, 16, 30, VideoCodec::H264);
    config.mode = EncodeMode::GpuTexture(GpuBackend::Vulkan);

    let error = match MuxedEncoder::create(path.to_string_lossy().to_string(), config) {
        Ok(_) => panic!("Vulkan texture encode should fail at create"),
        Err(error) => error,
    };
    assert_eq!(error.operation, "VideoEncoder::create");
    assert_eq!(error.backend, Some(GpuBackend::Vulkan));

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

/// Missing encoder, no Vulkan device, or a device without video encode support.
fn vulkan_encode_unavailable(error: &FfmpegError) -> bool {
    error.operation == "avcodec_find_encoder_by_name"
        || error.operation == "av_hwdevice_ctx_create"
        || error.code == Some(sys::AVERROR(libc::ENOSYS))
}
