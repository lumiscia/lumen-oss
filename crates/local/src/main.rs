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

use anyhow::{Context, Result, anyhow, bail};
use clap::{ArgGroup, Parser};
use image::{ImageEncoder, codecs::png::PngEncoder};
#[cfg(target_os = "linux")]
use lumen_engine::gpu::VulkanVideoTarget;
#[cfg(all(target_os = "macos", feature = "metal"))]
use lumen_engine::gpu::{MetalVideoToolboxTarget, MetalVideoToolboxTargetPool};
use lumen_engine::{
    audio::{AUDIO_CHANNELS, AUDIO_SAMPLE_RATE, AudioMixer, AudioResolver, duration_samples},
    composition::Composition,
    error::MediaError,
    ffmpeg::{FfmpegAudioResolver, FfmpegResolverOptions, FfmpegVideoResolver},
    gpu::GpuCompositionRenderer,
    image::ImageFileResolver,
    media::{FontResolver, ImageResolver, MediaFrame, MediaStore, VideoFrameResolver},
};
#[cfg(any(all(target_os = "macos", feature = "metal"), target_os = "linux"))]
use lumen_ffmpeg::EncodeMode;
#[cfg(any(all(target_os = "macos", feature = "metal"), target_os = "linux"))]
use lumen_ffmpeg::GpuBackend;
use lumen_ffmpeg::{
    AudioEncoderConfig, AudioFrame, CpuVideoFrame, EncoderQuality, MuxedEncoder, PixelFormat,
    RateControl, Rational, VideoEncoderChoice, VideoEncoderConfig,
};
#[cfg(target_os = "linux")]
use lumen_ffmpeg::{EncoderFamily, GpuVideoInput, VulkanDevice};
use lumen_server::render::{FONT_DIR_ENV, FontCatalog, existing_font_dir};
use tracing_subscriber::EnvFilter;

/// Renders a Lumen composition to an .mp4 video, or one frame to a .png image.
#[derive(Debug, Parser)]
#[command(
    name = "lumen-local",
    version,
    args_override_self = true,
    group(ArgGroup::new("rate_control").args(["bitrate", "crf", "qp"])),
    after_help = "\
Media sources in the composition resolve inside --media-root, which defaults to the current \
directory. Fonts come from --font-dir, which must exist, then <media-root>/assets/fonts, then the \
system, matched by the family name inside each font file. A family none of them has warns once \
and renders in the bundled font.

On .mp4 output, --encoder or LUMEN_VIDEO_ENCODER names the encoder, and a named encoder that \
won't open fails the render. With neither set, Linux uses h264_vulkan and falls back to libx264 \
with a warning when it won't open, which includes --crf and --preset, as h264_vulkan supports \
neither. macOS uses h264_videotoolbox. The default is a 14 Mb/s target. --bitrate, --crf, --qp \
and --preset override it where the encoder supports them (--crf: libx264, NVENC; --qp: libx264, \
NVENC, Vulkan; --preset: libx264 ultrafast..placebo, NVENC p1..p7) and fail otherwise.

LUMEN_HARDWARE_DECODE=1 decodes video on the GPU when the hardware supports it. RUST_LOG sets the \
log filter (default lumen_engine=warn,lumen_server=warn)."
)]
struct Cli {
    /// Composition JSON to render
    #[arg(long, value_name = "PATH")]
    composition: PathBuf,

    /// Where to write the render: .mp4 renders every frame, .png renders one
    #[arg(long, value_name = "PATH.[png|mp4]")]
    output: PathBuf,

    /// Directory that media sources must stay inside [default: the current directory]
    #[arg(long, value_name = "PATH")]
    media_root: Option<PathBuf>,

    /// Directory of fonts to look in before the media root's assets/fonts and the system fonts
    #[arg(long, env = FONT_DIR_ENV, value_name = "PATH", value_parser = existing_font_dir)]
    font_dir: Option<PathBuf>,

    /// Video encoder name, defaulting to LUMEN_VIDEO_ENCODER. .mp4 only
    #[arg(long, value_name = "NAME")]
    encoder: Option<String>,

    /// Target bits per second, as 14000000, 14000k or 14M. .mp4 only
    #[arg(long, value_name = "BPS|Nk|NM", value_parser = parse_bitrate)]
    bitrate: Option<u64>,

    /// Constant rate factor. .mp4 only
    #[arg(long, value_name = "N")]
    crf: Option<u8>,

    /// Constant quantizer. .mp4 only
    #[arg(long, value_name = "N")]
    qp: Option<u8>,

    /// Encoder speed preset. .mp4 only
    #[arg(long, value_name = "NAME")]
    preset: Option<String>,

    /// Frame to render [default: 0]. .png only
    #[arg(long, value_name = "N")]
    frame: Option<u32>,
}

enum OutputKind {
    Png,
    Mp4,
}

impl Cli {
    /// An empty `LUMEN_FONT_DIR` counts as unset.
    fn font_dir(&self) -> Option<PathBuf> {
        self.font_dir
            .clone()
            .filter(|dir| !dir.as_os_str().is_empty())
    }

    /// The rate control and preset the command line asked for, or `None` if it asked for neither.
    fn quality(&self) -> Option<EncoderQuality> {
        let rate_control = [
            self.bitrate.map(RateControl::Bitrate),
            self.crf.map(RateControl::Crf),
            self.qp.map(RateControl::Qp),
        ]
        .into_iter()
        .flatten()
        .next();
        (rate_control.is_some() || self.preset.is_some()).then(|| EncoderQuality {
            rate_control: rate_control.unwrap_or_default(),
            preset: self.preset.clone(),
        })
    }

