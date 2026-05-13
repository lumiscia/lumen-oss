use std::{
    fmt::Debug,
    path::{Path, PathBuf},
    sync::mpsc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, anyhow};
use lumen::media::{ImageResolver, MediaStore, VideoFrameResolver};
use lumen_ffmpeg::{CpuVideoFrame, MuxedEncoder, PixelFormat, VideoCodec, VideoEncoderConfig};

#[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
use lumen_ffmpeg::{
    CudaDriver, EncodeMode, GpuBackend, GpuVideoInput, import_owned_vulkan_opaque_fd_image,
};

#[derive(Debug)]
struct EmptyMediaStore;

impl MediaStore for EmptyMediaStore {
    fn get_image_resolver(&self, _source: &str) -> Option<Box<dyn ImageResolver>> {
        None
    }

    fn get_video_resolver(&self, _stream_id: &str) -> Option<Box<dyn VideoFrameResolver>> {
        None
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    RenderOnly,
    Readback,
    CpuEncode,
    VkCudaExport,
    VkCudaNvenc,
}

impl Mode {
    fn parse(value: &str) -> anyhow::Result<Self> {
        match value {
            "render-only" => Ok(Self::RenderOnly),
            "readback" => Ok(Self::Readback),
            "cpu-encode" => Ok(Self::CpuEncode),
            "vk-cuda-export" => Ok(Self::VkCudaExport),
            "vk-cuda-nvenc" => Ok(Self::VkCudaNvenc),
            _ => Err(anyhow!("unknown mode `{value}`")),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::RenderOnly => "render_only",
            Self::Readback => "readback",
            Self::CpuEncode => "cpu_encode",
            Self::VkCudaExport => "vk_cuda_export",
            Self::VkCudaNvenc => "vk_cuda_nvenc",
        }
    }

    fn encodes(self) -> bool {
        matches!(self, Self::CpuEncode | Self::VkCudaNvenc)
    }
}

struct DemoComposition {
    name: &'static str,
    source: &'static str,
}

const DEMOS: &[DemoComposition] = &[
    DemoComposition {
        name: "announcement_gpu",
        source: include_str!("../../../local/demo/announcement-gpu.json"),
    },
    DemoComposition {
        name: "feature_showcase",
        source: include_str!("../../../local/demo/feature-showcase.json"),
    },
];

#[derive(Debug)]
struct Args {
    composition: String,
    modes: Vec<Mode>,
    frames: Option<u32>,
    save: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = parse_args()?;
    let demos = selected_compositions(&args.composition)?;

    for demo in demos {
        let composition = lumen::json::parse(demo.source)
            .with_context(|| format!("failed to parse composition {}", demo.name))?;
        let frames = args
            .frames
            .unwrap_or_else(|| composition.timeline.duration_frames.min(90))
            .min(composition.timeline.duration_frames);

        for mode in selected_modes(&args.modes) {
            if requires_unsupported_platform(mode) {
                println!(
                    "composition_bench composition={} mode={} skipped=unsupported_platform",
                    demo.name,
                    mode.name()
                );
                continue;
            }
            let output = output_path(args.save.as_deref(), demo.name, mode, args.modes.len())?;
            let elapsed = run_mode(&composition, frames, mode, output.as_deref()).await?;
            if args.save.is_none()
                && let Some(path) = output.as_deref()
            {
                let _ = std::fs::remove_file(path);
            }
            println!(
                "composition_bench composition={} mode={} frames={} elapsed_ms={} fps={:.2} output={}",
                demo.name,
                mode.name(),
                frames,
                elapsed.as_millis(),
                fps(frames, elapsed),
                output
                    .as_deref()
                    .filter(|_| args.save.is_some())
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "-".to_string())
            );
        }
    }

    Ok(())
}

fn parse_args() -> anyhow::Result<Args> {
    let mut composition = "all".to_string();
    let mut modes = Vec::new();
    let mut frames = None;
    let mut save = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--composition" => {
                composition = args
                    .next()
                    .ok_or_else(|| anyhow!("--composition requires a value"))?;
            }
            "--mode" => {
                let value = args
                    .next()
                    .ok_or_else(|| anyhow!("--mode requires a value"))?;
                if value == "all" {
                    modes.clear();
                } else {
                    modes.push(Mode::parse(&value)?);
                }
            }
            "--frames" => {
                frames = Some(
                    args.next()
                        .ok_or_else(|| anyhow!("--frames requires a value"))?
                        .parse::<u32>()
                        .context("--frames must be a positive integer")?,
                );
            }
            "--save" => {
                save = Some(PathBuf::from(
                    args.next()
                        .ok_or_else(|| anyhow!("--save requires a path"))?,
                ));
            }
            "--list" => {
                println!("compositions: all, announcement_gpu, feature_showcase");
                println!(
                    "modes: all, render-only, readback, cpu-encode, vk-cuda-export, vk-cuda-nvenc"
                );
                std::process::exit(0);
            }
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            _ => return Err(anyhow!("unknown argument `{arg}`")),
        }
    }
    Ok(Args {
        composition,
        modes,
        frames,
        save,
    })
}

