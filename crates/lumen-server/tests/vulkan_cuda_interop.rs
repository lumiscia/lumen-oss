#![cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]

use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use lumen_ffmpeg::{
    CudaDriver, EncodeMode, GpuBackend, GpuVideoInput, MuxedEncoder, VideoCodec,
    VideoEncoderConfig, import_owned_vulkan_opaque_fd_image,
};

#[tokio::test]
async fn imports_exportable_wgpu_vulkan_texture_into_cuda() {
    if std::env::var_os("LUMEN_TEST_VK_CUDA").is_none() {
        eprintln!("set LUMEN_TEST_VK_CUDA=1 to run Vulkan-to-CUDA hardware interop smoke test");
        return;
    }

    let renderer = lumen_gpu::Renderer::new()
        .await
        .expect("create Vulkan-capable wgpu renderer");
    let size = lumen_gpu::Size::new(640, 360);
    let exportable = renderer
        .create_exportable_vulkan_texture(
            Some("lumen vk-cuda smoke texture"),
            size,
            lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
            lumen_gpu::wgpu::TextureUsages::COPY_DST
                | lumen_gpu::wgpu::TextureUsages::COPY_SRC
                | lumen_gpu::wgpu::TextureUsages::TEXTURE_BINDING,
        )
        .expect("create exportable Vulkan texture");

    let driver = CudaDriver::load().expect("load CUDA driver");
    let _context = driver
        .create_primary_context()
        .expect("create CUDA context");
    let imported = import_owned_vulkan_opaque_fd_image(
        &driver,
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
}

#[tokio::test]
async fn imports_wgpu_vulkan_texture_into_cuda_and_encodes_with_nvenc() {
    if std::env::var_os("LUMEN_TEST_VK_CUDA_NVENC").is_none() {
        eprintln!("set LUMEN_TEST_VK_CUDA_NVENC=1 to run Vulkan-to-CUDA NVENC smoke test");
        return;
    }

    let renderer = lumen_gpu::Renderer::new()
        .await
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
    let context = driver
        .create_primary_context()
        .expect("create CUDA primary context");
    let imported = import_owned_vulkan_opaque_fd_image(
        &driver,
        exportable
            .memory_fd()
            .try_clone()
            .expect("duplicate Vulkan memory fd for CUDA import"),
        exportable.allocation_size(),
        size.width,
        size.height,
    )
    .expect("import exportable Vulkan memory into CUDA");
    let cuda_frame = driver
        .allocate_rgba_frame(size.width, size.height)
        .expect("allocate CUDA RGBA frame");
    context
        .set_current()
        .expect("restore CUDA primary context before copy");
    driver
        .copy_image_to_rgba_frame(&imported, &cuda_frame)
        .expect("copy imported Vulkan image into CUDA RGBA frame");

    let path = temp_path("vk_cuda_nvenc", "mp4");
    let mut config = VideoEncoderConfig::cpu_rgba(size.width, size.height, 30, VideoCodec::H264);
    config.mode = EncodeMode::GpuTexture(GpuBackend::Cuda);
    config.bit_rate = 2_000_000;
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
    std::env::temp_dir().join(format!("lumen_server_{name}_{unique}.{extension}"))
}
