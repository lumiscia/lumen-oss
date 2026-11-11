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
            crate::gpu::VulkanFrameLayout::nv12(16, 16),
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

/// A render that fails before its first frame still closes its encoder.
#[cfg(ffmpeg_frame_cropping)]
#[test]
fn finishing_a_vulkan_encoder_that_got_no_frames_does_not_crash() {
    for name in ["h264_vulkan", "av1_vulkan"] {
        let path = temp_path("vulkan_no_frames", "mp4");
        let config = VideoEncoderConfig::cpu_rgba(256, 144, 30, name);
        let encoder = match MuxedEncoder::create(path.to_string_lossy().to_string(), config) {
            Ok(encoder) => encoder,
            Err(error) if vulkan_encode_unavailable(&error) => {
                eprintln!("{name} unavailable ({error}); skipping");
                continue;
            }
            Err(error) => panic!("{name} is available but failed to initialise: {error}"),
        };
        encoder.finish().expect("finish encode");
        let _ = fs::remove_file(path);
    }
}

#[test]
fn unsupported_rate_control_errors_at_create() {
    let path = temp_path("unsupported_crf", "mp4");
    let mut config = VideoEncoderConfig::cpu_rgba(16, 16, 30, "libx264");
    config.quality.rate_control = Some(RateControl::Crf(23));
    config.quality.preset = Some("p4".to_string());

    let error = match MuxedEncoder::create(path.to_string_lossy().to_string(), config) {
        Ok(_) => panic!("an NVENC preset should be rejected for libx264"),
        Err(error) => error,
    };
    assert!(error.message.contains("invalid preset"), "{error}");
    let _ = fs::remove_file(path);
}

