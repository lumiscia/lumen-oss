#[cfg(all(target_os = "macos", feature = "metal"))]
use std::collections::VecDeque;
use std::{
    collections::HashMap,
    env, fs,
    path::{Component, Path, PathBuf},
    sync::{Arc, RwLock, mpsc},
    thread::{self, JoinHandle},
    time::Instant,
};

use anyhow::{Context, Result, anyhow};
use image::{ImageEncoder, codecs::png::PngEncoder};
#[cfg(all(target_os = "macos", feature = "metal"))]
use lumen_engine::gpu::{MetalVideoToolboxTarget, MetalVideoToolboxTargetPool};
use lumen_engine::{
    audio::{
        AUDIO_CHANNELS, AUDIO_SAMPLE_RATE, AudioMixer, AudioResolver, AudioSourceProvider,
        duration_samples,
    },
    composition::Composition,
    ffmpeg::{FfmpegAudioResolver, FfmpegResolverOptions, FfmpegVideoResolver},
    gpu::GpuCompositionRenderer,
    image::ImageFileResolver,
    media::{ImageResolver, MediaFrame, MediaStore, VideoFrameResolver},
};
#[cfg(all(target_os = "macos", feature = "metal"))]
use lumen_ffmpeg::EncodeMode;
use lumen_ffmpeg::{
    AudioEncoderConfig, AudioFrame, CpuVideoFrame, MuxedEncoder, PixelFormat, SampleFormat,
    VideoCodec, VideoEncoderConfig,
};
use lumen_ffmpeg::{GpuBackend, gpu_texture_encode_support};

#[derive(Debug)]
struct CliArgs {
    composition: PathBuf,
    output: PathBuf,
    media_root: Option<PathBuf>,
    encoder: Option<String>,
    frame: Option<u32>,
}

const ENCODER_FRAME_QUEUE_CAPACITY: usize = 2;
#[cfg(all(target_os = "macos", feature = "metal"))]
const GPU_ENCODER_FRAMES_IN_FLIGHT: usize = 3;

struct EncoderFrame {
    frame: u32,
    pixels: Vec<u8>,
    recycle_tx: mpsc::SyncSender<Vec<u8>>,
}

enum EncoderMessage {
    Video(EncoderFrame),
    Audio(AudioFrame),
}

struct LumenFfmpegEncoder {
    message_tx: Option<mpsc::SyncSender<EncoderMessage>>,
    writer_handle: Option<JoinHandle<Result<()>>>,
}

#[derive(Debug, Default, Clone)]
struct RenderTiming {
    audio_mix_ms: u128,
    bind_ms: u128,
    upload_ms: u128,
    render_ms: u128,
    flush_ms: u128,
    readback_ms: u128,
    encode_send_ms: u128,
    encode_finish_ms: u128,
}

struct LocalMediaStore {
    root: PathBuf,
    video_options: FfmpegResolverOptions,
    audios: RwLock<HashMap<String, Arc<FfmpegAudioResolver>>>,
    images: RwLock<HashMap<String, Arc<ImageFileResolver>>>,
    videos: RwLock<HashMap<String, Arc<FfmpegVideoResolver>>>,
}

impl std::fmt::Debug for LocalMediaStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalMediaStore")
            .field("root", &self.root)
            .field("video_options", &self.video_options)
            .finish_non_exhaustive()
    }
}

impl LocalMediaStore {
    fn new(root: PathBuf) -> Self {
        Self {
            root,
            video_options: video_resolver_options_from_env(),
            audios: RwLock::new(HashMap::new()),
            images: RwLock::new(HashMap::new()),
            videos: RwLock::new(HashMap::new()),
        }
    }

    fn resolve_source(&self, source: &str) -> Option<String> {
        resolve_local_path_with_root(source, &self.root)
            .ok()
            .map(|path| path.to_string_lossy().to_string())
    }

    fn image_resolver(&self, source: &str) -> Option<Arc<ImageFileResolver>> {
        if let Ok(cache) = self.images.read()
            && let Some(resolver) = cache.get(source)
        {
            return Some(Arc::clone(resolver));
        }

        let resolver = Arc::new(
            ImageFileResolver::open(source.to_string())
                .map_err(|error| {
                    eprintln!("failed opening image resolver for {source}: {error}");
                    error
                })
                .ok()?,
        );
        if let Ok(mut cache) = self.images.write() {
            cache
                .entry(source.to_string())
                .or_insert_with(|| Arc::clone(&resolver));
        }
        Some(resolver)
    }

