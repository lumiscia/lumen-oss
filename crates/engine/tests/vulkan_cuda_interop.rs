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

use lumen_ffmpeg::{
    CudaDriver, EncodeMode, GpuBackend, GpuVideoInput, MuxedEncoder, RateControl,
    VideoEncoderConfig, import_owned_vulkan_opaque_fd_image,
};

#[test]
#[ignore = "requires NVIDIA"]
fn vulkan_texture_imports_into_cuda_and_encodes_with_nvenc() {
    let renderer = pollster::block_on(lumen_gpu::Renderer::new())
        .expect("create Vulkan-capable wgpu renderer");
    let size = lumen_gpu::Size::new(640, 360);
    let exportable = renderer
        .create_exportable_vulkan_texture(
            Some("lumen vk-cuda-nvenc smoke texture"),
            size,
            lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
            lumen_gpu::wgpu::TextureUsages::COPY_DST
                | lumen_gpu::wgpu::TextureUsages::COPY_SRC
                | lumen_gpu::wgpu::TextureUsages::TEXTURE_BINDING,
        )
        .expect("create exportable Vulkan texture");

    let rgba = rgba_gradient(size.width, size.height);
    renderer.queue.write_texture(
        exportable.texture().as_image_copy(),
        &rgba,
        lumen_gpu::wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(size.width * 4),
            rows_per_image: Some(size.height),
        },
        size.as_extent(),
    );
    renderer
        .device
        .poll(lumen_gpu::wgpu::PollType::wait_indefinitely())
        .expect("wait for Vulkan texture upload");

    let driver = CudaDriver::load().expect("load CUDA driver");
    let vulkan_uuid = exportable.device_info().device_uuid;
    let ordinal = driver
        .devices()
        .expect("list CUDA devices")
        .into_iter()
        .find(|device| device.uuid == vulkan_uuid)
        .expect("CUDA device matching the Vulkan device")
        .ordinal;
    let context = driver
        .create_primary_context_for_ordinal(ordinal)
        .expect("create CUDA primary context");
    let imported = import_owned_vulkan_opaque_fd_image(
        &context,
        exportable
            .memory_fd()
            .try_clone()
            .expect("duplicate Vulkan memory fd for CUDA import"),
        exportable.allocation_size(),
        size.width,
        size.height,
    )
    .expect("import exportable Vulkan memory into CUDA");
    assert_ne!(imported.mipmapped_array_raw(), 0);
    assert_ne!(imported.level_zero_raw(), 0);
    let cuda_frame = context
        .allocate_rgba_frame(size.width, size.height)
        .expect("allocate CUDA RGBA frame");
    context
        .copy_image_to_rgba_frame(&imported, &cuda_frame)
        .expect("copy imported Vulkan image into CUDA RGBA frame");

    let path = temp_path("vk_cuda_nvenc", "mp4");
    let mut config = VideoEncoderConfig::cpu_rgba(size.width, size.height, 30, "h264_nvenc");
    config.mode = EncodeMode::GpuTexture(GpuBackend::Cuda);
    config.cuda_ordinal = Some(ordinal);
    config.quality.rate_control = RateControl::Bitrate(2_000_000);
    let mut encoder =
        MuxedEncoder::create(path.to_string_lossy().to_string(), config).expect("create NVENC");
    for pts in 0..30 {
        let frame = cuda_frame.as_video_frame(Some(pts));
        let input = GpuVideoInput::Cuda(&frame);
        encoder.write_gpu_frame(&input).expect("write CUDA frame");
    }
    encoder.finish().expect("finish NVENC encode");

    let metadata = fs::metadata(&path).expect("encoded output exists");
    assert!(metadata.len() > 0);
    let _ = fs::remove_file(path);
}

fn rgba_gradient(width: u32, height: u32) -> Vec<u8> {
    let mut rgba = vec![0; width as usize * height as usize * 4];
    for y in 0..height {
        for x in 0..width {
            let offset = (y as usize * width as usize + x as usize) * 4;
            rgba[offset] = (x % 256) as u8;
            rgba[offset + 1] = (y % 256) as u8;
            rgba[offset + 2] = 0x80;
            rgba[offset + 3] = 0xff;
        }
    }
    rgba
}

fn temp_path(name: &str, extension: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!("lumen_engine_{name}_{unique}.{extension}"))
}
