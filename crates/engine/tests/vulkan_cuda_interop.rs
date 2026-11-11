//! Vulkan-to-CUDA interop on NVIDIA hardware. Run with
//! `cargo test -p lumen-engine --features ffmpeg,cuda,vulkan --test vulkan_cuda_interop -- --ignored`.
#![cfg(all(
    target_os = "linux",
    feature = "ffmpeg",
    feature = "cuda",
    feature = "vulkan"
))]

use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use lumen_engine::gpu::CudaNvencTargetPool;
use lumen_ffmpeg::{
    ColorMatrix, ColorTags, DecodeMode, DecodedVideoFrame, EncodeMode, GpuBackend, InputContext,
    MuxedEncoder, RateControl, VideoDecoder, VideoDecoderConfig, VideoEncoderConfig,
};

#[test]
#[ignore = "requires NVIDIA"]
fn rendered_texture_encodes_with_nvenc_as_bt709() {
    let renderer = pollster::block_on(lumen_gpu::Renderer::new())
        .expect("create Vulkan-capable wgpu renderer");
    let size = lumen_gpu::Size::new(640, 360);
    let pool =
        CudaNvencTargetPool::new(&renderer, size, false).expect("create CUDA/NVENC target pool");
    let target = pool.acquire(&renderer).expect("create CUDA/NVENC target");
    // Opaque red in half floats: 1.0 is 0x3c00.
    let red = [0x00, 0x3c, 0, 0, 0, 0, 0x00, 0x3c];
    renderer.queue.write_texture(
        target.external_texture().texture_arc().as_image_copy(),
        &red.repeat((size.width * size.height) as usize),
        lumen_gpu::wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(size.width * 8),
            rows_per_image: Some(size.height),
        },
        size.as_extent(),
    );

    let path = temp_path("vk_cuda_nvenc", "mp4");
    let mut config = VideoEncoderConfig::cpu_rgba(size.width, size.height, 30, "h264_nvenc");
    config.mode = EncodeMode::GpuTexture(GpuBackend::Cuda);
    config.cuda_ordinal = Some(pool.cuda_ordinal());
    config.quality.rate_control = Some(RateControl::Bitrate(2_000_000));
    let mut encoder =
        MuxedEncoder::create(path.to_string_lossy().to_string(), config).expect("create NVENC");
    for pts in 0..30 {
        let converted = target.convert(&renderer);
        renderer
            .device
            .poll(lumen_gpu::wgpu::PollType::Wait {
                submission_index: Some(converted),
                timeout: None,
            })
            .expect("wait for NV12 conversion");
        let frame = target.video_frame(Some(pts)).expect("CUDA NV12 frame");
        encoder
            .write_gpu_frame(&target.video_input(&frame))
            .expect("write CUDA frame");
    }
    encoder.finish().expect("finish NVENC encode");

    let frame = first_frame(&path);
    assert_eq!(
        frame.color,
        ColorTags {
            matrix: ColorMatrix::Bt709,
            full_range: false,
        }
    );
    // Pure red in BT.709 limited range.
    let sample = frame.sample(size.width / 2, size.height / 2);
    assert!(
        sample
            .iter()
            .zip([63, 102, 240])
            .all(|(got, want)| got.abs_diff(want) <= 4),
        "decoded {sample:?}"
    );
    let _ = fs::remove_file(path);
}

fn first_frame(path: &std::path::Path) -> lumen_ffmpeg::YuvVideoFrame {
    let mut input = InputContext::open(path.to_string_lossy().to_string()).expect("open output");
    let stream_index = input
        .best_video_stream()
        .expect("video stream")
        .stream_index;
    let mut decoder = VideoDecoder::open(
        &input,
        VideoDecoderConfig {
            stream_index,
            mode: DecodeMode::Cpu,
        },
    )
    .expect("open decoder");
    while let Some(packet) = input.read_packet().expect("read packet") {
        decoder.send_packet(&packet).expect("send packet");
        if let Some(frame) = decoder.receive_video_frame().expect("receive frame") {
            match frame {
                DecodedVideoFrame::Yuv(frame) => return frame,
                DecodedVideoFrame::Rgba { .. } => panic!("NVENC writes a YUV stream"),
            }
        }
    }
    decoder.send_eof().expect("send eof");
    match decoder.receive_video_frame().expect("receive frame") {
        Some(DecodedVideoFrame::Yuv(frame)) => frame,
        _ => panic!("no decoded frame"),
    }
}

fn temp_path(name: &str, extension: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!("lumen_engine_{name}_{unique}.{extension}"))
}