    fn video_resolver(&self, source: &str) -> Option<Arc<FfmpegVideoResolver>> {
        if let Ok(cache) = self.videos.read()
            && let Some(resolver) = cache.get(source)
        {
            return Some(Arc::clone(resolver));
        }

        let resolver = Arc::new(
            FfmpegVideoResolver::open_with_options(source.to_string(), self.video_options)
                .map_err(|error| {
                    eprintln!("failed opening video resolver for {source}: {error}");
                    error
                })
                .ok()?,
        );
        if let Ok(mut cache) = self.videos.write() {
            cache
                .entry(source.to_string())
                .or_insert_with(|| Arc::clone(&resolver));
        }
        Some(resolver)
    }

    fn audio_resolver(&self, source: &str) -> Option<Arc<FfmpegAudioResolver>> {
        if let Ok(cache) = self.audios.read()
            && let Some(resolver) = cache.get(source)
        {
            return Some(Arc::clone(resolver));
        }

        let resolver = Arc::new(
            FfmpegAudioResolver::open(source.to_string())
                .map_err(|error| {
                    eprintln!("failed opening audio resolver for {source}: {error}");
                    error
                })
                .ok()?,
        );
        if let Ok(mut cache) = self.audios.write() {
            cache
                .entry(source.to_string())
                .or_insert_with(|| Arc::clone(&resolver));
        }
        Some(resolver)
    }
}

impl MediaStore for LocalMediaStore {
    fn get_image_resolver(&self, source: &str) -> Option<Box<dyn ImageResolver>> {
        let resolved = self.resolve_source(source)?;
        Some(Box::new(SharedImageResolver(
            self.image_resolver(&resolved)?,
        )))
    }

    fn get_video_resolver(&self, stream_id: &str) -> Option<Box<dyn VideoFrameResolver>> {
        let resolved = self.resolve_source(stream_id)?;
        Some(Box::new(SharedVideoResolver(
            self.video_resolver(&resolved)?,
        )))
    }
}

impl AudioSourceProvider for LocalMediaStore {
    fn get_audio_resolver(&self, source_id: &str) -> Option<Box<dyn AudioResolver>> {
        let resolved = self.resolve_source(source_id)?;
        Some(Box::new(SharedAudioResolver(
            self.audio_resolver(&resolved)?,
        )))
    }
}

#[derive(Clone)]
struct SharedImageResolver(Arc<ImageFileResolver>);

impl ImageResolver for SharedImageResolver {
    fn id(&self) -> &str {
        self.0.id()
    }

    fn metadata(&self) -> lumen_engine::media::ImageMetadata {
        self.0.metadata()
    }

    fn frame(&self) -> Result<MediaFrame, lumen_engine::error::MediaError> {
        self.0.frame()
    }
}

#[derive(Clone)]
struct SharedVideoResolver(Arc<FfmpegVideoResolver>);

impl VideoFrameResolver for SharedVideoResolver {
    fn id(&self) -> &str {
        self.0.id()
    }

    fn metadata(&self) -> lumen_engine::media::VideoMetadata {
        self.0.metadata()
    }

    fn enqueue_frame(&self, frame: u32) -> Result<(), lumen_engine::error::MediaError> {
        self.0.enqueue_frame(frame)
    }

    fn frame(&self, frame: u32) -> Result<MediaFrame, lumen_engine::error::MediaError> {
        self.0.frame(frame)
    }

    fn retain_frames(&self, frames: &[u32]) {
        self.0.retain_frames(frames);
    }
}

#[derive(Clone)]
struct SharedAudioResolver(Arc<FfmpegAudioResolver>);

impl AudioResolver for SharedAudioResolver {
    fn id(&self) -> &str {
        self.0.id()
    }

    fn metadata(&self) -> lumen_engine::audio::AudioMetadata {
        self.0.metadata()
    }

    fn resolve_range(
        &self,
        start_sample: u64,
        frames: usize,
    ) -> Result<Arc<lumen_engine::audio::AudioBuffer>, lumen_engine::error::MediaError> {
        self.0.resolve_range(start_sample, frames)
    }
}

