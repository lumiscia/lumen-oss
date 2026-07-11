use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[cfg(all(target_os = "macos", feature = "metal"))]
use std::collections::VecDeque;

#[cfg(all(target_os = "macos", feature = "metal"))]
use anyhow::Context;
use anyhow::anyhow;
use lumen_ffmpeg::{CpuVideoFrame, MuxedEncoder, PixelFormat, VideoCodec, VideoEncoderConfig};

#[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
use lumen_engine::gpu::CudaNvencTargetPool;
#[cfg(all(target_os = "macos", feature = "metal"))]
use lumen_engine::gpu::{MetalVideoToolboxTarget, MetalVideoToolboxTargetPool};
#[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
use lumen_ffmpeg::{CudaDriver, EncodeMode, GpuBackend, import_owned_vulkan_opaque_fd_image};

use crate::bench::{media::BenchmarkMediaStore, timing::micros_per_frame};

use super::{
    profile::print_plan_profile,
    readback::{read_texture_rgba8, read_texture_rgba8_profile},
};

#[cfg(all(target_os = "macos", feature = "metal"))]
const GPU_ENCODER_FRAMES_IN_FLIGHT: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    RenderOnly,
    RenderProfile,
    Readback,
    ReadbackProfile,
    CpuEncode,
    CpuEncodeProfile,
    MetalVideotoolbox,
    MetalVideotoolboxProfile,
    VkCudaExport,
    VkCudaNvenc,
}

impl Mode {
    pub fn parse(value: &str) -> anyhow::Result<Self> {
        match value {
            "render-only" => Ok(Self::RenderOnly),
            "render-profile" => Ok(Self::RenderProfile),
            "readback" => Ok(Self::Readback),
            "readback-profile" => Ok(Self::ReadbackProfile),
            "cpu-encode" => Ok(Self::CpuEncode),
            "cpu-encode-profile" => Ok(Self::CpuEncodeProfile),
            "metal-videotoolbox" => Ok(Self::MetalVideotoolbox),
            "metal-videotoolbox-profile" => Ok(Self::MetalVideotoolboxProfile),
            "vk-cuda-export" => Ok(Self::VkCudaExport),
            "vk-cuda-nvenc" => Ok(Self::VkCudaNvenc),
            _ => Err(anyhow!("unknown mode `{value}`")),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::RenderOnly => "render_only",
            Self::RenderProfile => "render_profile",
            Self::Readback => "readback",
            Self::ReadbackProfile => "readback_profile",
            Self::CpuEncode => "cpu_encode",
            Self::CpuEncodeProfile => "cpu_encode_profile",
            Self::MetalVideotoolbox => "metal_videotoolbox",
            Self::MetalVideotoolboxProfile => "metal_videotoolbox_profile",
            Self::VkCudaExport => "vk_cuda_export",
            Self::VkCudaNvenc => "vk_cuda_nvenc",
        }
    }

    pub fn encodes(self) -> bool {
        matches!(
            self,
            Self::CpuEncode
                | Self::CpuEncodeProfile
                | Self::MetalVideotoolbox
                | Self::MetalVideotoolboxProfile
                | Self::VkCudaNvenc
        )
    }
}

pub fn selected_modes(modes: &[Mode]) -> Vec<Mode> {
    if !modes.is_empty() {
        return modes.to_vec();
    }
    vec![
        Mode::RenderOnly,
        Mode::Readback,
        Mode::CpuEncode,
        Mode::MetalVideotoolbox,
        Mode::VkCudaExport,
        Mode::VkCudaNvenc,
    ]
}

pub fn requires_unsupported_platform(mode: Mode) -> bool {
    if matches!(
        mode,
        Mode::MetalVideotoolbox | Mode::MetalVideotoolboxProfile
    ) && !cfg!(all(target_os = "macos", feature = "metal"))
    {
        return true;
    }
    matches!(mode, Mode::VkCudaExport | Mode::VkCudaNvenc)
        && !cfg!(all(
            target_os = "linux",
            feature = "cuda",
            feature = "vulkan"
        ))
}

