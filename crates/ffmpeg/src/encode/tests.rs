use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg(ffmpeg_frame_cropping)]
use crate::FfmpegError;
#[cfg(ffmpeg_frame_cropping)]
use crate::ffi::sys;
#[cfg(ffmpeg_frame_cropping)]
use crate::format::InputContext;
use crate::gpu::GpuBackend;
#[cfg(feature = "vulkan")]
use crate::gpu::GpuVideoInput;
use crate::video::{CpuVideoFrame, EncodeMode, PixelFormat};

use super::{muxed::MuxedEncoder, quality::RateControl, video::VideoEncoderConfig};

fn temp_path(name: &str, extension: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!("lumen_ffmpeg_{name}_{unique}.{extension}"))
}

#[cfg(feature = "vulkan")]
#[test]
fn cpu_upload_encoder_rejects_gpu_input() {
    use ash::vk::Handle;

    use crate::gpu::VulkanVideoFrame;

    let path = temp_path("gpu_failure", "mp4");
    let config = VideoEncoderConfig::cpu_rgba(16, 16, 30, "libx264");
    let mut encoder =
        MuxedEncoder::create(path.to_string_lossy().to_string(), config).expect("libx264 encoder");
    // SAFETY: The CPU encoder rejects the frame before any work could read the fake buffer.
    let frame = unsafe {
        VulkanVideoFrame::new(
            ash::vk::Device::from_raw(1),
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

    let _ = fs::remove_file(path);
}

#[cfg(feature = "vulkan")]
#[test]
fn vulkan_texture_encoder_rejects_invalid_config_at_create() {
    for (width, height, expected) in [
        (15, 16, "even dimensions"),
        (16, 15, "even dimensions"),
        (16, 16, "vulkan_device"),
    ] {
        let path = temp_path("gpu_create_failure", "mp4");
        let mut config = VideoEncoderConfig::cpu_rgba(width, height, 30, "h264_vulkan");
        config.mode = EncodeMode::GpuTexture(GpuBackend::Vulkan);

        let error = match MuxedEncoder::create(path.to_string_lossy().to_string(), config) {
            Ok(_) => panic!("Vulkan texture encode at {width}x{height} should fail at create"),
            Err(error) => error,
        };
        assert_eq!(error.operation, "VideoEncoder::create");
        assert_eq!(error.backend, Some(GpuBackend::Vulkan));
        assert!(error.message.contains(expected), "{error}");

        let _ = fs::remove_file(path);
    }
}

#[cfg(ffmpeg_frame_cropping)]
#[test]
fn vulkan_av1_crops_coded_padding_in_the_container() {
    let (width, height) = (256, 138);
    let path = temp_path("vulkan_crop", "mp4");
    let config = VideoEncoderConfig::cpu_rgba(width, height, 30, "av1_vulkan");
    let mut encoder = match MuxedEncoder::create(path.to_string_lossy().to_string(), config) {
        Ok(encoder) => encoder,
        Err(error) if vulkan_encode_unavailable(&error) => {
            eprintln!("av1_vulkan unavailable ({error}); skipping Vulkan crop test");
            return;
        }
        Err(error) => panic!("av1_vulkan is available but failed to initialise: {error}"),
    };
    let Some(crop) = encoder.container_crop() else {
        eprintln!("av1_vulkan codes {width}x{height} exactly; skipping Vulkan crop test");
        return;
    };
    encoder
        .write_video_frame(&noisy_frame(width, height, 0))
        .expect("encode frame");
    encoder.finish().expect("finish encode");

    let input = InputContext::open(path.to_string_lossy().to_string()).expect("open encoded file");
    let stream = &input.media_info().video_streams[0];
    assert_eq!(
        (stream.width, stream.height),
        (crop.coded_width, crop.coded_height)
    );
    let sides: [u32; 4] = unsafe {
        let params = (**(*input.ptr).streams.add(stream.stream_index)).codecpar;
        let side_data = sys::av_packet_side_data_get(
            (*params).coded_side_data,
            (*params).nb_coded_side_data,
            sys::AVPacketSideDataType::AV_PKT_DATA_FRAME_CROPPING,
        );
        assert!(!side_data.is_null(), "stream has no cropping side data");
        std::slice::from_raw_parts((*side_data).data, 16)
            .as_chunks::<4>()
            .0
            .iter()
            .map(|side| u32::from_le_bytes(*side))
            .collect::<Vec<_>>()
            .try_into()
            .unwrap()
    };
    assert_eq!(
        sides,
        [0, crop.coded_height - height, 0, crop.coded_width - width]
    );

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
        pts: Some(i64::from(seed)),
        data,
    }
}

#[test]
fn unsupported_rate_control_errors_at_create() {
    let path = temp_path("unsupported_crf", "mp4");
    let mut config = VideoEncoderConfig::cpu_rgba(16, 16, 30, "libx264");
    config.quality.rate_control = RateControl::Crf(23);
    config.quality.preset = Some("p4".to_string());

    let error = match MuxedEncoder::create(path.to_string_lossy().to_string(), config) {
        Ok(_) => panic!("an NVENC preset should be rejected for libx264"),
        Err(error) => error,
    };
    assert!(error.message.contains("invalid preset"), "{error}");
    let _ = fs::remove_file(path);
}

#[test]
fn a_frame_without_a_pts_errors() {
    let path = temp_path("no_pts", "mp4");
    let config = VideoEncoderConfig::cpu_rgba(16, 16, 30, "libx264");
    let mut encoder =
        MuxedEncoder::create(path.to_string_lossy().to_string(), config).expect("libx264 encoder");
    let frame = CpuVideoFrame {
        pts: None,
        ..noisy_frame(16, 16, 0)
    };
    let error = encoder.write_video_frame(&frame).unwrap_err();
    assert!(error.message.contains("pts"), "{error}");
    let _ = fs::remove_file(path);
}

#[test]
fn texture_input_needs_an_encoder_of_that_backend() {
    let path = temp_path("texture_mismatch", "mp4");
    let mut config = VideoEncoderConfig::cpu_rgba(16, 16, 30, "libx264");
    config.mode = EncodeMode::GpuTexture(GpuBackend::Cuda);
    let error = match MuxedEncoder::create(path.to_string_lossy().to_string(), config) {
        Ok(_) => panic!("libx264 takes no CUDA textures"),
        Err(error) => error,
    };
    assert!(error.message.contains("libx264"), "{error}");
    let _ = fs::remove_file(path);
}

/// Missing encoder, no Vulkan device, or a device without video encode support.
#[cfg(ffmpeg_frame_cropping)]
fn vulkan_encode_unavailable(error: &FfmpegError) -> bool {
    error.operation == "avcodec_find_encoder_by_name"
        || error.operation == "av_hwdevice_ctx_create"
        || error.code == Some(sys::AVERROR(libc::ENOSYS))
}