impl LumenFfmpegEncoder {
    fn create(
        output: &Path,
        width: u32,
        height: u32,
        fps: f32,
        encoder: &str,
        include_audio: bool,
    ) -> Result<Self> {
        let codec = match encoder {
            "hevc" | "hevc_videotoolbox" | "libx265" => VideoCodec::Hevc,
            _ => VideoCodec::H264,
        };
        let mut config =
            VideoEncoderConfig::cpu_rgba(width, height, fps.round().max(1.0) as u32, codec);
        config.encoder_name = Some(encoder.to_string());
        config.bit_rate = 14_000_000;
        let audio = include_audio
            .then(|| AudioEncoderConfig::aac(AUDIO_SAMPLE_RATE, AUDIO_CHANNELS as u16));
        let (message_tx, message_rx) =
            mpsc::sync_channel::<EncoderMessage>(ENCODER_FRAME_QUEUE_CAPACITY);
        let (startup_tx, startup_rx) = mpsc::sync_channel::<Result<()>>(1);
        let output = output.to_string_lossy().to_string();
        let writer_handle = thread::spawn(move || {
            let mut encoder = match MuxedEncoder::create_with_audio(output, config, audio) {
                Ok(encoder) => {
                    let _ = startup_tx.send(Ok(()));
                    encoder
                }
                Err(error) => {
                    let _ = startup_tx.send(Err(anyhow!(
                        "lumen-ffmpeg encoder failed to start: {error}"
                    )));
                    return Ok(());
                }
            };
            while let Ok(message) = message_rx.recv() {
                match message {
                    EncoderMessage::Video(frame) => {
                        let frame_index = frame.frame;
                        let cpu_frame = CpuVideoFrame {
                            width,
                            height,
                            stride: (width as usize) * 4,
                            pixel_format: PixelFormat::Rgba8,
                            pts: Some(i64::from(frame_index)),
                            data: frame.pixels,
                        };
                        encoder.write_video_frame(&cpu_frame).map_err(|error| {
                            anyhow!("lumen-ffmpeg encode failed at frame {frame_index}: {error}")
                        })?;
                        recycle_pixels(cpu_frame.data, &frame.recycle_tx);
                    }
                    EncoderMessage::Audio(frame) => encoder
                        .write_audio_frame(&frame)
                        .map_err(|error| anyhow!("lumen-ffmpeg audio encode failed: {error}"))?,
                }
            }
            encoder
                .finish()
                .map_err(|error| anyhow!("lumen-ffmpeg encoder finish failed: {error}"))
        });
        startup_rx
            .recv()
            .map_err(|_| anyhow!("lumen-ffmpeg encoder startup thread stopped"))??;
        Ok(Self {
            message_tx: Some(message_tx),
            writer_handle: Some(writer_handle),
        })
    }

    fn send(&self, frame: EncoderFrame) -> Result<()> {
        self.message_tx
            .as_ref()
            .ok_or_else(|| anyhow!("lumen-ffmpeg encoder writer unavailable"))?
            .send(EncoderMessage::Video(frame))
            .map_err(|_| anyhow!("lumen-ffmpeg encoder writer stopped"))
    }

    fn send_audio(&self, frame: AudioFrame) -> Result<()> {
        self.message_tx
            .as_ref()
            .ok_or_else(|| anyhow!("lumen-ffmpeg encoder writer unavailable"))?
            .send(EncoderMessage::Audio(frame))
            .map_err(|_| anyhow!("lumen-ffmpeg encoder writer stopped"))
    }

    fn finish(mut self) -> Result<()> {
        self.message_tx.take();
        self.writer_handle
            .take()
            .ok_or_else(|| anyhow!("lumen-ffmpeg encoder writer thread missing"))?
            .join()
            .map_err(|_| anyhow!("lumen-ffmpeg encoder writer thread panicked"))?
    }
}

fn recycle_pixels(pixels: Vec<u8>, recycle_tx: &mpsc::SyncSender<Vec<u8>>) {
    let _ = recycle_tx.try_send(pixels);
}

fn parse_args() -> Result<CliArgs> {
    parse_args_from(env::args())
}

fn parse_args_from<I>(args: I) -> Result<CliArgs>
where
    I: IntoIterator<Item = String>,
{
    let mut args = args.into_iter().skip(1);
    let mut composition = None;
    let mut output = None;
    let mut media_root = None;
    let mut encoder = None;
    let mut frame = None;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--composition" => {
                let value = args
                    .next()
                    .ok_or_else(|| anyhow!("missing value for --composition"))?;
                composition = Some(PathBuf::from(value));
            }
            "--output" => {
                let value = args
                    .next()
                    .ok_or_else(|| anyhow!("missing value for --output"))?;
                output = Some(PathBuf::from(value));
            }
            "--media-root" => {
                let value = args
                    .next()
                    .ok_or_else(|| anyhow!("missing value for --media-root"))?;
                media_root = Some(PathBuf::from(value));
            }
            "--encoder" => {
                let value = args
                    .next()
                    .ok_or_else(|| anyhow!("missing value for --encoder"))?;
                encoder = Some(value);
            }
            "--frame" => {
                let value = args
                    .next()
                    .ok_or_else(|| anyhow!("missing value for --frame"))?;
                frame = Some(
                    value
                        .parse::<u32>()
                        .with_context(|| format!("invalid u32 value for --frame: {value}"))?,
                );
            }
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            "--project" => {
                return Err(anyhow!(
                    "--project is no longer supported; use --composition"
                ));
            }
            unknown => return Err(anyhow!("unknown argument: {unknown}")),
        }
    }

    let composition = composition.ok_or_else(|| anyhow!("--composition is required"))?;
    let output = output.ok_or_else(|| anyhow!("--output is required"))?;

    Ok(CliArgs {
        composition,
        output,
        media_root,
        encoder,
        frame,
    })
}