fn print_help() {
    println!(
        "usage: lumen-bench-composition [--composition all|announcement_gpu|feature_showcase] [--mode all|render-only|readback|cpu-encode|vk-cuda-export|vk-cuda-nvenc] [--frames N] [--save PATH]"
    );
}

fn selected_compositions(name: &str) -> anyhow::Result<Vec<&'static DemoComposition>> {
    if name == "all" {
        return Ok(DEMOS.iter().collect());
    }
    DEMOS
        .iter()
        .find(|demo| demo.name == name)
        .map(|demo| vec![demo])
        .ok_or_else(|| anyhow!("unknown composition `{name}`"))
}

fn selected_modes(modes: &[Mode]) -> Vec<Mode> {
    if !modes.is_empty() {
        return modes.to_vec();
    }
    vec![
        Mode::RenderOnly,
        Mode::Readback,
        Mode::CpuEncode,
        Mode::VkCudaExport,
        Mode::VkCudaNvenc,
    ]
}

fn requires_unsupported_platform(mode: Mode) -> bool {
    matches!(mode, Mode::VkCudaExport | Mode::VkCudaNvenc)
        && !cfg!(all(
            target_os = "linux",
            feature = "cuda",
            feature = "vulkan"
        ))
}

fn output_path(
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

async fn run_mode(
    composition: &lumen::composition::Composition,
    frames: u32,
    mode: Mode,
    output: Option<&Path>,
) -> anyhow::Result<Duration> {
    match mode {
        Mode::RenderOnly => benchmark_render_only(composition, frames).await,
        Mode::Readback => benchmark_render_readback(composition, frames).await,
        Mode::CpuEncode => {
            benchmark_render_cpu_encode(
                composition,
                frames,
                output.ok_or_else(|| anyhow!("cpu encode mode needs an output path"))?,
            )
            .await
        }
        Mode::VkCudaExport => benchmark_render_vk_cuda_export(composition, frames).await,
        Mode::VkCudaNvenc => {
            benchmark_render_vk_cuda_nvenc(
                composition,
                frames,
                output.ok_or_else(|| anyhow!("NVENC mode needs an output path"))?,
            )
            .await
        }
    }
}

async fn benchmark_render_only(
    composition: &lumen::composition::Composition,
    frames: u32,
) -> anyhow::Result<Duration> {
    let media = EmptyMediaStore;
    let mut renderer = renderer(composition).await?;
    let started = Instant::now();
    for frame in 0..frames {
        renderer.render_frame_submitted(composition, frame, &media)?;
        renderer
            .gpu_renderer()
            .device
            .poll(lumen_gpu::wgpu::PollType::wait_indefinitely())?;
    }
    Ok(started.elapsed())
}

async fn benchmark_render_readback(
    composition: &lumen::composition::Composition,
    frames: u32,
) -> anyhow::Result<Duration> {
    let media = EmptyMediaStore;
    let mut renderer = renderer(composition).await?;
    let started = Instant::now();
    for frame in 0..frames {
        let (raster, _) = renderer.render_frame_submitted(composition, frame, &media)?;
        let _pixels = read_texture_rgba8(
            renderer.gpu_renderer(),
            raster.texture,
            raster.domain.storage_size,
        )?;
    }
    Ok(started.elapsed())
}

async fn benchmark_render_cpu_encode(
    composition: &lumen::composition::Composition,
    frames: u32,
    output: &Path,
) -> anyhow::Result<Duration> {
    let media = EmptyMediaStore;
    let mut renderer = renderer(composition).await?;
    let width = composition.render_settings.width;
    let height = composition.render_settings.height;
    let mut encoder = MuxedEncoder::create(
        output.to_string_lossy().to_string(),
        video_config(composition, VideoCodec::H264),
    )?;
    let started = Instant::now();
    for frame in 0..frames {
        let (raster, _) = renderer.render_frame_submitted(composition, frame, &media)?;
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

#[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
async fn benchmark_render_vk_cuda_export(
    composition: &lumen::composition::Composition,
    frames: u32,
) -> anyhow::Result<Duration> {
    let media = EmptyMediaStore;
    let mut renderer = renderer(composition).await?;
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
        let (raster, _) = renderer.render_frame_submitted(composition, frame, &media)?;
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
    _composition: &lumen::composition::Composition,
    _frames: u32,
) -> anyhow::Result<Duration> {
    Err(anyhow!(
        "vk-cuda-export requires linux + cuda + vulkan features"
    ))
}

#[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
async fn benchmark_render_vk_cuda_nvenc(
    composition: &lumen::composition::Composition,
    frames: u32,
    output: &Path,
) -> anyhow::Result<Duration> {
    let media = EmptyMediaStore;
    let mut renderer = renderer(composition).await?;
    let output_size = composition_size(composition);
    let exportable = create_exportable_texture(renderer.gpu_renderer(), output_size)?;
    let driver = CudaDriver::load().map_err(|error| anyhow!(error))?;
    let context = driver
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
    let cuda_frame = driver
        .allocate_rgba_frame(output_size.width, output_size.height)
        .map_err(|error| anyhow!(error))?;
    let mut config = video_config(composition, VideoCodec::H264);
    config.mode = EncodeMode::GpuTexture(GpuBackend::Cuda);
    let mut encoder = MuxedEncoder::create(output.to_string_lossy().to_string(), config)?;

    let started = Instant::now();
    for frame in 0..frames {
        let (raster, _) = renderer.render_frame_submitted(composition, frame, &media)?;
        renderer
            .gpu_renderer()
            .copy_texture_to_external(raster.texture, exportable.texture())?;
        renderer
            .gpu_renderer()
            .device
            .poll(lumen_gpu::wgpu::PollType::wait_indefinitely())?;
        context.set_current().map_err(|error| anyhow!(error))?;
        driver
            .copy_image_to_rgba_frame(&imported, &cuda_frame)
            .map_err(|error| anyhow!(error))?;
        let frame = cuda_frame.as_video_frame(Some(i64::from(frame)));
        encoder.write_gpu_frame(&GpuVideoInput::Cuda(&frame))?;
    }
    encoder.finish()?;
    Ok(started.elapsed())
}

#[cfg(not(all(target_os = "linux", feature = "cuda", feature = "vulkan")))]
async fn benchmark_render_vk_cuda_nvenc(
    _composition: &lumen::composition::Composition,
    _frames: u32,
    _output: &Path,
) -> anyhow::Result<Duration> {
    Err(anyhow!(
        "vk-cuda-nvenc requires linux + cuda + vulkan features"
    ))
}

async fn renderer(
    composition: &lumen::composition::Composition,
) -> anyhow::Result<lumen::gpu::GpuCompositionRenderer> {
    let media = EmptyMediaStore;
    let mut renderer = lumen::gpu::GpuCompositionRenderer::new().await?;
    renderer.compile_with_media(
        composition,
        &media,
        lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
    )?;
    Ok(renderer)
}

fn video_config(
    composition: &lumen::composition::Composition,
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

#[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
fn composition_size(composition: &lumen::composition::Composition) -> lumen_gpu::Size {
    lumen_gpu::Size::new(
        composition.render_settings.width,
        composition.render_settings.height,
    )
}

#[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
fn create_exportable_texture(
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

fn read_texture_rgba8(
    renderer: &lumen_gpu::Renderer,
    id: lumen_gpu::TextureId,
    size: lumen_gpu::Size,
) -> anyhow::Result<Vec<u8>> {
    let bytes_per_pixel = 4;
    let unpadded_bytes_per_row = size.width.saturating_mul(bytes_per_pixel);
    let padded_bytes_per_row = align_to(
        unpadded_bytes_per_row,
        lumen_gpu::wgpu::COPY_BYTES_PER_ROW_ALIGNMENT,
    );
    let output_size = u64::from(padded_bytes_per_row).saturating_mul(u64::from(size.height));
    let output = renderer
        .device
        .create_buffer(&lumen_gpu::wgpu::BufferDescriptor {
            label: Some("lumen composition benchmark readback"),
            size: output_size.max(1),
            usage: lumen_gpu::wgpu::BufferUsages::COPY_DST
                | lumen_gpu::wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
    let mut encoder =
        renderer
            .device
            .create_command_encoder(&lumen_gpu::wgpu::CommandEncoderDescriptor {
                label: Some("lumen composition benchmark readback encoder"),
            });
    let texture = renderer
        .texture(id)
        .ok_or_else(|| anyhow!("unknown texture"))?;
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        lumen_gpu::wgpu::TexelCopyBufferInfo {
            buffer: &output,
            layout: lumen_gpu::wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_bytes_per_row),
                rows_per_image: Some(size.height),
            },
        },
        size.as_extent(),
    );
    renderer.queue.submit([encoder.finish()]);

    let slice = output.slice(..);
    let (tx, rx) = mpsc::channel();
    slice.map_async(lumen_gpu::wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result);
    });
    renderer
        .device
        .poll(lumen_gpu::wgpu::PollType::wait_indefinitely())?;
    rx.recv()
        .map_err(|_| anyhow!("GPU readback callback closed"))??;

    let mapped = slice.get_mapped_range();
    let mut pixels = vec![
        0;
        (size.width as usize)
            .saturating_mul(size.height as usize)
            .saturating_mul(bytes_per_pixel as usize)
    ];
    for row in 0..size.height as usize {
        let src_start = row.saturating_mul(padded_bytes_per_row as usize);
        let src_end = src_start.saturating_add(unpadded_bytes_per_row as usize);
        let dst_start = row.saturating_mul(unpadded_bytes_per_row as usize);
        let dst_end = dst_start.saturating_add(unpadded_bytes_per_row as usize);
        pixels[dst_start..dst_end].copy_from_slice(&mapped[src_start..src_end]);
    }
    drop(mapped);
    output.unmap();
    Ok(pixels)
}

fn align_to(value: u32, alignment: u32) -> u32 {
    value.div_ceil(alignment) * alignment
}

fn fps(frames: u32, elapsed: Duration) -> f64 {
    f64::from(frames) / elapsed.as_secs_f64().max(1e-9)
}

fn temp_path(name: &str, extension: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!("lumen_bench_{name}_{unique}.{extension}"))
}
