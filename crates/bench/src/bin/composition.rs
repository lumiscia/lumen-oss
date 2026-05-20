use std::{
    fmt::Debug,
    path::{Path, PathBuf},
    sync::mpsc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[cfg(all(target_os = "macos", feature = "metal"))]
use std::collections::VecDeque;

use anyhow::{Context, anyhow};
use lumen_engine::media::{ImageResolver, MediaStore, VideoFrameResolver};
use lumen_ffmpeg::{CpuVideoFrame, MuxedEncoder, PixelFormat, VideoCodec, VideoEncoderConfig};

#[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
use lumen_engine::gpu::CudaNvencTargetPool;
#[cfg(all(target_os = "macos", feature = "metal"))]
use lumen_engine::gpu::{MetalVideoToolboxTarget, MetalVideoToolboxTargetPool};
#[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
use lumen_ffmpeg::{CudaDriver, EncodeMode, GpuBackend, import_owned_vulkan_opaque_fd_image};

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
    fn parse(value: &str) -> anyhow::Result<Self> {
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

    fn name(self) -> &'static str {
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

    fn encodes(self) -> bool {
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

#[cfg(all(target_os = "macos", feature = "metal"))]
const GPU_ENCODER_FRAMES_IN_FLIGHT: usize = 3;

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
        let composition = lumen_engine::json::parse(demo.source)
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
                    "modes: all, render-only, render-profile, readback, readback-profile, cpu-encode, cpu-encode-profile, metal-videotoolbox, metal-videotoolbox-profile, vk-cuda-export, vk-cuda-nvenc"
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
        "usage: lumen-bench-composition [--composition all|announcement_gpu|feature_showcase] [--mode all|render-only|render-profile|readback|readback-profile|cpu-encode|cpu-encode-profile|metal-videotoolbox|metal-videotoolbox-profile|vk-cuda-export|vk-cuda-nvenc] [--frames N] [--save PATH]"
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
        Mode::MetalVideotoolbox,
        Mode::VkCudaExport,
        Mode::VkCudaNvenc,
    ]
}

fn requires_unsupported_platform(mode: Mode) -> bool {
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
    composition: &lumen_engine::composition::Composition,
    frames: u32,
    mode: Mode,
    output: Option<&Path>,
) -> anyhow::Result<Duration> {
    match mode {
        Mode::RenderOnly => benchmark_render_only(composition, frames).await,
        Mode::RenderProfile => benchmark_render_profile(composition, frames).await,
        Mode::Readback => benchmark_render_readback(composition, frames).await,
        Mode::ReadbackProfile => benchmark_render_readback_profile(composition, frames).await,
        Mode::CpuEncode => {
            benchmark_render_cpu_encode(
                composition,
                frames,
                output.ok_or_else(|| anyhow!("cpu encode mode needs an output path"))?,
            )
            .await
        }
        Mode::CpuEncodeProfile => {
            benchmark_render_cpu_encode_profile(
                composition,
                frames,
                output.ok_or_else(|| anyhow!("cpu encode profile mode needs an output path"))?,
            )
            .await
        }
        Mode::MetalVideotoolbox => {
            benchmark_render_metal_videotoolbox(
                composition,
                frames,
                output.ok_or_else(|| anyhow!("Metal VideoToolbox mode needs an output path"))?,
                false,
            )
            .await
        }
        Mode::MetalVideotoolboxProfile => {
            benchmark_render_metal_videotoolbox(
                composition,
                frames,
                output.ok_or_else(|| {
                    anyhow!("Metal VideoToolbox profile mode needs an output path")
                })?,
                true,
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
    composition: &lumen_engine::composition::Composition,
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

async fn benchmark_render_profile(
    composition: &lumen_engine::composition::Composition,
    frames: u32,
) -> anyhow::Result<Duration> {
    let media = EmptyMediaStore;
    let mut renderer = renderer(composition).await?;
    print_plan_profile(renderer.compiled());
    let mut bind = Duration::ZERO;
    let mut upload = Duration::ZERO;
    let mut submit = Duration::ZERO;
    let mut poll = Duration::ZERO;
    let started = Instant::now();
    for frame in 0..frames {
        let step = Instant::now();
        let bound = renderer.bind_frame(composition, frame, &media)?;
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

async fn benchmark_render_readback_profile(
    composition: &lumen_engine::composition::Composition,
    frames: u32,
) -> anyhow::Result<Duration> {
    let media = EmptyMediaStore;
    let mut renderer = renderer(composition).await?;
    print_plan_profile(renderer.compiled());
    let mut render = Duration::ZERO;
    let mut create_buffer = Duration::ZERO;
    let mut encode_copy = Duration::ZERO;
    let mut map_wait = Duration::ZERO;
    let mut copy_rows = Duration::ZERO;
    let started = Instant::now();
    for frame in 0..frames {
        let step = Instant::now();
        let (raster, _) = renderer.render_frame_submitted(composition, frame, &media)?;
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

async fn benchmark_render_cpu_encode_profile(
    composition: &lumen_engine::composition::Composition,
    frames: u32,
    output: &Path,
) -> anyhow::Result<Duration> {
    let media = EmptyMediaStore;
    let mut renderer = renderer(composition).await?;
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
        let (raster, _) = renderer.render_frame_submitted(composition, frame, &media)?;
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
    frames: u32,
    output: &Path,
    profile: bool,
) -> anyhow::Result<Duration> {
    let media = EmptyMediaStore;
    let mut renderer =
        renderer_with_format(composition, lumen_gpu::wgpu::TextureFormat::Bgra8Unorm).await?;
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
            &media,
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
    _composition: &lumen_engine::composition::Composition,
    _frames: u32,
) -> anyhow::Result<Duration> {
    Err(anyhow!(
        "vk-cuda-export requires linux + cuda + vulkan features"
    ))
}

#[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
async fn benchmark_render_vk_cuda_nvenc(
    composition: &lumen_engine::composition::Composition,
    frames: u32,
    output: &Path,
) -> anyhow::Result<Duration> {
    let media = EmptyMediaStore;
    let mut renderer = renderer(composition).await?;
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
            &media,
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
    _frames: u32,
    _output: &Path,
) -> anyhow::Result<Duration> {
    Err(anyhow!(
        "vk-cuda-nvenc requires linux + cuda + vulkan features"
    ))
}

async fn renderer(
    composition: &lumen_engine::composition::Composition,
) -> anyhow::Result<lumen_engine::gpu::GpuCompositionRenderer> {
    renderer_with_format(composition, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm).await
}

async fn renderer_with_format(
    composition: &lumen_engine::composition::Composition,
    format: lumen_gpu::wgpu::TextureFormat,
) -> anyhow::Result<lumen_engine::gpu::GpuCompositionRenderer> {
    let media = EmptyMediaStore;
    let mut renderer = lumen_engine::gpu::GpuCompositionRenderer::new().await?;
    renderer.compile_with_media(composition, &media, format)?;
    Ok(renderer)
}

fn video_config(
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
fn composition_size(composition: &lumen_engine::composition::Composition) -> lumen_gpu::Size {
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
    read_texture_rgba8_profile(renderer, id, size).map(|profile| profile.pixels)
}

struct ReadbackProfile {
    pixels: Vec<u8>,
    create_buffer: Duration,
    encode_copy: Duration,
    map_wait: Duration,
    copy_rows: Duration,
}

fn read_texture_rgba8_profile(
    renderer: &lumen_gpu::Renderer,
    id: lumen_gpu::TextureId,
    size: lumen_gpu::Size,
) -> anyhow::Result<ReadbackProfile> {
    let bytes_per_pixel = 4;
    let unpadded_bytes_per_row = size.width.saturating_mul(bytes_per_pixel);
    let padded_bytes_per_row = align_to(
        unpadded_bytes_per_row,
        lumen_gpu::wgpu::COPY_BYTES_PER_ROW_ALIGNMENT,
    );
    let output_size = u64::from(padded_bytes_per_row).saturating_mul(u64::from(size.height));
    let step = Instant::now();
    let output = renderer
        .device
        .create_buffer(&lumen_gpu::wgpu::BufferDescriptor {
            label: Some("lumen composition benchmark readback"),
            size: output_size.max(1),
            usage: lumen_gpu::wgpu::BufferUsages::COPY_DST
                | lumen_gpu::wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
    let create_buffer = step.elapsed();
    let step = Instant::now();
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
    let encode_copy = step.elapsed();

    let step = Instant::now();
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
    let map_wait = step.elapsed();

    let step = Instant::now();
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
    let copy_rows = step.elapsed();
    Ok(ReadbackProfile {
        pixels,
        create_buffer,
        encode_copy,
        map_wait,
        copy_rows,
    })
}

fn align_to(value: u32, alignment: u32) -> u32 {
    value.div_ceil(alignment) * alignment
}

fn fps(frames: u32, elapsed: Duration) -> f64 {
    f64::from(frames) / elapsed.as_secs_f64().max(1e-9)
}

fn print_plan_profile(compiled: Option<&lumen_engine::gpu::CompiledComposition>) {
    let Some(compiled) = compiled else {
        return;
    };
    println!(
        "plan_profile textures={} buffers={} programs={} passes={} frame_bindings={}",
        compiled.plan.textures().len(),
        compiled.plan.buffers().len(),
        compiled.plan.programs().len(),
        compiled.plan.passes().len(),
        compiled.frame_bindings.len(),
    );
    for pass in compiled.plan.passes() {
        let (kind, label) = match &pass.desc {
            lumen_gpu::PassDesc::Render(desc) => ("render", desc.label.as_deref()),
            lumen_gpu::PassDesc::Compute(desc) => ("compute", desc.label.as_deref()),
            lumen_gpu::PassDesc::CopyTexture(desc) => ("copy", desc.label.as_deref()),
        };
        println!(
            "plan_pass id={} kind={} label={}",
            pass.id.0,
            kind,
            label.unwrap_or("-")
        );
    }
}

fn micros_per_frame(duration: Duration, frames: u32) -> f64 {
    if frames == 0 {
        return 0.0;
    }
    duration.as_secs_f64() * 1_000_000.0 / f64::from(frames)
}

fn temp_path(name: &str, extension: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!("lumen_bench_{name}_{unique}.{extension}"))
}