fn print_usage() {
    eprintln!(
        "usage: lumen-local --composition <path> --output <path.[png|mp4]> [--media-root <path>] [--encoder <name>] [--frame <n>]"
    )
}

fn main() {
    if let Err(error) = run() {
        eprintln!("lumen-local failed: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args = parse_args().inspect_err(|_| print_usage())?;
    let composition = load_composition(&args.composition)?;
    let media_root = media_root(args.media_root.as_deref())?;
    let media_store = LocalMediaStore::new(media_root);

    let extension = args
        .output
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_default();

    match extension.as_str() {
        "png" => render_single_png(&composition, &media_store, &args.output, args.frame),
        "mp4" => {
            if args.frame.is_some() {
                return Err(anyhow!("--frame is only supported when output is .png"));
            }
            render_mp4(
                composition,
                media_store,
                &args.output,
                args.encoder.as_deref(),
            )
        }
        _ => Err(anyhow!(
            "unsupported output extension; use .png or .mp4 (got `{}`)",
            args.output.display()
        )),
    }
}

fn load_composition(path: &Path) -> Result<Composition> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read composition file {}", path.display()))?;

    lumen_engine::json::parse(&raw)
        .with_context(|| format!("failed to parse composition {}", path.display()))
}

fn render_single_png(
    composition: &Composition,
    media_store: &LocalMediaStore,
    output: &Path,
    frame_override: Option<u32>,
) -> Result<()> {
    let frame = frame_override.unwrap_or(0);
    if frame >= composition.timeline.duration_frames {
        return Err(anyhow!(
            "requested frame {frame} is out of range for duration {}",
            composition.timeline.duration_frames
        ));
    }

    let mut renderer = pollster::block_on(GpuCompositionRenderer::new())
        .context("failed to create GPU renderer")?;
    renderer
        .compile(composition)
        .context("failed to compile composition")?;
    let rendered = renderer
        .render_frame(composition, frame, media_store)
        .with_context(|| format!("render failed at frame {frame}"))?;
    let size = rendered.domain.storage_size;
    let pixels = read_texture_rgba8(renderer.gpu_renderer(), rendered.texture, size)
        .with_context(|| format!("failed reading rendered pixels for frame {frame}"))?;

    write_png(output, size.width, size.height, pixels.as_slice())?;

    println!("render complete output={} frame={frame}", output.display());
    Ok(())
}

fn render_mp4(
    composition: Composition,
    media_store: LocalMediaStore,
    output: &Path,
    override_encoder: Option<&str>,
) -> Result<()> {
    if composition.timeline.fps <= 0.0 {
        return Err(anyhow!(
            "invalid timeline fps: {}",
            composition.timeline.fps
        ));
    }
    if composition.timeline.duration_frames == 0 {
        return Err(anyhow!(
            "composition duration_frames must be greater than zero"
        ));
    }

    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create output dir {}", parent.display()))?;
    }

    let encoder = choose_video_encoder(override_encoder);
    let width = composition.render_settings.width;
    let height = composition.render_settings.height;
    let total_frames = composition.timeline.duration_frames;
    if encoder == "h264_videotoolbox_gpu" || encoder == "hevc_videotoolbox_gpu" {
        return render_video_gpu_videotoolbox(
            composition,
            media_store,
            output,
            width,
            height,
            total_frames,
            &encoder,
        );
    }
    let mut renderer = pollster::block_on(GpuCompositionRenderer::new())
        .context("failed to create GPU renderer")?;
    renderer
        .compile_with_media(
            &composition,
            &media_store,
            lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
        )
        .context("failed to compile composition")?;
    let mut timings = RenderTiming::default();
    let include_audio = has_audio(&composition);

    report_gpu_encode_capabilities();

    let encoder_sink = LumenFfmpegEncoder::create(
        output,
        width,
        height,
        composition.timeline.fps,
        &encoder,
        include_audio,
    )?;
    let render_started = Instant::now();
    let (pixel_recycle_tx, _pixel_recycle_rx) =
        mpsc::sync_channel::<Vec<u8>>(ENCODER_FRAME_QUEUE_CAPACITY + 2);

    for frame in 0..total_frames {
        let bind_started = Instant::now();
        let bound = renderer
            .bind_frame(&composition, frame, &media_store)
            .with_context(|| format!("frame bind failed at frame {frame}"))?;
        timings.bind_ms = timings
            .bind_ms
            .saturating_add(bind_started.elapsed().as_millis());

        let upload_started = Instant::now();
        renderer
            .upload_bound_frame(&bound)
            .with_context(|| format!("frame upload failed at frame {frame}"))?;
        timings.upload_ms = timings
            .upload_ms
            .saturating_add(upload_started.elapsed().as_millis());

        let render_frame_started = Instant::now();
        let (raster, _submission) = renderer
            .submit_render()
            .with_context(|| format!("render submit failed at frame {frame}"))?;
        timings.render_ms = timings
            .render_ms
            .saturating_add(render_frame_started.elapsed().as_millis());

        let flush_started = Instant::now();
        renderer
            .gpu_renderer()
            .device
            .poll(lumen_gpu::wgpu::PollType::Poll)?;
        timings.flush_ms = timings
            .flush_ms
            .saturating_add(flush_started.elapsed().as_millis());

        let storage_width = raster.domain.storage_size.width;
        let storage_height = raster.domain.storage_size.height;
        if storage_width != width || storage_height != height {
            return Err(anyhow!(
                "unexpected frame {frame} dimensions {}x{} (expected {}x{})",
                storage_width,
                storage_height,
                width,
                height
            ));
        }
        let readback_started = Instant::now();
        let pixels = read_texture_rgba8(
            renderer.gpu_renderer(),
            raster.texture,
            raster.domain.storage_size,
        )
        .with_context(|| format!("failed reading rendered pixels for frame {frame}"))?;
        timings.readback_ms = timings
            .readback_ms
            .saturating_add(readback_started.elapsed().as_millis());

        let encode_send_started = Instant::now();
        encoder_sink.send(EncoderFrame {
            frame,
            pixels,
            recycle_tx: pixel_recycle_tx.clone(),
        })?;
        timings.encode_send_ms = timings
            .encode_send_ms
            .saturating_add(encode_send_started.elapsed().as_millis());

        if frame == 0 || frame + 1 == total_frames || frame % 60 == 0 {
            println!("progress frame={}/{}", frame + 1, total_frames);
        }
    }

    if include_audio {
        let audio_started = Instant::now();
        write_composited_audio(&composition, &media_store, &encoder_sink)?;
        timings.audio_mix_ms = audio_started.elapsed().as_millis();
    }

    let encode_finish_started = Instant::now();
    encoder_sink.finish()?;
    timings.encode_finish_ms = encode_finish_started.elapsed().as_millis();

    let total_ms = render_started.elapsed().as_millis();
    println!(
        "render complete output={} frames={} audio_mix_ms={} bind_ms={} upload_ms={} render_ms={} flush_ms={} readback_ms={} encode_send_ms={} encode_finish_ms={} total_ms={}",
        output.display(),
        total_frames,
        timings.audio_mix_ms,
        timings.bind_ms,
        timings.upload_ms,
        timings.render_ms,
        timings.flush_ms,
        timings.readback_ms,
        timings.encode_send_ms,
        timings.encode_finish_ms,
        total_ms,
    );
    Ok(())
}

