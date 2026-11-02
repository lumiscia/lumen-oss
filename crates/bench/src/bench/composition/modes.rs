use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[cfg(all(target_os = "macos", feature = "metal"))]
use std::collections::VecDeque;

#[cfg(all(target_os = "macos", feature = "metal"))]
use anyhow::Context;
use anyhow::anyhow;
use lumen_engine::{composition::Composition, gpu::GpuCompositionRenderer, gpu::RasterHandle};
use lumen_ffmpeg::{
    CpuVideoFrame, MuxedEncoder, PixelFormat, Rational, VideoCodec, VideoEncoderConfig,
};

#[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
use lumen_engine::gpu::CudaNvencTargetPool;
#[cfg(all(target_os = "linux", feature = "vulkan"))]
use lumen_engine::gpu::VulkanVideoTarget;
#[cfg(all(target_os = "macos", feature = "metal"))]
use lumen_engine::gpu::{MetalVideoToolboxTarget, MetalVideoToolboxTargetPool};
#[cfg(any(
    all(target_os = "linux", feature = "vulkan"),
    all(target_os = "linux", feature = "cuda", feature = "vulkan")
))]
use lumen_ffmpeg::{EncodeMode, GpuBackend};
#[cfg(all(target_os = "linux", feature = "vulkan"))]
use lumen_ffmpeg::{GpuVideoInput, VulkanDevice, gpu_texture_encode_support};

use crate::bench::media::BenchmarkMediaStore;

use super::{
    profile::{PhaseTimes, print_plan_memory, print_plan_profile, timed},
    readback::read_texture_rgba8,
};

#[cfg(all(target_os = "macos", feature = "metal"))]
const GPU_ENCODER_FRAMES_IN_FLIGHT: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    RenderOnly,
    Readback,
    CpuEncode,
    MetalVideotoolbox,
    VulkanVideo,
    VkCudaNvenc,
}

impl Mode {
    pub const ALL: [Self; 6] = [
        Self::RenderOnly,
        Self::Readback,
        Self::CpuEncode,
        Self::MetalVideotoolbox,
        Self::VulkanVideo,
        Self::VkCudaNvenc,
    ];

    pub fn parse(value: &str) -> anyhow::Result<Self> {
        Self::ALL
            .into_iter()
            .find(|mode| mode.cli_name() == value)
            .ok_or_else(|| anyhow!("unknown mode `{value}`"))
    }

    pub fn cli_name(self) -> &'static str {
        match self {
            Self::RenderOnly => "render-only",
            Self::Readback => "readback",
            Self::CpuEncode => "cpu-encode",
            Self::MetalVideotoolbox => "metal-videotoolbox",
            Self::VulkanVideo => "vulkan-video",
            Self::VkCudaNvenc => "vk-cuda-nvenc",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::RenderOnly => "render_only",
            Self::Readback => "readback",
            Self::CpuEncode => "cpu_encode",
            Self::MetalVideotoolbox => "metal_videotoolbox",
            Self::VulkanVideo => "vulkan_video",
            Self::VkCudaNvenc => "vk_cuda_nvenc",
        }
    }

    pub fn encodes(self) -> bool {
        matches!(
            self,
            Self::CpuEncode | Self::MetalVideotoolbox | Self::VulkanVideo | Self::VkCudaNvenc
        )
    }

    pub fn requires_unsupported_platform(self) -> bool {
        match self {
            Self::MetalVideotoolbox => !cfg!(all(target_os = "macos", feature = "metal")),
            Self::VulkanVideo => !cfg!(all(target_os = "linux", feature = "vulkan")),
            Self::VkCudaNvenc => !cfg!(all(
                target_os = "linux",
                feature = "cuda",
                feature = "vulkan"
            )),
            Self::RenderOnly | Self::Readback | Self::CpuEncode => false,
        }
    }
}

/// What a mode renders.
pub struct Workload<'a> {
    pub composition: &'a Composition,
    pub media: &'a BenchmarkMediaStore,
    pub frames: u32,
    /// Disables the automatic reuse of static node output. Memos whose key is unchanged still
    /// hold.
    pub full_render: bool,
}