#[test]
fn an_option_that_changes_the_pixel_format_errors_at_create() {
    let path = temp_path("pixel_format_option", "mp4");
    let mut config = VideoEncoderConfig::cpu_rgba(16, 16, 30, "libx264");
    config.quality.options = vec![("pixel_format".to_string(), "yuv444p".to_string())];
    let error = match MuxedEncoder::create(path.to_string_lossy().to_string(), config) {
        Ok(_) => panic!("the bit depth sets the pixel format, not an option"),
        Err(error) => error,
    };
    assert!(error.message.contains("pixel format"), "{error}");
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

/// Encodes a few frames with each software encoder's `standard` settings, where this build has it.
#[test]
fn software_encoders_encode_through_a_preset() {
    use crate::{EncodeCodec, QualityConfig, QualityRequest};

    for (codec, encoder) in [
        (EncodeCodec::Hevc, "libx265"),
        (EncodeCodec::Av1, "libsvtav1"),
    ] {
        if super::codec::encoder_info(encoder).is_err() {
            eprintln!("{encoder} not in this FFmpeg build; skipping");
            continue;
        }
        let choice = QualityConfig::built_in()
            .resolve(&QualityRequest {
                codec: Some(codec),
                encoder: Some(encoder.to_string()),
                ..QualityRequest::default()
            })
            .expect("standard has an entry");
        let path = temp_path(encoder, "mp4");
        let mut config = VideoEncoderConfig::cpu_rgba(64, 64, 30, encoder);
        config.quality = choice.first().quality.clone();
        let mut muxed =
            MuxedEncoder::create(path.to_string_lossy().to_string(), config).expect(encoder);
        for seed in 0..5 {
            muxed
                .write_video_frame(&noisy_frame(64, 64, seed))
                .expect("write frame");
        }
        muxed.finish().expect("finish encode");
        assert!(fs::metadata(&path).expect("output").len() > 0);
        let _ = fs::remove_file(path);
    }
}

#[test]
fn a_container_that_cannot_hold_the_codec_errors_at_create() {
    let path = temp_path("h264_in_webm", "webm");
    let config = VideoEncoderConfig::cpu_rgba(16, 16, 30, "libx264");
    let error = match MuxedEncoder::create(path.to_string_lossy().to_string(), config) {
        Ok(_) => panic!("webm can't hold H.264"),
        Err(error) => error,
    };
    assert!(
        error.message.contains("webm container can't hold") && error.message.contains("libx264"),
        "{error}"
    );
    let _ = fs::remove_file(path);
}

/// A flat frame of `format` with luma `y`, neutral chroma, and alpha `left` on the left half and
/// `right` on the right, laid out as [`CpuVideoFrame`] describes with unpadded rows.
fn yuva_frame(format: PixelFormat, size: u32, y: u16, left: u16, right: u16) -> CpuVideoFrame {
    let (sample, full_chroma, depth) = match format {
        PixelFormat::Yuva420p => (1, false, 8),
        PixelFormat::Yuva444p10 => (2, true, 10),
        _ => unreachable!("an alpha format"),
    };
    let side = size as usize;
    let chroma_side = if full_chroma { side } else { side / 2 };
    let neutral = 1u16 << (depth - 1);
    let bytes = |value: u16| -> Vec<u8> {
        match sample {
            1 => vec![value as u8],
            _ => value.to_le_bytes().to_vec(),
        }
    };
    let mut data = Vec::new();
    for _ in 0..side * side {
        data.extend(bytes(y));
    }
    for _ in 0..2 * chroma_side * chroma_side {
        data.extend(bytes(neutral));
    }
    for _ in 0..side {
        for x in 0..side {
            data.extend(bytes(if x < side / 2 { left } else { right }));
        }
    }
    CpuVideoFrame {
        width: size,
        height: size,
        stride: side * sample,
        pixel_format: format,
        pts: Some(0),
        data,
    }
}

/// The first decoded frame of `path`'s video, with whether the container signals an alpha layer.
fn decode_first_yuv(path: &std::path::Path) -> (crate::video::YuvVideoFrame, bool) {
    use crate::video::{DecodeMode, DecodedVideoFrame, VideoDecoder, VideoDecoderConfig};

    let mut input = crate::format::InputContext::open(path.to_string_lossy().to_string())
        .expect("open encoded file");
    let stream_index = input
        .best_video_stream()
        .expect("video stream")
        .stream_index;
    let alpha_layer = input.has_alpha_layer(stream_index);
    let mut decoder = VideoDecoder::open(
        &input,
        VideoDecoderConfig {
            stream_index,
            mode: DecodeMode::Cpu,
        },
    )
    .expect("open decoder");
    loop {
        match input.read_packet().expect("read packet") {
            Some(packet) => decoder.send_packet(&packet).expect("send packet"),
            None => decoder.send_eof().expect("send eof"),
        }
        if let Some(frame) = decoder.receive_video_frame().expect("receive frame") {
            match frame {
                DecodedVideoFrame::Yuv(frame) => return (frame, alpha_layer),
                DecodedVideoFrame::Rgba { .. } => panic!("the stream decodes as YUV"),
            }
        }
    }
}

/// The alpha sample at `(x, y)` of a decoded planar frame with alpha.
fn alpha_sample(frame: &crate::video::YuvVideoFrame, x: u32, y: u32) -> u16 {
    let (width, height) = (frame.width as usize, frame.height as usize);
    let bytes = if frame.depth > 8 { 2 } else { 1 };
    let chroma = match frame.subsampling {
        crate::video::YuvSubsampling::Yuv444 => width * height,
        _ => width.div_ceil(2) * height.div_ceil(2),
    };
    let at = (width * height + 2 * chroma + y as usize * width + x as usize) * bytes;
    match bytes {
        1 => u16::from(frame.data[at]),
        _ => u16::from_le_bytes([frame.data[at], frame.data[at + 1]]),
    }
}

#[test]
fn alpha_planes_reach_prores_in_mov_and_vp9_in_webm() {
    use super::codec::BitDepth;

    for (encoder, extension, format, depth, (y, left, right), tolerance) in [
        (
            "prores_ks",
            "mov",
            PixelFormat::Yuva444p10,
            BitDepth::Ten,
            (500, 1023, 300),
            [4, 0],
        ),
        (
            "libvpx-vp9",
            "webm",
            PixelFormat::Yuva420p,
            BitDepth::Eight,
            (120, 255, 77),
            [3, 3],
        ),
    ] {
        if super::codec::encoder_info(encoder).is_err() {
            eprintln!("{encoder} not in this FFmpeg build; skipping its alpha round trip");
            continue;
        }
        let path = temp_path(encoder, extension);
        let mut config = VideoEncoderConfig::cpu_rgba(64, 64, 30, encoder);
        config.bit_depth = depth;
        config.alpha = true;
        let mut muxed =
            MuxedEncoder::create(path.to_string_lossy().to_string(), config).expect(encoder);
        assert_eq!(muxed.cpu_input_format(), Some(format));
        let rgba = noisy_frame(64, 64, 0);
        assert!(
            muxed.write_video_frame(&rgba).is_err(),
            "{encoder} takes no RGBA"
        );
        for pts in 0..3 {
            let frame = CpuVideoFrame {
                pts: Some(pts),
                ..yuva_frame(format, 64, y, left, right)
            };
            muxed.write_video_frame(&frame).expect("write frame");
        }
        muxed.finish().expect("finish encode");

        let (decoded, alpha_layer) = decode_first_yuv(&path);
        assert!(decoded.has_alpha, "{encoder} lost its alpha plane");
        assert_eq!(alpha_layer, encoder == "libvpx-vp9", "{encoder} alpha_mode");
        // FFmpeg decodes ProRes 4444 at 12 bits.
        let shift = decoded.depth - depth.bits();
        for (x, expected_alpha) in [(8, left), (56, right)] {
            let luma = decoded.sample(x, 32)[0] >> shift;
            assert!(
                luma.abs_diff(y) <= tolerance[0],
                "{encoder} Y {luma} at {x}"
            );
            let alpha = alpha_sample(&decoded, x, 32) >> shift;
            assert!(
                alpha.abs_diff(expected_alpha) <= tolerance[1],
                "{encoder} alpha {alpha} at {x}, wrote {expected_alpha}"
            );
        }
        let _ = fs::remove_file(path);
    }
}

#[test]
fn alpha_needs_an_encoder_that_writes_it() {
    let path = temp_path("alpha_h264", "mp4");
    let mut config = VideoEncoderConfig::cpu_rgba(16, 16, 30, "libx264");
    config.alpha = true;
    let error = match MuxedEncoder::create(path.to_string_lossy().to_string(), config) {
        Ok(_) => panic!("libx264 can't write alpha"),
        Err(error) => error,
    };
    assert!(
        error.message.contains("prores (.mov) or vp9 (.webm)"),
        "{error}"
    );
    let _ = fs::remove_file(path);
}