#[cfg(all(target_os = "macos", feature = "metal"))]
fn render_video_gpu_videotoolbox(
    composition: Composition,
    media_store: LocalMediaStore,
    output: &Path,
    width: u32,
    height: u32,
    total_frames: u32,
    encoder: &str,
) -> Result<()> {
    let mut renderer = pollster::block_on(GpuCompositionRenderer::new())
        .context("failed to create GPU renderer")?;
    renderer
        .compile_with_media(
            &composition,
            &media_store,
            lumen_gpu::wgpu::TextureFormat::Bgra8Unorm,
        )
        .context("failed to compile composition")?;
    let mut config = VideoEncoderConfig::cpu_rgba(
        width,
        height,
        composition.timeline.fps.round().max(1.0) as u32,
        if encoder.starts_with("hevc") {
            VideoCodec::Hevc
        } else {
            VideoCodec::H264
        },
    );
    config.mode = EncodeMode::GpuTexture(GpuBackend::Metal);
    config.bit_rate = 14_000_000;
    let include_audio = has_audio(&composition);
    let audio =
        include_audio.then(|| AudioEncoderConfig::aac(AUDIO_SAMPLE_RATE, AUDIO_CHANNELS as u16));

    let mut target_pool = MetalVideoToolboxTargetPool::bgra8(
        renderer.gpu_renderer(),
        lumen_gpu::Size::new(width, height),
    )?;
    let mut encoder =
        MuxedEncoder::create_with_audio(output.to_string_lossy().to_string(), config, audio)
            .map_err(|error| anyhow!("lumen-ffmpeg GPU encoder failed to start: {error}"))?;

    let mut timings = RenderTiming::default();
    let render_started = Instant::now();
    let mut pending = VecDeque::<PendingGpuEncodeFrame>::new();
    for frame in 0..total_frames {
        let target = target_pool.acquire(renderer.gpu_renderer(), frame)?;

        let render_frame_started = Instant::now();
        let submitted = renderer
            .render_frame_into_external(
                &composition,
                frame,
                &media_store,
                target.external_texture(),
            )
            .with_context(|| format!("render submit failed at frame {frame}"))?;
        timings.render_ms = timings
            .render_ms
            .saturating_add(render_frame_started.elapsed().as_millis());

        pending.push_back(PendingGpuEncodeFrame {
            frame,
            target,
            submitted,
        });
        if pending.len() >= GPU_ENCODER_FRAMES_IN_FLIGHT {
            encode_ready_gpu_frame(&renderer, &mut encoder, &mut timings, &mut pending)?;
        }

        if frame == 0 || frame + 1 == total_frames || frame % 60 == 0 {
            println!("progress frame={}/{}", frame + 1, total_frames);
        }
    }
    while !pending.is_empty() {
        encode_ready_gpu_frame(&renderer, &mut encoder, &mut timings, &mut pending)?;
    }

    if include_audio {
        let audio_started = Instant::now();
        write_composited_audio_direct(&composition, &media_store, &mut encoder)?;
        timings.audio_mix_ms = audio_started.elapsed().as_millis();
    }

    let encode_finish_started = Instant::now();
    encoder
        .finish()
        .map_err(|error| anyhow!("lumen-ffmpeg GPU encoder finish failed: {error}"))?;
    timings.encode_finish_ms = encode_finish_started.elapsed().as_millis();

    let total_ms = render_started.elapsed().as_millis();
    println!(
        "render complete output={} frames={} audio_mix_ms={} bind_ms={} upload_ms={} render_ms={} flush_ms={} readback_ms=0 encode_send_ms={} encode_finish_ms={} total_ms={}",
        output.display(),
        total_frames,
        timings.audio_mix_ms,
        timings.bind_ms,
        timings.upload_ms,
        timings.render_ms,
        timings.flush_ms,
        timings.encode_send_ms,
        timings.encode_finish_ms,
        total_ms,
    );
    Ok(())
}