/// How long a run spent before its frame loop, and in it.
///
/// Modes that complete each frame (`render-only`, `readback`) also report the first frame, so the
/// rest is steady state. Encoders buffer, so no frame is complete until the encoder finishes;
/// encoding modes have no first-frame time and `steady` covers every frame through finish.
#[derive(Clone, Copy, Debug)]
pub struct RunTimes {
    pub setup: Duration,
    pub first_frame: Option<Duration>,
    pub steady: Duration,
    loop_started: Instant,
    steady_started: Instant,
}

impl RunTimes {
    /// Starts the frame loop. Diagnostics printed after the setup was measured stay out of it.
    fn begin(setup: Duration) -> Self {
        let now = Instant::now();
        Self {
            setup,
            first_frame: None,
            steady: Duration::ZERO,
            loop_started: now,
            steady_started: now,
        }
    }

    /// Marks frame `frame` as fully rendered; only the first one is recorded.
    fn frame_done(&mut self, frame: u32) {
        if frame == 0 {
            let now = Instant::now();
            self.first_frame = Some(now - self.loop_started);
            self.steady_started = now;
        }
    }

    fn finish(mut self) -> Self {
        self.steady = self.steady_started.elapsed();
        self
    }

    pub fn loop_total(&self) -> Duration {
        self.first_frame.unwrap_or_default() + self.steady
    }

    /// Frames timed by `steady`.
    pub fn steady_frames(&self, frames: u32) -> u32 {
        frames - u32::from(self.first_frame.is_some())
    }
}