pub fn output_path(
    save: Option<&Path>,
    composition: &str,
    mode: Mode,
    explicit_mode_count: usize,
) -> anyhow::Result<Option<PathBuf>> {
    if !mode.encodes() {
        return Ok(None);
    }
    let Some(save) = save else {
        return Ok(Some(temp_path(mode.name(), "mp4")));
    };
    if explicit_mode_count <= 1 {
        return Ok(Some(save.to_path_buf()));
    }
    let parent = save.parent().unwrap_or_else(|| Path::new("."));
    let stem = save
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("lumen-bench");
    let extension = save
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("mp4");
    Ok(Some(parent.join(format!(
        "{stem}_{composition}_{}.{extension}",
        mode.name()
    ))))
}

pub async fn run_mode(
    composition: &lumen_engine::composition::Composition,
    media: &BenchmarkMediaStore,
    frames: u32,
    mode: Mode,
    output: Option<&Path>,
) -> anyhow::Result<Duration> {
    match mode {
        Mode::RenderOnly => benchmark_render_only(composition, media, frames).await,
        Mode::RenderProfile => benchmark_render_profile(composition, media, frames).await,
        Mode::Readback => benchmark_render_readback(composition, media, frames).await,
        Mode::ReadbackProfile => {
            benchmark_render_readback_profile(composition, media, frames).await
        }
        Mode::CpuEncode => {
            benchmark_render_cpu_encode(
                composition,
                media,
                frames,
                output.ok_or_else(|| anyhow!("cpu encode mode needs an output path"))?,
            )
            .await
        }
        Mode::CpuEncodeProfile => {
            benchmark_render_cpu_encode_profile(
                composition,
                media,
                frames,
                output.ok_or_else(|| anyhow!("cpu encode profile mode needs an output path"))?,
            )
            .await
        }
        Mode::MetalVideotoolbox => {
            benchmark_render_metal_videotoolbox(
                composition,
                media,
                frames,
                output.ok_or_else(|| anyhow!("Metal VideoToolbox mode needs an output path"))?,
                false,
            )
            .await
        }
        Mode::MetalVideotoolboxProfile => {
            benchmark_render_metal_videotoolbox(
                composition,
                media,
                frames,
                output.ok_or_else(|| {
                    anyhow!("Metal VideoToolbox profile mode needs an output path")
                })?,
                true,
            )
            .await
        }
        Mode::VkCudaExport => benchmark_render_vk_cuda_export(composition, media, frames).await,
        Mode::VkCudaNvenc => {
            benchmark_render_vk_cuda_nvenc(
                composition,
                media,
                frames,
                output.ok_or_else(|| anyhow!("NVENC mode needs an output path"))?,
            )
            .await
        }
    }
}

async fn benchmark_render_only(
    composition: &lumen_engine::composition::Composition,
    media: &BenchmarkMediaStore,
    frames: u32,
) -> anyhow::Result<Duration> {
    let mut renderer = renderer(composition, media).await?;
    let started = Instant::now();
    for frame in 0..frames {
        renderer.render_frame_submitted(composition, frame, media)?;
        renderer
            .gpu_renderer()
            .device
            .poll(lumen_gpu::wgpu::PollType::wait_indefinitely())?;
    }
    Ok(started.elapsed())
}

async fn benchmark_render_profile(
    composition: &lumen_engine::composition::Composition,
    media: &BenchmarkMediaStore,
    frames: u32,
) -> anyhow::Result<Duration> {
    let mut renderer = renderer(composition, media).await?;
    print_plan_profile(renderer.compiled());
    let mut bind = Duration::ZERO;
    let mut upload = Duration::ZERO;
    let mut submit = Duration::ZERO;
    let mut poll = Duration::ZERO;
    let started = Instant::now();
    for frame in 0..frames {
        let step = Instant::now();
        let bound = renderer.bind_frame(composition, frame, media)?;
        bind += step.elapsed();

        let step = Instant::now();
        renderer.upload_bound_frame(&bound)?;
        upload += step.elapsed();

        let step = Instant::now();
        let _ = renderer.submit_render()?;
        submit += step.elapsed();

        let step = Instant::now();
        renderer
            .gpu_renderer()
            .device
            .poll(lumen_gpu::wgpu::PollType::wait_indefinitely())?;
        poll += step.elapsed();
    }
    let elapsed = started.elapsed();
    println!(
        "composition_profile frames={} bind_ms={} upload_ms={} submit_ms={} poll_ms={} bind_us_per_frame={:.2} upload_us_per_frame={:.2} submit_us_per_frame={:.2} poll_us_per_frame={:.2}",
        frames,
        bind.as_millis(),
        upload.as_millis(),
        submit.as_millis(),
        poll.as_millis(),
        micros_per_frame(bind, frames),
        micros_per_frame(upload, frames),
        micros_per_frame(submit, frames),
        micros_per_frame(poll, frames),
    );
    Ok(elapsed)
}