#[cfg(all(target_os = "macos", feature = "metal"))]
struct PendingGpuEncodeFrame {
    frame: u32,
    target: MetalVideoToolboxTarget,
    submitted: lumen_gpu::SubmittedExternalTexture,
}

#[cfg(all(target_os = "macos", feature = "metal"))]
fn encode_ready_gpu_frame(
    renderer: &GpuCompositionRenderer,
    encoder: &mut MuxedEncoder,
    timings: &mut RenderTiming,
    pending: &mut VecDeque<PendingGpuEncodeFrame>,
) -> Result<()> {
    let PendingGpuEncodeFrame {
        frame,
        target,
        submitted,
    } = pending
        .pop_front()
        .ok_or_else(|| anyhow!("no pending GPU frame to encode"))?;
    let flush_started = Instant::now();
    submitted.wait(&renderer.gpu_renderer().device)?;
    timings.flush_ms = timings
        .flush_ms
        .saturating_add(flush_started.elapsed().as_millis());

    let encode_send_started = Instant::now();
    encoder
        .write_gpu_frame(&target.video_input())
        .map_err(|error| anyhow!("lumen-ffmpeg GPU encode failed at frame {}: {error}", frame))?;
    timings.encode_send_ms = timings
        .encode_send_ms
        .saturating_add(encode_send_started.elapsed().as_millis());
    Ok(())
}