    /// The kind of output asked for, after checking that no flag only the other kind supports was
    /// given. `LUMEN_VIDEO_ENCODER` is a default, so only `--encoder` conflicts with .png.
    fn output_kind(&self) -> Result<OutputKind> {
        let extension = self
            .output
            .extension()
            .and_then(|value| value.to_str())
            .map(|value| value.to_ascii_lowercase())
            .unwrap_or_default();
        match extension.as_str() {
            "png" => {
                if self.encoder.is_some() || self.quality().is_some() {
                    bail!(
                        "--encoder, --bitrate, --crf, --qp and --preset are only supported when output is .mp4"
                    );
                }
                Ok(OutputKind::Png)
            }
            "mp4" => {
                if self.frame.is_some() {
                    bail!("--frame is only supported when output is .png");
                }
                Ok(OutputKind::Mp4)
            }
            _ => bail!(
                "unsupported output extension; use .png or .mp4 (got `{}`)",
                self.output.display()
            ),
        }
    }
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

/// Passes per frame skipped because their output was kept from an earlier frame, out of the
/// passes per frame in the plans rendered.
fn cached_pass_summary(renderer: &GpuCompositionRenderer, frames: u32) -> String {
    let frames = f64::from(frames.max(1));
    format!(
        "static_cached={:.1} passes={:.1}",
        renderer.cached_pass_count() as f64 / frames,
        renderer.submitted_pass_count() as f64 / frames,
    )
}

struct LocalMediaStore {
    root: PathBuf,
    fonts: FontCatalog,
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
    fn new(root: PathBuf, font_dir: Option<PathBuf>) -> Self {
        let fonts = FontCatalog::new(font_dir.as_deref(), &root);

        Self {
            root,
            fonts,
            video_options: video_resolver_options_from_env(),
            audios: RwLock::new(HashMap::new()),
            images: RwLock::new(HashMap::new()),
            videos: RwLock::new(HashMap::new()),
        }
    }

    fn resolve_source(&self, source: &str) -> Result<String, MediaError> {
        resolve_local_path_with_root(source, &self.root)
            .map(|path| path.to_string_lossy().to_string())
            .map_err(|error| MediaError::source_not_found(source, format!("{error:#}")))
    }

    fn image_resolver(&self, source: &str) -> Result<Arc<ImageFileResolver>, MediaError> {
        if let Ok(cache) = self.images.read()
            && let Some(resolver) = cache.get(source)
        {
            return Ok(Arc::clone(resolver));
        }

        let resolver = Arc::new(ImageFileResolver::open(source.to_string())?);
        if let Ok(mut cache) = self.images.write() {
            cache
                .entry(source.to_string())
                .or_insert_with(|| Arc::clone(&resolver));
        }
        Ok(resolver)
    }

    fn video_resolver(&self, source: &str) -> Result<Arc<FfmpegVideoResolver>, MediaError> {
        if let Ok(cache) = self.videos.read()
            && let Some(resolver) = cache.get(source)
        {
            return Ok(Arc::clone(resolver));
        }

        let resolver = Arc::new(FfmpegVideoResolver::open_with_options(
            source.to_string(),
            self.video_options.clone(),
        )?);
        if let Ok(mut cache) = self.videos.write() {
            cache
                .entry(source.to_string())
                .or_insert_with(|| Arc::clone(&resolver));
        }
        Ok(resolver)
    }

    fn audio_resolver(&self, source: &str) -> Result<Arc<FfmpegAudioResolver>, MediaError> {
        if let Ok(cache) = self.audios.read()
            && let Some(resolver) = cache.get(source)
        {
            return Ok(Arc::clone(resolver));
        }

        let resolver = Arc::new(FfmpegAudioResolver::open(source.to_string())?);
        if let Ok(mut cache) = self.audios.write() {
            cache
                .entry(source.to_string())
                .or_insert_with(|| Arc::clone(&resolver));
        }
        Ok(resolver)
    }
}

impl MediaStore for LocalMediaStore {
    fn get_image_resolver(&self, source: &str) -> Result<Box<dyn ImageResolver>, MediaError> {
        let resolved = self.resolve_source(source)?;
        Ok(Box::new(SharedImageResolver(
            self.image_resolver(&resolved)?,
        )))
    }

    fn get_video_resolver(
        &self,
        stream_id: &str,
    ) -> Result<Box<dyn VideoFrameResolver>, MediaError> {
        let resolved = self.resolve_source(stream_id)?;
        Ok(Box::new(SharedVideoResolver(
            self.video_resolver(&resolved)?,
        )))
    }

    fn get_audio_resolver(&self, source_id: &str) -> Result<Box<dyn AudioResolver>, MediaError> {
        let resolved = self.resolve_source(source_id)?;
        Ok(Box::new(SharedAudioResolver(
            self.audio_resolver(&resolved)?,
        )))
    }

    fn font_resolver_id(&self, font_family: &str) -> Option<String> {
        Some(self.fonts.resolve(font_family, || None)?.id().to_string())
    }