async fn benchmark_render_readback(
    composition: &lumen_engine::composition::Composition,
    media: &BenchmarkMediaStore,
    frames: u32,
) -> anyhow::Result<Duration> {
    let mut renderer = renderer(composition, media).await?;
    let started = Instant::now();
    for frame in 0..frames {
        let (raster, _) = renderer.render_frame_submitted(composition, frame, media)?;
        let _pixels = read_texture_rgba8(
            renderer.gpu_renderer(),
            raster.texture,
            raster.domain.storage_size,
        )?;
    }
    Ok(started.elapsed())
}

async fn benchmark_render_readback_profile(
    composition: &lumen_engine::composition::Composition,
    media: &BenchmarkMediaStore,
    frames: u32,
) -> anyhow::Result<Duration> {
    let mut renderer = renderer(composition, media).await?;
    print_plan_profile(renderer.compiled());
    let mut render = Duration::ZERO;
    let mut create_buffer = Duration::ZERO;
    let mut encode_copy = Duration::ZERO;
    let mut map_wait = Duration::ZERO;
    let mut copy_rows = Duration::ZERO;
    let started = Instant::now();
    for frame in 0..frames {
        let step = Instant::now();
        let (raster, _) = renderer.render_frame_submitted(composition, frame, media)?;
        render += step.elapsed();
        let timings = read_texture_rgba8_profile(
            renderer.gpu_renderer(),
            raster.texture,
            raster.domain.storage_size,
        )?;
        create_buffer += timings.create_buffer;
        encode_copy += timings.encode_copy;
        map_wait += timings.map_wait;
        copy_rows += timings.copy_rows;
    }
    let elapsed = started.elapsed();
    println!(
        "readback_profile frames={} render_ms={} create_buffer_ms={} encode_copy_ms={} map_wait_ms={} copy_rows_ms={} render_us_per_frame={:.2} create_buffer_us_per_frame={:.2} encode_copy_us_per_frame={:.2} map_wait_us_per_frame={:.2} copy_rows_us_per_frame={:.2}",
        frames,
        render.as_millis(),
        create_buffer.as_millis(),
        encode_copy.as_millis(),
        map_wait.as_millis(),
        copy_rows.as_millis(),
        micros_per_frame(render, frames),
        micros_per_frame(create_buffer, frames),
        micros_per_frame(encode_copy, frames),
        micros_per_frame(map_wait, frames),
        micros_per_frame(copy_rows, frames),
    );
    Ok(elapsed)
}

async fn benchmark_render_cpu_encode(
    composition: &lumen_engine::composition::Composition,
    media: &BenchmarkMediaStore,
    frames: u32,
    output: &Path,
) -> anyhow::Result<Duration> {
    let mut renderer = renderer(composition, media).await?;
    let width = composition.render_settings.width;
    let height = composition.render_settings.height;
    let mut encoder = MuxedEncoder::create(
        output.to_string_lossy().to_string(),
        video_config(composition, VideoCodec::H264),
    )?;
    let started = Instant::now();
    for frame in 0..frames {
        let (raster, _) = renderer.render_frame_submitted(composition, frame, media)?;
        let pixels = read_texture_rgba8(
            renderer.gpu_renderer(),
            raster.texture,
            raster.domain.storage_size,
        )?;
        encoder.write_video_frame(&CpuVideoFrame {
            width,
            height,
            stride: width as usize * 4,
            pixel_format: PixelFormat::Rgba8,
            pts: Some(i64::from(frame)),
            data: pixels,
        })?;
    }
    encoder.finish()?;
    Ok(started.elapsed())
}