#[cfg(not(all(target_os = "macos", feature = "metal")))]
fn render_video_gpu_videotoolbox(
    _composition: Composition,
    _media_store: LocalMediaStore,
    _output: &Path,
    _width: u32,
    _height: u32,
    _total_frames: u32,
    _encoder: &str,
) -> Result<()> {
    Err(anyhow!(
        "VideoToolbox GPU texture encode is only available on macOS"
    ))
}

fn write_composited_audio(
    composition: &Composition,
    media_store: &LocalMediaStore,
    encoder: &LumenFfmpegEncoder,
) -> Result<()> {
    let Some(audio) = composition.audio.as_ref() else {
        return Ok(());
    };

    let total_samples = duration_samples(
        composition.timeline.duration_frames,
        composition.timeline.fps,
    );
    let mixer = AudioMixer::new(audio, media_store);
    let mut start_sample = 0_u64;
    let chunk_frames = (AUDIO_SAMPLE_RATE as usize).saturating_mul(30);

    while start_sample < total_samples {
        let frames = usize::try_from((total_samples - start_sample).min(chunk_frames as u64))
            .unwrap_or(chunk_frames);
        let mixed = mixer
            .mix_range(start_sample, frames)
            .map_err(|err| anyhow!("audio mix failed at sample {start_sample}: {err}"))?;
        encoder.send_audio(audio_frame_from_buffer(&mixed, start_sample))?;
        start_sample = start_sample.saturating_add(frames as u64);
    }

    Ok(())
}

#[cfg(all(target_os = "macos", feature = "metal"))]
fn write_composited_audio_direct(
    composition: &Composition,
    media_store: &LocalMediaStore,
    encoder: &mut MuxedEncoder,
) -> Result<()> {
    let Some(audio) = composition.audio.as_ref() else {
        return Ok(());
    };

    let total_samples = duration_samples(
        composition.timeline.duration_frames,
        composition.timeline.fps,
    );
    let mixer = AudioMixer::new(audio, media_store);
    let mut start_sample = 0_u64;
    let chunk_frames = (AUDIO_SAMPLE_RATE as usize).saturating_mul(30);

    while start_sample < total_samples {
        let frames = usize::try_from((total_samples - start_sample).min(chunk_frames as u64))
            .unwrap_or(chunk_frames);
        let mixed = mixer
            .mix_range(start_sample, frames)
            .map_err(|err| anyhow!("audio mix failed at sample {start_sample}: {err}"))?;
        encoder
            .write_audio_frame(&audio_frame_from_buffer(&mixed, start_sample))
            .map_err(|error| anyhow!("lumen-ffmpeg audio encode failed: {error}"))?;
        start_sample = start_sample.saturating_add(frames as u64);
    }

    Ok(())
}

fn has_audio(composition: &Composition) -> bool {
    composition
        .audio
        .as_ref()
        .is_some_and(|audio| !audio.clips.is_empty())
}

fn audio_frame_from_buffer(
    buffer: &lumen_engine::audio::AudioBuffer,
    start_sample: u64,
) -> AudioFrame {
    AudioFrame {
        sample_rate: buffer.sample_rate(),
        channels: buffer.channel_count() as u16,
        sample_format: SampleFormat::F32,
        pts: Some(start_sample as i64),
        samples: buffer.frames(),
        interleaved_f32: buffer.interleaved_f32(),
    }
}

fn report_gpu_encode_capabilities() {
    for backend in [GpuBackend::Metal, GpuBackend::Vulkan] {
        let support = gpu_texture_encode_support(VideoCodec::H264, backend);
        eprintln!(
            "gpu_encode backend={:?} codec={:?} encoder={} available={} direct_texture_path={} reason={}",
            support.backend,
            support.codec,
            support.encoder_name.unwrap_or("none"),
            support.available,
            support.direct_texture_path,
            support.reason.as_deref().unwrap_or("none"),
        );
    }
}

fn read_texture_rgba8(
    renderer: &lumen_gpu::Renderer,
    id: lumen_gpu::TextureId,
    size: lumen_gpu::Size,
) -> Result<Vec<u8>> {
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
            label: Some("lumen-local readback"),
            size: output_size.max(1),
            usage: lumen_gpu::wgpu::BufferUsages::COPY_DST
                | lumen_gpu::wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
    let mut encoder =
        renderer
            .device
            .create_command_encoder(&lumen_gpu::wgpu::CommandEncoderDescriptor {
                label: Some("lumen-local readback encoder"),
            });
    let texture = renderer
        .texture(id)
        .ok_or_else(|| anyhow!("render output texture {id:?} is unavailable"))?;
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
        lumen_gpu::wgpu::Extent3d {
            width: size.width,
            height: size.height,
            depth_or_array_layers: 1,
        },
    );
    renderer.queue.submit([encoder.finish()]);

    let slice = output.slice(..);
    let (tx, rx) = mpsc::channel();
    slice.map_async(lumen_gpu::wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result);
    });
    renderer
        .device
        .poll(lumen_gpu::wgpu::PollType::wait_indefinitely())
        .map_err(|error| anyhow!("GPU readback poll failed: {error}"))?;
    rx.recv()
        .map_err(|_| anyhow!("GPU readback channel closed"))?
        .map_err(|error| anyhow!("GPU readback map failed: {error}"))?;

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