pub fn output_path(
    save: Option<&Path>,
    composition: &str,
    mode: Mode,
    encoding_runs: usize,
) -> PathBuf {
    let Some(save) = save else {
        return temp_path(mode.name(), "mp4");
    };
    if encoding_runs <= 1 {
        return save.to_path_buf();
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
    parent.join(format!("{stem}_{composition}_{}.{extension}", mode.name()))
}

pub async fn run_mode(
    workload: &Workload<'_>,
    mode: Mode,
    output: Option<&Path>,
    mut phases: Option<&mut PhaseTimes>,
) -> anyhow::Result<RunTimes> {
    let phases = &mut phases;
    let output = || output.ok_or_else(|| anyhow!("{} needs an output path", mode.cli_name()));
    match mode {
        Mode::RenderOnly => render_only(workload, phases).await,
        Mode::Readback => readback(workload, phases).await,
        Mode::CpuEncode => cpu_encode(workload, output()?, phases).await,
        Mode::MetalVideotoolbox => metal_videotoolbox(workload, output()?, phases).await,
        Mode::VulkanVideo => vulkan_video(workload, output()?, phases).await,
        Mode::VkCudaNvenc => vk_cuda_nvenc(workload, output()?, phases).await,
    }
}

/// Binds, uploads and submits one frame to the renderer's own output texture.
fn render_frame(
    renderer: &mut GpuCompositionRenderer,
    workload: &Workload<'_>,
    frame: u32,
    phases: &mut Option<&mut PhaseTimes>,
) -> anyhow::Result<RasterHandle> {
    let bound = timed(phases, "bind", || {
        renderer.bind_frame(workload.composition, frame, workload.media)
    })?;
    timed(phases, "upload", || renderer.upload_bound_frame(&bound))?;
    let (raster, _) = timed(phases, "submit", || renderer.submit_render())?;
    Ok(raster)
}

async fn render_only(
    workload: &Workload<'_>,
    phases: &mut Option<&mut PhaseTimes>,
) -> anyhow::Result<RunTimes> {
    let setup_started = Instant::now();
    let mut renderer = renderer(workload).await?;
    let setup = setup_started.elapsed();
    if phases.is_some() {
        print_plan_profile(renderer.compiled());
    }
    let mut times = RunTimes::begin(setup);
    for frame in 0..workload.frames {
        render_frame(&mut renderer, workload, frame, phases)?;
        timed(phases, "poll", || {
            renderer
                .gpu_renderer()
                .device
                .poll(lumen_gpu::wgpu::PollType::wait_indefinitely())
        })?;
        times.frame_done(frame);
    }
    let times = times.finish();
    print_plan_memory(renderer.compiled());
    Ok(times)
}

async fn readback(
    workload: &Workload<'_>,
    phases: &mut Option<&mut PhaseTimes>,
) -> anyhow::Result<RunTimes> {
    let setup_started = Instant::now();
    let mut renderer = renderer(workload).await?;
    let setup = setup_started.elapsed();
    if phases.is_some() {
        print_plan_profile(renderer.compiled());
    }
    let mut times = RunTimes::begin(setup);
    for frame in 0..workload.frames {
        let raster = render_frame(&mut renderer, workload, frame, phases)?;
        read_texture_rgba8(
            renderer.gpu_renderer(),
            raster.texture,
            raster.domain.storage_size,
            phases,
        )?;
        times.frame_done(frame);
    }
    Ok(times.finish())
}

async fn cpu_encode(
    workload: &Workload<'_>,
    output: &Path,
    phases: &mut Option<&mut PhaseTimes>,
) -> anyhow::Result<RunTimes> {
    let setup_started = Instant::now();
    let mut renderer = renderer(workload).await?;
    let width = workload.composition.render_settings().width;
    let height = workload.composition.render_settings().height;
    let mut encoder = MuxedEncoder::create(
        output.to_string_lossy().to_string(),
        video_config(workload.composition, VideoCodec::H264)?,
    )?;
    let setup = setup_started.elapsed();
    if phases.is_some() {
        print_plan_profile(renderer.compiled());
    }
    let times = RunTimes::begin(setup);
    for frame in 0..workload.frames {
        let raster = render_frame(&mut renderer, workload, frame, phases)?;
        let pixels = read_texture_rgba8(
            renderer.gpu_renderer(),
            raster.texture,
            raster.domain.storage_size,
            phases,
        )?;
        timed(phases, "encode", || {
            encoder.write_video_frame(&CpuVideoFrame {
                width,
                height,
                stride: width as usize * 4,
                pixel_format: PixelFormat::Rgba8,
                pts: Some(i64::from(frame)),
                data: pixels,
            })
        })?;
    }
    timed(phases, "finish", || encoder.finish())?;
    Ok(times.finish())
}

#[cfg(all(target_os = "macos", feature = "metal"))]
async fn metal_videotoolbox(
    workload: &Workload<'_>,
    output: &Path,
    phases: &mut Option<&mut PhaseTimes>,
) -> anyhow::Result<RunTimes> {
    let setup_started = Instant::now();
    let mut renderer =
        renderer_with_format(workload, lumen_gpu::wgpu::TextureFormat::Bgra8Unorm).await?;
    let output_size = composition_size(workload.composition);
    let mut target_pool = MetalVideoToolboxTargetPool::bgra8(renderer.gpu_renderer(), output_size)?;
    let mut config = video_config(workload.composition, VideoCodec::H264)?;
    config.mode = lumen_ffmpeg::EncodeMode::GpuTexture(lumen_ffmpeg::GpuBackend::Metal);
    let mut encoder = MuxedEncoder::create(output.to_string_lossy().to_string(), config)?;
    let mut pending = VecDeque::<PendingMetalEncodeFrame>::new();
    let setup = setup_started.elapsed();
    if phases.is_some() {
        print_plan_profile(renderer.compiled());
    }
    let times = RunTimes::begin(setup);

    for frame in 0..workload.frames {
        let target = target_pool.acquire(renderer.gpu_renderer(), frame)?;
        let submitted = timed(phases, "render", || {
            renderer.render_frame_into_external(
                workload.composition,
                frame,
                workload.media,
                target.external_texture(),
            )
        })?;
        pending.push_back(PendingMetalEncodeFrame {
            frame,
            target,
            submitted,
        });
        if pending.len() >= GPU_ENCODER_FRAMES_IN_FLIGHT {
            encode_ready_metal_frame(&renderer, &mut encoder, &mut pending, phases)?;
        }
    }
    while !pending.is_empty() {
        encode_ready_metal_frame(&renderer, &mut encoder, &mut pending, phases)?;
    }
    timed(phases, "finish", || encoder.finish())?;
    Ok(times.finish())
}

#[cfg(not(all(target_os = "macos", feature = "metal")))]
async fn metal_videotoolbox(
    _workload: &Workload<'_>,
    _output: &Path,
    _phases: &mut Option<&mut PhaseTimes>,
) -> anyhow::Result<RunTimes> {
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
    renderer: &GpuCompositionRenderer,
    encoder: &mut MuxedEncoder,
    pending: &mut VecDeque<PendingMetalEncodeFrame>,
    phases: &mut Option<&mut PhaseTimes>,
) -> anyhow::Result<()> {
    let PendingMetalEncodeFrame {
        frame,
        target,
        submitted,
    } = pending
        .pop_front()
        .ok_or_else(|| anyhow!("no pending Metal frame to encode"))?;
    timed(phases, "poll", || {
        submitted.wait(&renderer.gpu_renderer().device)
    })?;
    timed(phases, "encode", || {
        encoder.write_gpu_frame(&target.video_input())
    })
    .with_context(|| format!("Metal VideoToolbox encode failed at frame {frame}"))?;
    Ok(())
}

/// Renders and encodes on one Vulkan device, so frames never leave the GPU.
#[cfg(all(target_os = "linux", feature = "vulkan"))]
async fn vulkan_video(
    workload: &Workload<'_>,
    output: &Path,
    phases: &mut Option<&mut PhaseTimes>,
) -> anyhow::Result<RunTimes> {
    let setup_started = Instant::now();
    let size = composition_size(workload.composition);
    if !size.width.is_multiple_of(2) || !size.height.is_multiple_of(2) {
        return Err(anyhow!(
            "NV12 frames need even dimensions, got {}x{}",
            size.width,
            size.height
        ));
    }
    let (mut renderer, device) = GpuCompositionRenderer::new_vulkan_video().await?;
    renderer.set_static_output_reuse(!workload.full_render);
    renderer.compile_with_media(
        workload.composition,
        workload.media,
        lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
    )?;
    let target = VulkanVideoTarget::nv12(renderer.gpu_renderer(), size);
    let (mut encoder, encoder_name) = open_vulkan_encoder(workload.composition, &device, output)?;
    let setup = setup_started.elapsed();
    println!("composition_bench vulkan_video encoder={encoder_name}");
    if phases.is_some() {
        print_plan_profile(renderer.compiled());
    }
    let times = RunTimes::begin(setup);
    for frame in 0..workload.frames {
        let raster = render_frame(&mut renderer, workload, frame, phases)?;
        let texture = renderer
            .gpu_renderer()
            .texture(raster.texture)
            .ok_or_else(|| anyhow!("render output texture {:?} is unavailable", raster.texture))?;
        let video_frame = timed(phases, "convert", || {
            target.convert(renderer.gpu_renderer(), texture, Some(i64::from(frame)))
        })?;
        timed(phases, "flush", || {
            renderer
                .gpu_renderer()
                .device
                .poll(lumen_gpu::wgpu::PollType::Poll)
        })?;
        timed(phases, "encode", || {
            encoder.write_gpu_frame(&GpuVideoInput::Vulkan(&video_frame))
        })?;
    }
    timed(phases, "finish", || encoder.finish())?;
    Ok(times.finish())
}

/// Opens the first Vulkan Video encoder the machine supports, preferring H.264.
#[cfg(all(target_os = "linux", feature = "vulkan"))]
fn open_vulkan_encoder(
    composition: &Composition,
    device: &VulkanDevice,
    output: &Path,
) -> anyhow::Result<(MuxedEncoder, &'static str)> {
    let mut failures = Vec::new();
    for codec in [VideoCodec::H264, VideoCodec::Hevc, VideoCodec::Av1] {
        let support = gpu_texture_encode_support(codec, GpuBackend::Vulkan);
        let Some(name) = support.encoder_name.filter(|_| support.available) else {
            failures.push(format!("{codec:?}: {}", support.reason.unwrap_or_default()));
            continue;
        };
        let mut config = video_config(composition, codec)?;
        config.mode = EncodeMode::GpuTexture(GpuBackend::Vulkan);
        config.vulkan_device = Some(device.clone());
        match MuxedEncoder::create(output.to_string_lossy().to_string(), config) {
            Ok(encoder) => return Ok((encoder, name)),
            Err(error) => failures.push(format!("{name}: {error}")),
        }
    }
    Err(anyhow!(
        "no Vulkan Video encoder opened ({})",
        failures.join("; ")
    ))
}

#[cfg(not(all(target_os = "linux", feature = "vulkan")))]
async fn vulkan_video(
    _workload: &Workload<'_>,
    _output: &Path,
    _phases: &mut Option<&mut PhaseTimes>,
) -> anyhow::Result<RunTimes> {
    Err(anyhow!("vulkan-video requires linux + vulkan feature"))
}

#[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
async fn vk_cuda_nvenc(
    workload: &Workload<'_>,
    output: &Path,
    phases: &mut Option<&mut PhaseTimes>,
) -> anyhow::Result<RunTimes> {
    let setup_started = Instant::now();
    let mut renderer = renderer(workload).await?;
    let output_size = composition_size(workload.composition);
    let target_pool = CudaNvencTargetPool::rgba8(renderer.gpu_renderer(), output_size)?;
    let target = target_pool.acquire(renderer.gpu_renderer())?;
    let mut config = video_config(workload.composition, VideoCodec::H264)?;
    config.mode = EncodeMode::GpuTexture(GpuBackend::Cuda);
    config.cuda_ordinal = Some(target_pool.cuda_ordinal());
    let mut encoder = MuxedEncoder::create(output.to_string_lossy().to_string(), config)?;

    let setup = setup_started.elapsed();
    if phases.is_some() {
        print_plan_profile(renderer.compiled());
    }
    let times = RunTimes::begin(setup);
    for frame in 0..workload.frames {
        let submitted = timed(phases, "render", || {
            renderer.render_frame_into_external(
                workload.composition,
                frame,
                workload.media,
                target.external_texture(),
            )
        })?;
        timed(phases, "poll", || {
            submitted.wait(&renderer.gpu_renderer().device)
        })?;
        timed(phases, "copy_to_cuda", || {
            target.copy_rendered_frame_to_cuda()
        })?;
        let cuda_frame = target.video_frame(Some(i64::from(frame)));
        timed(phases, "encode", || {
            encoder.write_gpu_frame(&target.video_input(&cuda_frame))
        })?;
    }
    timed(phases, "finish", || encoder.finish())?;
    Ok(times.finish())
}

#[cfg(not(all(target_os = "linux", feature = "cuda", feature = "vulkan")))]
async fn vk_cuda_nvenc(
    _workload: &Workload<'_>,
    _output: &Path,
    _phases: &mut Option<&mut PhaseTimes>,
) -> anyhow::Result<RunTimes> {
    Err(anyhow!(
        "vk-cuda-nvenc requires linux + cuda + vulkan features"
    ))
}

async fn renderer(workload: &Workload<'_>) -> anyhow::Result<GpuCompositionRenderer> {
    renderer_with_format(workload, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm).await
}

async fn renderer_with_format(
    workload: &Workload<'_>,
    format: lumen_gpu::wgpu::TextureFormat,
) -> anyhow::Result<GpuCompositionRenderer> {
    let mut renderer = GpuCompositionRenderer::new().await?;
    renderer.set_static_output_reuse(!workload.full_render);
    renderer.compile_with_media(workload.composition, workload.media, format)?;
    Ok(renderer)
}

fn video_config(
    composition: &Composition,
    codec: VideoCodec,
) -> anyhow::Result<VideoEncoderConfig> {
    let mut config = VideoEncoderConfig::cpu_rgba(
        composition.render_settings().width,
        composition.render_settings().height,
        Rational::from_fps(f64::from(composition.timeline().fps))?,
        codec,
    );
    config.bit_rate = 14_000_000;
    Ok(config)
}

#[cfg(any(
    all(target_os = "macos", feature = "metal"),
    all(target_os = "linux", feature = "vulkan")
))]
fn composition_size(composition: &Composition) -> lumen_gpu::Size {
    lumen_gpu::Size::new(
        composition.render_settings().width,
        composition.render_settings().height,
    )
}

fn temp_path(name: &str, extension: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!("lumen_bench_{name}_{unique}.{extension}"))
}