async fn benchmark_render_cpu_encode_profile(
    composition: &lumen_engine::composition::Composition,
    media: &BenchmarkMediaStore,
    frames: u32,
    output: &Path,
) -> anyhow::Result<Duration> {
    let mut renderer = renderer(composition, media).await?;
    print_plan_profile(renderer.compiled());
    let width = composition.render_settings.width;
    let height = composition.render_settings.height;
    let mut encoder = MuxedEncoder::create(
        output.to_string_lossy().to_string(),
        video_config(composition, VideoCodec::H264),
    )?;
    let mut render = Duration::ZERO;
    let mut readback = Duration::ZERO;
    let mut encode = Duration::ZERO;
    let mut finish = Duration::ZERO;
    let started = Instant::now();
    for frame in 0..frames {
        let step = Instant::now();
        let (raster, _) = renderer.render_frame_submitted(composition, frame, media)?;
        render += step.elapsed();

        let step = Instant::now();
        let pixels = read_texture_rgba8(
            renderer.gpu_renderer(),
            raster.texture,
            raster.domain.storage_size,
        )?;
        readback += step.elapsed();

        let step = Instant::now();
        encoder.write_video_frame(&CpuVideoFrame {
            width,
            height,
            stride: width as usize * 4,
            pixel_format: PixelFormat::Rgba8,
            pts: Some(i64::from(frame)),
            data: pixels,
        })?;
        encode += step.elapsed();
    }
    let step = Instant::now();
    encoder.finish()?;
    finish += step.elapsed();
    let elapsed = started.elapsed();
    println!(
        "cpu_encode_profile frames={} render_ms={} readback_ms={} encode_ms={} finish_ms={} render_us_per_frame={:.2} readback_us_per_frame={:.2} encode_us_per_frame={:.2}",
        frames,
        render.as_millis(),
        readback.as_millis(),
        encode.as_millis(),
        finish.as_millis(),
        micros_per_frame(render, frames),
        micros_per_frame(readback, frames),
        micros_per_frame(encode, frames),
    );
    Ok(elapsed)
}

#[cfg(all(target_os = "macos", feature = "metal"))]
async fn benchmark_render_metal_videotoolbox(
    composition: &lumen_engine::composition::Composition,
    media: &BenchmarkMediaStore,
    frames: u32,
    output: &Path,
    profile: bool,
) -> anyhow::Result<Duration> {
    let mut renderer = renderer_with_format(
        composition,
        media,
        lumen_gpu::wgpu::TextureFormat::Bgra8Unorm,
    )
    .await?;
    if profile {
        print_plan_profile(renderer.compiled());
    }
    let output_size = composition_size(composition);
    let mut target_pool = MetalVideoToolboxTargetPool::bgra8(renderer.gpu_renderer(), output_size)?;
    let mut config = video_config(composition, VideoCodec::H264);
    config.mode = lumen_ffmpeg::EncodeMode::GpuTexture(lumen_ffmpeg::GpuBackend::Metal);
    let mut encoder = MuxedEncoder::create(output.to_string_lossy().to_string(), config)?;
    let mut pending = VecDeque::<PendingMetalEncodeFrame>::new();
    let mut render = Duration::ZERO;
    let mut poll = Duration::ZERO;
    let mut encode = Duration::ZERO;
    let mut finish = Duration::ZERO;
    let started = Instant::now();

    for frame in 0..frames {
        let target = target_pool.acquire(renderer.gpu_renderer(), frame)?;

        let step = Instant::now();
        let submitted = renderer.render_frame_into_external(
            composition,
            frame,
            media,
            target.external_texture(),
        )?;
        render += step.elapsed();
        pending.push_back(PendingMetalEncodeFrame {
            frame,
            target,
            submitted,
        });
        if pending.len() >= GPU_ENCODER_FRAMES_IN_FLIGHT {
            encode_ready_metal_frame(
                &renderer,
                &mut encoder,
                &mut pending,
                &mut poll,
                &mut encode,
            )?;
        }
    }
    while !pending.is_empty() {
        encode_ready_metal_frame(
            &renderer,
            &mut encoder,
            &mut pending,
            &mut poll,
            &mut encode,
        )?;
    }

    let step = Instant::now();
    encoder.finish()?;
    finish += step.elapsed();
    let elapsed = started.elapsed();
    if profile {
        println!(
            "metal_videotoolbox_profile frames={} render_ms={} poll_ms={} encode_ms={} finish_ms={} render_us_per_frame={:.2} poll_us_per_frame={:.2} encode_us_per_frame={:.2}",
            frames,
            render.as_millis(),
            poll.as_millis(),
            encode.as_millis(),
            finish.as_millis(),
            micros_per_frame(render, frames),
            micros_per_frame(poll, frames),
            micros_per_frame(encode, frames),
        );
    }
    Ok(elapsed)
}