fn write_png(output: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<()> {
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create output dir {}", parent.display()))?;
    }

    let mut png = Vec::new();
    let encoder = PngEncoder::new(&mut png);
    encoder
        .write_image(rgba, width, height, image::ExtendedColorType::Rgba8)
        .with_context(|| format!("failed to encode PNG {}", output.display()))?;

    fs::write(output, png).with_context(|| format!("failed to write PNG {}", output.display()))
}

fn media_root(override_root: Option<&Path>) -> Result<PathBuf> {
    let root = match override_root {
        Some(path) => path.to_path_buf(),
        None => env::current_dir().context("failed to read current directory")?,
    };
    root.canonicalize()
        .with_context(|| format!("failed to canonicalize media root {}", root.display()))
}

fn video_resolver_options_from_env() -> FfmpegResolverOptions {
    FfmpegResolverOptions {
        prefer_hardware_decode: env::var("LUMEN_HARDWARE_DECODE")
            .ok()
            .map(|value| matches_flag(&value))
            .unwrap_or_default(),
    }
}

fn matches_flag(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn resolve_local_path_with_root(source: &str, root: &Path) -> Result<PathBuf> {
    if source.starts_with("http://") || source.starts_with("https://") {
        return Err(anyhow!(
            "remote media source `{source}` is not supported by lumen-local"
        ));
    }

    if source.contains("://") && !source.starts_with("file://") {
        return Err(anyhow!("unsupported URI scheme for `{source}`"));
    }

    let raw_path = source.strip_prefix("file://").unwrap_or(source);
    let path = Path::new(raw_path);

    if path.as_os_str().is_empty() {
        return Err(anyhow!("asset path must not be empty"));
    }

    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(anyhow!(
            "parent traversal is not allowed in asset paths: `{source}`"
        ));
    }

    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };

    let candidate = candidate.canonicalize().with_context(|| {
        format!(
            "failed to canonicalize asset path `{}`",
            candidate.display()
        )
    })?;

    if !candidate.starts_with(root) {
        return Err(anyhow!(
            "asset path escapes allowed media root: `{}`",
            candidate.display()
        ));
    }

    Ok(candidate)
}

fn choose_video_encoder(override_encoder: Option<&str>) -> String {
    if let Some(encoder) = override_encoder {
        let encoder = encoder.trim();
        if !encoder.is_empty() {
            return encoder.to_string();
        }
    }

    if let Ok(encoder) = env::var("LUMEN_VIDEO_ENCODER") {
        let encoder = encoder.trim();
        if !encoder.is_empty() {
            return encoder.to_string();
        }
    }

    if cfg!(target_os = "macos") {
        "h264_videotoolbox".to_string()
    } else {
        "libx264".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::{choose_video_encoder, parse_args_from, resolve_local_path_with_root};

    fn argv(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn parse_args_requires_composition_flag() {
        let error = parse_args_from(argv(&["lumen-local", "--output", "out.png"]))
            .expect_err("missing composition should fail");
        assert!(error.to_string().contains("--composition is required"));
    }

    #[test]
    fn parse_args_rejects_legacy_project_flag() {
        let error = parse_args_from(argv(&[
            "lumen-local",
            "--project",
            "in.json",
            "--output",
            "out.png",
        ]))
        .expect_err("legacy project flag should fail");
        assert!(error.to_string().contains("no longer supported"));
    }

    #[test]
    fn choose_video_encoder_prefers_override() {
        assert_eq!(choose_video_encoder(Some("libx265")), "libx265");
    }

    #[test]
    fn reject_parent_traversal_paths() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let error = resolve_local_path_with_root("../secret.png", tmp.path())
            .expect_err("traversal should fail");
        assert!(error.to_string().contains("parent traversal"));
    }

    #[test]
    fn reject_remote_sources() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let error = resolve_local_path_with_root("https://example.com/image.png", tmp.path())
            .expect_err("remote source should fail");
        assert!(error.to_string().contains("remote media source"));
    }
}