    fn get_font_resolver(&self, font_family: &str) -> Option<Box<dyn FontResolver>> {
        Some(Box::new(self.fonts.resolve(font_family, || None)?))
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

    fn frame(&self) -> Result<MediaFrame, MediaError> {
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

    fn enqueue_frame(&self, frame: u32) -> Result<(), MediaError> {
        self.0.enqueue_frame(frame)
    }

    fn frame(&self, frame: u32) -> Result<MediaFrame, MediaError> {
        self.0.frame(frame)
    }

    fn retain_frames(&self, frames: &[u32]) {
        self.0.retain_frames(frames);
    }
}

#[derive(Clone)]
struct SharedAudioResolver(Arc<FfmpegAudioResolver>);

impl AudioResolver for SharedAudioResolver {
    fn resolve_range(
        &self,
        start_sample: u64,
        frames: usize,
    ) -> Result<Arc<lumen_engine::audio::AudioBuffer>, MediaError> {
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
        quality: &EncoderQuality,
        include_audio: bool,
    ) -> Result<Self> {
        let mut config = VideoEncoderConfig::cpu_rgba(
            width,
            height,
            Rational::from_fps(f64::from(fps))?,
            encoder,
        );
        config.quality = quality.clone();
        let audio = include_audio
            .then(|| AudioEncoderConfig::aac(AUDIO_SAMPLE_RATE, AUDIO_CHANNELS as u16));
        let (message_tx, message_rx) =
            mpsc::sync_channel::<EncoderMessage>(ENCODER_FRAME_QUEUE_CAPACITY);
        let (startup_tx, startup_rx) = mpsc::sync_channel::<Result<()>>(1);
        let output = output.to_string_lossy().to_string();
        let writer_handle = thread::spawn(move || {
            let mut encoder = match MuxedEncoder::create_with_audio(output, config, audio) {
                Ok(encoder) => {
                    warn_if_container_cropped(&encoder);
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

fn warn_if_container_cropped(encoder: &MuxedEncoder) {
    if let Some(crop) = encoder.container_crop() {
        eprintln!(
            "warning: the encoder coded {coded_width}x{coded_height} for a {width}x{height} canvas; \
             the container crops it back, but players that ignore the crop (including Chromium) \
             show the padding. Render at {coded_width}x{coded_height} to avoid this.",
            coded_width = crop.coded_width,
            coded_height = crop.coded_height,
            width = crop.width,
            height = crop.height,
        );
    }
}

fn recycle_pixels(pixels: Vec<u8>, recycle_tx: &mpsc::SyncSender<Vec<u8>>) {
    let _ = recycle_tx.try_send(pixels);
}

/// Parses bits per second, with optional `k` or `M` suffixes (`14M` is 14,000,000).
fn parse_bitrate(value: &str) -> Result<u64> {
    let (digits, multiplier) = match value.strip_suffix(['k', 'K']) {
        Some(digits) => (digits, 1_000),
        None => match value.strip_suffix(['m', 'M']) {
            Some(digits) => (digits, 1_000_000),
            None => (value, 1),
        },
    };
    let bitrate = digits
        .parse::<u64>()?
        .checked_mul(multiplier)
        .ok_or_else(|| anyhow!("bitrate is too large"))?;
    if bitrate == 0 {
        return Err(anyhow!("bitrate must be greater than zero"));
    }
    Ok(bitrate)
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("lumen_engine=warn,lumen_server=warn")),
        )
        .with_writer(std::io::stderr)
        .init();
    if let Err(error) = run(Cli::parse()) {
        eprintln!("lumen-local failed: {error:#}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    let kind = cli.output_kind()?;
    let composition = load_composition(&cli.composition)?;
    let media_root = media_root(cli.media_root.as_deref())?;
    let mut media_store = LocalMediaStore::new(media_root, cli.font_dir());

    match kind {
        OutputKind::Png => {
            render_single_png(&composition, &mut media_store, &cli.output, cli.frame)
        }
        OutputKind::Mp4 => {
            let quality = cli.quality().unwrap_or_default();
            render_to_partial(&cli.output, |partial| {
                render_mp4(
                    composition,
                    media_store,
                    partial,
                    cli.encoder.as_deref(),
                    &quality,
                )
            })
        }
    }
}

/// Renders to a new file beside `output` and moves it into place only on success, so a failed render
/// keeps any earlier output. A process killed mid-render leaves the temporary file behind.
fn render_to_partial(output: &Path, render: impl FnOnce(&Path) -> Result<()>) -> Result<()> {
    let destination = resolve_output_destination(output)?;
    let directory = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(directory)
        .with_context(|| format!("failed to create output dir {}", directory.display()))?;

    let existing = fs::metadata(&destination).ok();
    let mut partial = tempfile::Builder::new();
    partial.prefix(".lumen-local-").suffix(".partial.mp4");
    // Without this the rename would leave a new output readable only by its owner, and a render
    // replacing a private output would be readable by others while it runs.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = existing
            .as_ref()
            .map_or(0o666, |existing| existing.permissions().mode() & 0o777);
        partial.permissions(fs::Permissions::from_mode(mode));
    }
    let partial = partial
        .tempfile_in(directory)
        .with_context(|| format!("failed to create a file in {}", directory.display()))?
        .into_temp_path();

    render(&partial)?;

    if let Some(existing) = existing {
        fs::set_permissions(&partial, existing.permissions())
            .with_context(|| format!("failed to set permissions for {}", output.display()))?;
    }
    partial
        .persist(&destination)
        .with_context(|| format!("failed to write output {}", output.display()))?;
    println!("output written to {}", output.display());
    Ok(())
}

/// The file a render to `output` replaces. Renaming onto a symlink would replace the link, so it
/// resolves to the link's target, and anything other than a regular file is refused.
fn resolve_output_destination(output: &Path) -> Result<PathBuf> {
    if fs::symlink_metadata(output).is_err() {
        return Ok(output.to_path_buf());
    }
    let destination = output
        .canonicalize()
        .with_context(|| format!("failed to resolve output path {}", output.display()))?;
    let metadata = fs::metadata(&destination)
        .with_context(|| format!("failed to inspect output path {}", output.display()))?;
    if !metadata.is_file() {
        return Err(anyhow!(
            "output path {} already exists and is not a regular file",
            output.display()
        ));
    }
    Ok(destination)
}

fn load_composition(path: &Path) -> Result<Composition> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read composition file {}", path.display()))?;

    lumen_engine::json::parse(&raw)
        .with_context(|| format!("failed to parse composition {}", path.display()))
}

fn read_texture(
    renderer: &lumen_gpu::Renderer,
    id: lumen_gpu::TextureId,
    size: lumen_gpu::Size,
) -> Result<Vec<u8>> {
    let texture = renderer
        .texture(id)
        .ok_or_else(|| anyhow!("render output texture {id:?} is unavailable"))?;
    renderer.read_texture_rgba8(texture, size)
}

/// Creates a renderer that hardware decode can share a device with, when decode is asked for and
/// the hardware has it.
fn create_renderer(media_store: &mut LocalMediaStore) -> Result<GpuCompositionRenderer> {
    #[cfg(target_os = "linux")]
    if media_store.video_options.prefer_hardware_decode {
        match pollster::block_on(GpuCompositionRenderer::new_vulkan_decode()) {
            Ok((renderer, device)) => {
                media_store.video_options.vulkan_device = Some(device);
                return Ok(renderer);
            }
            Err(error) => eprintln!("renderer has no Vulkan decode device: {error}"),
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = media_store;
    pollster::block_on(GpuCompositionRenderer::new()).context("failed to create GPU renderer")
}

fn render_single_png(
    composition: &Composition,
    media_store: &mut LocalMediaStore,
    output: &Path,
    frame_override: Option<u32>,
) -> Result<()> {
    let frame = frame_override.unwrap_or(0);
    if frame >= composition.timeline().duration_frames {
        return Err(anyhow!(
            "requested frame {frame} is out of range for duration {}",
            composition.timeline().duration_frames
        ));
    }

    let mut renderer = create_renderer(media_store)?;
    renderer
        .compile(composition)
        .context("failed to compile composition")?;
    let rendered = renderer
        .render_frame(composition, frame, &*media_store)
        .with_context(|| format!("render failed at frame {frame}"))?;
    let size = rendered.domain.storage_size;
    let mut pixels = read_texture(renderer.gpu_renderer(), rendered.texture, size)
        .with_context(|| format!("failed reading rendered pixels for frame {frame}"))?;
    unpremultiply_rgba8(&mut pixels);

    write_png(output, size.width, size.height, pixels.as_slice())?;

    println!("render complete output={} frame={frame}", output.display());
    Ok(())
}

fn render_mp4(
    composition: Composition,
    mut media_store: LocalMediaStore,
    output: &Path,
    override_encoder: Option<&str>,
    quality: &EncoderQuality,
) -> Result<()> {
    let choice = VideoEncoderChoice::resolve(override_encoder);
    let width = composition.render_settings().width;
    let height = composition.render_settings().height;
    let total_frames = composition.timeline().duration_frames;
    if choice.name == "h264_videotoolbox_gpu" || choice.name == "hevc_videotoolbox_gpu" {
        return render_video_gpu_videotoolbox(
            composition,
            media_store,
            output,
            width,
            height,
            total_frames,
            &choice.name,
            quality,
        );
    }
    let (choice, encoder_info) = choice.look_up(|warning| eprintln!("warning: {warning}"))?;
    #[cfg(not(target_os = "linux"))]
    let _ = encoder_info;
    #[cfg(target_os = "linux")]
    let choice = if encoder_info.family == EncoderFamily::Vulkan {
        match start_vulkan_video_encode(&composition, output, &choice.name, quality) {
            Ok(session) => {
                println!("video_encoder name={} path=zero_copy", choice.name);
                return render_video_gpu_vulkan(composition, media_store, total_frames, session);
            }
            Err(error) => match choice.fall_back(&error) {
                Some((fallback, warning)) => {
                    eprintln!("warning: {warning}");
                    fallback
                }
                None => {
                    eprintln!(
                        "vulkan zero-copy encode unavailable, uploading frames from the CPU instead: {error}"
                    );
                    choice
                }
            },
        }
    } else {
        choice
    };
    let mut renderer = create_renderer(&mut media_store)?;
    renderer
        .compile_with_media(
            &composition,
            &media_store,
            lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
        )
        .context("failed to compile composition")?;
    let mut timings = RenderTiming::default();
    let include_audio = has_audio(&composition);

    let (encoder_sink, encoder) = choice.open(
        |encoder| {
            LumenFfmpegEncoder::create(
                output,
                width,
                height,
                composition.timeline().fps,
                encoder,
                quality,
                include_audio,
            )
        },
        |warning| eprintln!("warning: {warning}"),
    )?;
    println!("video_encoder name={encoder} path=cpu_upload");
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
        let pixels = read_texture(
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
        "render complete frames={} audio_mix_ms={} bind_ms={} upload_ms={} render_ms={} flush_ms={} readback_ms={} encode_send_ms={} encode_finish_ms={} total_ms={} plan_bytes={} {}",
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
        renderer.plan_cache_bytes(),
        cached_pass_summary(&renderer, total_frames),
    );
    Ok(())
}

#[cfg(all(target_os = "macos", feature = "metal"))]
#[allow(clippy::too_many_arguments)]
fn render_video_gpu_videotoolbox(
    composition: Composition,
    media_store: LocalMediaStore,
    output: &Path,
    width: u32,
    height: u32,
    total_frames: u32,
    encoder: &str,
    quality: &EncoderQuality,
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
        Rational::from_fps(f64::from(composition.timeline().fps))?,
        encoder.trim_end_matches("_gpu"),
    );
    config.mode = EncodeMode::GpuTexture(GpuBackend::Metal);
    config.quality = quality.clone();
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
        "render complete frames={} audio_mix_ms={} bind_ms={} upload_ms={} render_ms={} flush_ms={} readback_ms=0 encode_send_ms={} encode_finish_ms={} total_ms={} plan_bytes={} {}",
        total_frames,
        timings.audio_mix_ms,
        timings.bind_ms,
        timings.upload_ms,
        timings.render_ms,
        timings.flush_ms,
        timings.encode_send_ms,
        timings.encode_finish_ms,
        total_ms,
        renderer.plan_cache_bytes(),
        cached_pass_summary(&renderer, total_frames),
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
#[allow(clippy::too_many_arguments)]
fn render_video_gpu_videotoolbox(
    _composition: Composition,
    _media_store: LocalMediaStore,
    _output: &Path,
    _width: u32,
    _height: u32,
    _total_frames: u32,
    _encoder: &str,
    _quality: &EncoderQuality,
) -> Result<()> {
    Err(anyhow!(
        "VideoToolbox GPU texture encode is only available on macOS"
    ))
}

#[cfg(target_os = "linux")]
struct VulkanVideoSession {
    renderer: GpuCompositionRenderer,
    device: VulkanDevice,
    target: VulkanVideoTarget,
    encoder: MuxedEncoder,
}

/// Renders and encodes on one Vulkan device, so frames never leave the GPU.
#[cfg(target_os = "linux")]
fn start_vulkan_video_encode(
    composition: &Composition,
    output: &Path,
    encoder: &str,
    quality: &EncoderQuality,
) -> Result<VulkanVideoSession> {
    let (width, height) = (
        composition.render_settings().width,
        composition.render_settings().height,
    );
    if !width.is_multiple_of(2) || !height.is_multiple_of(2) {
        return Err(anyhow!(
            "NV12 frames need even dimensions, got {width}x{height}"
        ));
    }
    let (renderer, device) = pollster::block_on(GpuCompositionRenderer::new_vulkan_video())?;
    let target =
        VulkanVideoTarget::nv12(renderer.gpu_renderer(), lumen_gpu::Size::new(width, height));
    let mut config = VideoEncoderConfig::cpu_rgba(
        width,
        height,
        Rational::from_fps(f64::from(composition.timeline().fps))?,
        encoder,
    );
    config.mode = EncodeMode::GpuTexture(GpuBackend::Vulkan);
    config.vulkan_device = Some(device.clone());
    config.quality = quality.clone();
    let audio = has_audio(composition)
        .then(|| AudioEncoderConfig::aac(AUDIO_SAMPLE_RATE, AUDIO_CHANNELS as u16));
    let encoder =
        MuxedEncoder::create_with_audio(output.to_string_lossy().to_string(), config, audio)
            .map_err(|error| anyhow!("lumen-ffmpeg Vulkan encoder failed to start: {error}"))?;
    warn_if_container_cropped(&encoder);
    Ok(VulkanVideoSession {
        renderer,
        device,
        target,
        encoder,
    })
}

#[cfg(target_os = "linux")]
fn render_video_gpu_vulkan(
    composition: Composition,
    mut media_store: LocalMediaStore,
    total_frames: u32,
    session: VulkanVideoSession,
) -> Result<()> {
    let VulkanVideoSession {
        mut renderer,
        device,
        target,
        mut encoder,
    } = session;
    media_store.video_options.vulkan_device = device.can_decode().then_some(device);
    renderer
        .compile_with_media(
            &composition,
            &media_store,
            lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
        )
        .context("failed to compile composition")?;
    let (width, height) = (
        composition.render_settings().width,
        composition.render_settings().height,
    );

    let mut timings = RenderTiming::default();
    let render_started = Instant::now();
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
        let size = raster.domain.storage_size;
        if (size.width, size.height) != (width, height) {
            return Err(anyhow!(
                "unexpected frame {frame} dimensions {}x{} (expected {width}x{height})",
                size.width,
                size.height,
            ));
        }
        let texture = renderer
            .gpu_renderer()
            .texture(raster.texture)
            .ok_or_else(|| anyhow!("render output texture {:?} is unavailable", raster.texture))?;
        let video_frame = target
            .convert(renderer.gpu_renderer(), texture, Some(i64::from(frame)))
            .with_context(|| format!("NV12 conversion failed at frame {frame}"))?;
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

        let encode_send_started = Instant::now();
        encoder
            .write_gpu_frame(&GpuVideoInput::Vulkan(&video_frame))
            .map_err(|error| {
                anyhow!("lumen-ffmpeg Vulkan encode failed at frame {frame}: {error}")
            })?;
        timings.encode_send_ms = timings
            .encode_send_ms
            .saturating_add(encode_send_started.elapsed().as_millis());

        if frame == 0 || frame + 1 == total_frames || frame % 60 == 0 {
            println!("progress frame={}/{}", frame + 1, total_frames);
        }
    }

    if has_audio(&composition) {
        let audio_started = Instant::now();
        write_composited_audio_direct(&composition, &media_store, &mut encoder)?;
        timings.audio_mix_ms = audio_started.elapsed().as_millis();
    }

    let encode_finish_started = Instant::now();
    encoder
        .finish()
        .map_err(|error| anyhow!("lumen-ffmpeg Vulkan encoder finish failed: {error}"))?;
    timings.encode_finish_ms = encode_finish_started.elapsed().as_millis();

    let total_ms = render_started.elapsed().as_millis();
    println!(
        "render complete frames={} audio_mix_ms={} bind_ms={} upload_ms={} render_ms={} flush_ms={} readback_ms=0 encode_send_ms={} encode_finish_ms={} total_ms={} plan_bytes={} {}",
        total_frames,
        timings.audio_mix_ms,
        timings.bind_ms,
        timings.upload_ms,
        timings.render_ms,
        timings.flush_ms,
        timings.encode_send_ms,
        timings.encode_finish_ms,
        total_ms,
        renderer.plan_cache_bytes(),
        cached_pass_summary(&renderer, total_frames),
    );
    Ok(())
}

fn write_composited_audio(
    composition: &Composition,
    media_store: &LocalMediaStore,
    encoder: &LumenFfmpegEncoder,
) -> Result<()> {
    let Some(audio) = composition.audio() else {
        return Ok(());
    };

    let total_samples = duration_samples(
        composition.timeline().duration_frames,
        composition.timeline().fps,
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
        encoder.send_audio(audio_frame_from_buffer(&mixed))?;
        start_sample = start_sample.saturating_add(frames as u64);
    }

    Ok(())
}

#[cfg(any(all(target_os = "macos", feature = "metal"), target_os = "linux"))]
fn write_composited_audio_direct(
    composition: &Composition,
    media_store: &LocalMediaStore,
    encoder: &mut MuxedEncoder,
) -> Result<()> {
    let Some(audio) = composition.audio() else {
        return Ok(());
    };

    let total_samples = duration_samples(
        composition.timeline().duration_frames,
        composition.timeline().fps,
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
            .write_audio_frame(&audio_frame_from_buffer(&mixed))
            .map_err(|error| anyhow!("lumen-ffmpeg audio encode failed: {error}"))?;
        start_sample = start_sample.saturating_add(frames as u64);
    }

    Ok(())
}

fn has_audio(composition: &Composition) -> bool {
    composition
        .audio()
        .as_ref()
        .is_some_and(|audio| !audio.clips.is_empty())
}

fn audio_frame_from_buffer(buffer: &lumen_engine::audio::AudioBuffer) -> AudioFrame {
    AudioFrame {
        sample_rate: AUDIO_SAMPLE_RATE,
        channels: buffer.channel_count() as u16,
        samples: buffer.frames(),
        interleaved_f32: buffer.interleaved_f32(),
    }
}

/// PNG holds straight alpha, so colour is divided back out of premultiplied pixels. Fully
/// transparent pixels stay zero.
fn unpremultiply_rgba8(pixels: &mut [u8]) {
    for pixel in pixels.as_chunks_mut::<4>().0 {
        let alpha = u32::from(pixel[3]);
        if alpha == 0 || alpha == 255 {
            continue;
        }
        for channel in &mut pixel[..3] {
            *channel = ((u32::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8;
        }
    }
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
    FfmpegResolverOptions::with_hardware_decode(
        env::var("LUMEN_HARDWARE_DECODE")
            .ok()
            .map(|value| matches_flag(&value))
            .unwrap_or_default(),
    )
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

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::os::unix::fs::FileTypeExt;
    use std::{fs, path::Path};

    use clap::{CommandFactory, Parser};
    use lumen_engine::media::MediaStore;

    use super::{
        Cli, EncoderQuality, LocalMediaStore, RateControl, parse_bitrate, render_to_partial,
        resolve_local_path_with_root,
    };

    #[test]
    fn failed_renders_leave_no_partial_file_and_keep_earlier_output() {
        let dir = tempfile::tempdir().expect("temp dir");
        let output = dir.path().join("out.mp4");
        let file_names = || {
            let mut names = fs::read_dir(dir.path())
                .expect("read dir")
                .map(|entry| entry.expect("entry").file_name())
                .collect::<Vec<_>>();
            names.sort();
            names
        };

        let failed = render_to_partial(&output, |partial| {
            fs::write(partial, b"partial")?;
            Err(anyhow::anyhow!("render failed"))
        });
        assert!(failed.is_err());
        assert!(file_names().is_empty(), "{:?}", file_names());

        render_to_partial(&output, |partial| Ok(fs::write(partial, b"first")?)).expect("render");
        assert_eq!(fs::read(&output).expect("output"), b"first");

        let failed = render_to_partial(&output, |partial| {
            fs::write(partial, b"partial")?;
            Err(anyhow::anyhow!("render failed"))
        });
        assert!(failed.is_err());
        assert_eq!(fs::read(&output).expect("output"), b"first");
        assert_eq!(file_names().len(), 1, "{:?}", file_names());
    }

    #[cfg(unix)]
    #[test]
    fn renders_leave_files_named_like_the_temporary_file_alone() {
        let dir = tempfile::tempdir().expect("temp dir");
        let output = dir.path().join("out.mp4");
        let bystander = dir.path().join(".out.partial.mp4");
        let fail = |partial: &Path| -> anyhow::Result<()> {
            fs::write(partial, b"partial")?;
            Err(anyhow::anyhow!("render failed"))
        };

        fs::write(&bystander, b"mine").expect("write bystander");
        assert!(render_to_partial(&output, fail).is_err());
        assert_eq!(fs::read(&bystander).expect("bystander"), b"mine");
        fs::remove_file(&bystander).expect("remove bystander");

        fs::write(&output, b"first").expect("write output");
        std::os::unix::fs::symlink(&output, &bystander).expect("link bystander");
        assert!(render_to_partial(&output, fail).is_err());
        assert_eq!(fs::read(&output).expect("output"), b"first");
    }

    #[test]
    fn concurrent_renders_reserve_their_own_temporary_files() {
        let dir = tempfile::tempdir().expect("temp dir");
        let output = dir.path().join("out.mp4");
        render_to_partial(&output, |outer| {
            assert!(outer.is_file(), "the path is reserved before rendering");
            assert_eq!(outer.extension().expect("extension"), "mp4");
            render_to_partial(&output, |inner| {
                assert_ne!(outer, inner);
                Ok(fs::write(inner, b"inner")?)
            })?;
            Ok(fs::write(outer, b"outer")?)
        })
        .expect("render");
        assert_eq!(fs::read(&output).expect("output"), b"outer");
        assert_eq!(fs::read_dir(dir.path()).expect("read dir").count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn non_regular_outputs_are_refused_before_rendering() {
        let dir = tempfile::tempdir().expect("temp dir");
        let fifo = dir.path().join("fifo.mp4");
        let status = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("run mkfifo");
        assert!(status.success());
        let dangling = dir.path().join("dangling.mp4");
        std::os::unix::fs::symlink(dir.path().join("missing.mp4"), &dangling).expect("link");

        for output in [fifo.as_path(), dangling.as_path(), dir.path()] {
            render_to_partial(output, |_| panic!("must not render {output:?}"))
                .expect_err("must refuse");
        }
        let error = render_to_partial(&fifo, |_| Ok(())).expect_err("must refuse");
        assert!(error.to_string().contains("not a regular file"), "{error}");
        assert!(
            fs::symlink_metadata(&fifo)
                .expect("fifo")
                .file_type()
                .is_fifo()
        );
    }

    #[cfg(unix)]
    #[test]
    fn replacing_an_output_keeps_its_mode_and_symlink() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("temp dir");
        let mode = |path: &Path| fs::metadata(path).expect("metadata").permissions().mode();
        let render = |partial: &Path| -> anyhow::Result<()> { Ok(fs::write(partial, b"new")?) };

        let reference = dir.path().join("reference");
        fs::write(&reference, b"").expect("write reference");
        let fresh = dir.path().join("fresh.mp4");
        render_to_partial(&fresh, render).expect("render");
        assert_eq!(mode(&fresh), mode(&reference));

        let restricted = dir.path().join("restricted.mp4");
        fs::write(&restricted, b"old").expect("write output");
        fs::set_permissions(&restricted, fs::Permissions::from_mode(0o640)).expect("chmod");
        render_to_partial(&restricted, render).expect("render");
        assert_eq!(mode(&restricted) & 0o777, 0o640);

        let private = dir.path().join("private.mp4");
        fs::write(&private, b"old").expect("write output");
        fs::set_permissions(&private, fs::Permissions::from_mode(0o600)).expect("chmod");
        render_to_partial(&private, |partial| {
            assert_eq!(mode(partial) & 0o777, 0o600);
            render(partial)
        })
        .expect("render");

        let target = dir.path().join("target.mp4");
        let link = dir.path().join("link.mp4");
        fs::write(&target, b"old").expect("write target");
        std::os::unix::fs::symlink(&target, &link).expect("link");
        render_to_partial(&link, render).expect("render");
        assert!(
            fs::symlink_metadata(&link)
                .expect("link")
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read(&target).expect("target"), b"new");
    }

    #[test]
    fn unpremultiply_restores_straight_colour() {
        let mut pixels = [
            64, 32, 0, 128, // half-covered colour
            0, 0, 0, 0, // transparent stays zero
            10, 20, 30, 255, // opaque is unchanged
        ];
        super::unpremultiply_rgba8(&mut pixels);
        assert_eq!(pixels, [128, 64, 0, 128, 0, 0, 0, 0, 10, 20, 30, 255]);
    }

    fn parse(flags: &[&str]) -> Result<Cli, clap::Error> {
        let base = ["lumen-local", "--composition", "in.json"];
        Cli::try_parse_from(base.into_iter().chain(flags.iter().copied()))
    }

    #[test]
    fn cli_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn rate_control_flags_conflict_and_validate() {
        let quality = |flags: &[&str]| parse(flags).unwrap().quality().unwrap();
        let output = ["--output", "o.mp4"];
        assert_eq!(
            quality(&[&output[..], &["--crf", "23", "--preset", "fast"]].concat()),
            EncoderQuality {
                rate_control: RateControl::Crf(23),
                preset: Some("fast".into()),
            }
        );
        assert_eq!(
            quality(&[&output[..], &["--bitrate", "8M"]].concat()).rate_control,
            RateControl::Bitrate(8_000_000)
        );
        assert_eq!(
            quality(&[&output[..], &["--qp", "30"]].concat()).rate_control,
            RateControl::Qp(30)
        );

        for pair in [
            ["--crf", "--qp"],
            ["--bitrate", "--crf"],
            ["--bitrate", "--qp"],
        ] {
            let value = |flag: &str| if flag == "--bitrate" { "1M" } else { "20" };
            let error = parse(&[
                "--output",
                "o.mp4",
                pair[0],
                value(pair[0]),
                pair[1],
                value(pair[1]),
            ])
            .unwrap_err();
            assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
        }
        assert!(parse(&["--output", "o.mp4", "--crf", "300"]).is_err());
        assert!(parse(&["--output", "o.mp4", "--bitrate", "0"]).is_err());
    }

    #[test]
    fn output_kind_rejects_flags_the_output_cannot_use() {
        let kind = |flags: &[&str]| parse(flags).unwrap().output_kind();
        assert!(kind(&["--output", "o.mp4", "--crf", "23"]).is_ok());
        assert!(kind(&["--output", "o.png", "--frame", "3"]).is_ok());

        let error = kind(&["--output", "o.png", "--encoder", "libx264"])
            .err()
            .unwrap();
        assert!(
            error
                .to_string()
                .contains("only supported when output is .mp4")
        );
        for quality in [["--bitrate", "14M"], ["--crf", "23"], ["--preset", "fast"]] {
            let error = kind(&["--output", "o.png", quality[0], quality[1]])
                .err()
                .unwrap();
            assert!(
                error
                    .to_string()
                    .contains("only supported when output is .mp4"),
                "{quality:?}"
            );
        }
        let error = kind(&["--output", "o.mp4", "--frame", "3"]).err().unwrap();
        assert!(
            error
                .to_string()
                .contains("only supported when output is .png")
        );
        let error = kind(&["--output", "o.gif"]).err().unwrap();
        assert!(error.to_string().contains("unsupported output extension"));
    }

    /// Runs `parsed_font_dir` in a child process, since clap reads `LUMEN_FONT_DIR` from the real
    /// environment and a test must not change that for the others.
    fn font_dir_with(env_dir: Option<&str>, flags: &[&str]) -> String {
        let mut child = std::process::Command::new(std::env::current_exe().expect("test binary"));
        child
            .args(["--exact", "tests::parsed_font_dir", "--nocapture"])
            .env("LUMEN_TEST_FONT_FLAGS", flags.join(" "))
            .env_remove("LUMEN_FONT_DIR");
        if let Some(env_dir) = env_dir {
            child.env("LUMEN_FONT_DIR", env_dir);
        }
        let output = child.output().expect("run test binary");
        assert!(output.status.success());
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .find_map(|line| line.strip_prefix("font_dir=").map(str::to_string))
            .expect("child output")
    }

    #[test]
    fn parsed_font_dir() {
        let Ok(flags) = std::env::var("LUMEN_TEST_FONT_FLAGS") else {
            return;
        };
        let cli = parse(&flags.split_whitespace().collect::<Vec<_>>()).unwrap();
        println!("font_dir={:?}", cli.font_dir());
    }

    #[test]
    fn cli_font_directory_takes_precedence_over_environment() {
        let (cli_dir, environment_dir) =
            (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (cli_dir, environment_dir) = (
            cli_dir.path().to_str().unwrap(),
            environment_dir.path().to_str().unwrap(),
        );
        let output = ["--output", "o.png"];
        let with = |env_dir, flags: &[&str]| font_dir_with(env_dir, &[&output[..], flags].concat());
        assert_eq!(
            with(Some(environment_dir), &["--font-dir", cli_dir]),
            format!("Some({cli_dir:?})")
        );
        assert_eq!(
            with(Some(environment_dir), &[]),
            format!("Some({environment_dir:?})")
        );
        assert_eq!(with(Some(""), &[]), "None");
        assert_eq!(with(None, &[]), "None");
    }

    #[test]
    fn a_font_directory_that_does_not_exist_fails_the_parse() {
        let error = parse(&["--output", "o.png", "--font-dir", "no-such-fonts"])
            .err()
            .unwrap();
        assert!(
            error
                .to_string()
                .contains("`no-such-fonts` is not a directory")
        );
    }

    #[test]
    fn parse_bitrate_accepts_suffixes() {
        assert_eq!(parse_bitrate("14000000").unwrap(), 14_000_000);
        assert_eq!(parse_bitrate("500k").unwrap(), 500_000);
        assert_eq!(parse_bitrate("14M").unwrap(), 14_000_000);
        assert!(parse_bitrate("0").is_err());
        assert!(parse_bitrate("fast").is_err());
        assert!(parse_bitrate("18446744073709551615M").is_err());
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

    #[test]
    fn media_lookups_report_why_a_source_failed() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().canonicalize().expect("root");
        fs::write(root.join("corrupt.png"), b"not a png").expect("fixture");
        let store = LocalMediaStore::new(root.clone(), None);
        let failure = |source: &str| match store.get_image_resolver(source) {
            Ok(_) => panic!("{source} should not open"),
            Err(error) => error.to_string(),
        };

        assert_eq!(
            failure("missing.png"),
            format!(
                "media source `missing.png` not found: failed to canonicalize asset path `{}`: No such file or directory (os error 2)",
                root.join("missing.png").display()
            )
        );
        assert_eq!(
            failure("corrupt.png"),
            format!(
                "media decoding failed for {}: failed decoding image source: The image format could not be determined",
                root.join("corrupt.png").display()
            )
        );
    }
}