#[cfg(not(all(target_os = "macos", feature = "metal")))]
async fn benchmark_render_metal_videotoolbox(
    _composition: &lumen_engine::composition::Composition,
    _media: &BenchmarkMediaStore,
    _frames: u32,
    _output: &Path,
    _profile: bool,
) -> anyhow::Result<Duration> {
    Err(anyhow!("metal-videotoolbox requires macOS + metal feature"))
}

#[cfg(all(target_os = "macos", feature = "metal"))]
struct PendingMetalEncodeFrame {
    frame: u32,
    target: MetalVideoToolboxTarget,
    submitted: lumen_gpu::SubmittedExternalTexture,
}

#[cfg(all(target_os = "macos", feature = "metal"))]
fn encode_ready_metal_frame(
    renderer: &lumen_engine::gpu::GpuCompositionRenderer,
    encoder: &mut MuxedEncoder,
    pending: &mut VecDeque<PendingMetalEncodeFrame>,
    poll: &mut Duration,
    encode: &mut Duration,
) -> anyhow::Result<()> {
    let PendingMetalEncodeFrame {
        frame,
        target,
        submitted,
    } = pending
        .pop_front()
        .ok_or_else(|| anyhow!("no pending Metal frame to encode"))?;
    let step = Instant::now();
    submitted.wait(&renderer.gpu_renderer().device)?;
    *poll += step.elapsed();

    let step = Instant::now();
    encoder
        .write_gpu_frame(&target.video_input())
        .with_context(|| format!("Metal VideoToolbox encode failed at frame {frame}"))?;
    *encode += step.elapsed();
    Ok(())
}

#[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
async fn benchmark_render_vk_cuda_export(
    composition: &lumen_engine::composition::Composition,
    media: &BenchmarkMediaStore,
    frames: u32,
) -> anyhow::Result<Duration> {
    let mut renderer = renderer(composition, media).await?;
    let output_size = composition_size(composition);
    let exportable = create_exportable_texture(renderer.gpu_renderer(), output_size)?;
    let driver = CudaDriver::load().map_err(|error| anyhow!(error))?;
    let _context = driver
        .create_primary_context()
        .map_err(|error| anyhow!(error))?;
    let imported = import_owned_vulkan_opaque_fd_image(
        &driver,
        exportable.memory_fd().try_clone()?,
        exportable.allocation_size(),
        output_size.width,
        output_size.height,
    )
    .map_err(|error| anyhow!(error))?;
    assert_ne!(imported.level_zero_raw(), 0);

    let started = Instant::now();
    for frame in 0..frames {
        let (raster, _) = renderer.render_frame_submitted(composition, frame, media)?;
        renderer
            .gpu_renderer()
            .copy_texture_to_external(raster.texture, exportable.texture())?;
        renderer
            .gpu_renderer()
            .device
            .poll(lumen_gpu::wgpu::PollType::wait_indefinitely())?;
    }
    Ok(started.elapsed())
}

#[cfg(not(all(target_os = "linux", feature = "cuda", feature = "vulkan")))]
async fn benchmark_render_vk_cuda_export(
    _composition: &lumen_engine::composition::Composition,
    _media: &BenchmarkMediaStore,
    _frames: u32,
) -> anyhow::Result<Duration> {
    Err(anyhow!(
        "vk-cuda-export requires linux + cuda + vulkan features"
    ))
}

#[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
async fn benchmark_render_vk_cuda_nvenc(
    composition: &lumen_engine::composition::Composition,
    media: &BenchmarkMediaStore,
    frames: u32,
    output: &Path,
) -> anyhow::Result<Duration> {
    let mut renderer = renderer(composition, media).await?;
    let output_size = composition_size(composition);
    let target_pool = CudaNvencTargetPool::rgba8(renderer.gpu_renderer(), output_size)?;
    let target = target_pool.acquire(renderer.gpu_renderer())?;
    let mut config = video_config(composition, VideoCodec::H264);
    config.mode = EncodeMode::GpuTexture(GpuBackend::Cuda);
    let mut encoder = MuxedEncoder::create(output.to_string_lossy().to_string(), config)?;

    let started = Instant::now();
    for frame in 0..frames {
        let submitted = renderer.render_frame_into_external(
            composition,
            frame,
            media,
            target.external_texture(),
        )?;
        submitted.wait(&renderer.gpu_renderer().device)?;
        target.copy_rendered_frame_to_cuda()?;
        let frame = target.video_frame(Some(i64::from(frame)));
        encoder.write_gpu_frame(&target.video_input(&frame))?;
    }
    encoder.finish()?;
    Ok(started.elapsed())
}

#[cfg(not(all(target_os = "linux", feature = "cuda", feature = "vulkan")))]
async fn benchmark_render_vk_cuda_nvenc(
    _composition: &lumen_engine::composition::Composition,
    _media: &BenchmarkMediaStore,
    _frames: u32,
    _output: &Path,
) -> anyhow::Result<Duration> {
    Err(anyhow!(
        "vk-cuda-nvenc requires linux + cuda + vulkan features"
    ))
}

pub(crate) async fn renderer(
    composition: &lumen_engine::composition::Composition,
    media: &BenchmarkMediaStore,
) -> anyhow::Result<lumen_engine::gpu::GpuCompositionRenderer> {
    renderer_with_format(
        composition,
        media,
        lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
    )
    .await
}

pub(crate) async fn renderer_with_format(
    composition: &lumen_engine::composition::Composition,
    media: &BenchmarkMediaStore,
    format: lumen_gpu::wgpu::TextureFormat,
) -> anyhow::Result<lumen_engine::gpu::GpuCompositionRenderer> {
    let mut renderer = lumen_engine::gpu::GpuCompositionRenderer::new().await?;
    renderer.compile_with_media(composition, media, format)?;
    Ok(renderer)
}

pub(crate) fn video_config(
    composition: &lumen_engine::composition::Composition,
    codec: VideoCodec,
) -> VideoEncoderConfig {
    let mut config = VideoEncoderConfig::cpu_rgba(
        composition.render_settings.width,
        composition.render_settings.height,
        composition.timeline.fps.round().max(1.0) as u32,
        codec,
    );
    config.bit_rate = 14_000_000;
    config
}

#[cfg(any(
    all(target_os = "macos", feature = "metal"),
    all(target_os = "linux", feature = "cuda", feature = "vulkan")
))]
pub(crate) fn composition_size(
    composition: &lumen_engine::composition::Composition,
) -> lumen_gpu::Size {
    lumen_gpu::Size::new(
        composition.render_settings.width,
        composition.render_settings.height,
    )
}

#[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
pub(crate) fn create_exportable_texture(
    renderer: &lumen_gpu::Renderer,
    size: lumen_gpu::Size,
) -> anyhow::Result<lumen_gpu::ExportableVulkanTexture> {
    renderer.create_exportable_vulkan_texture(
        Some("lumen composition benchmark export texture"),
        size,
        lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
        lumen_gpu::wgpu::TextureUsages::COPY_DST
            | lumen_gpu::wgpu::TextureUsages::COPY_SRC
            | lumen_gpu::wgpu::TextureUsages::TEXTURE_BINDING,
    )
}

pub fn temp_path(name: &str, extension: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!("lumen_bench_{name}_{unique}.{extension}"))
}
