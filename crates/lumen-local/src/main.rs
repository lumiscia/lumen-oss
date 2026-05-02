use std::{
    collections::HashMap,
    env, fs,
    io::Write,
    path::{Component, Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, RwLock, mpsc},
    thread::{self, JoinHandle},
    time::Instant,
};

use anyhow::{Context, Result, anyhow};
use image::{ImageEncoder, codecs::png::PngEncoder};
use lumen::{
    audio::{
        AUDIO_CHANNELS, AUDIO_SAMPLE_RATE, AudioMixer, AudioResolver, AudioSourceProvider,
        duration_samples,
    },
    composition::Composition,
    ffmpeg::{FfmpegAudioResolver, FfmpegVideoResolver},
    gpu_image::GpuImageFrame,
    image::ImageFileResolver,
    media::{ImageResolver, MediaStore, VideoFrameResolver},
    render::{
        RenderOrchestrator, RenderOrchestratorConfig,
        surface::{DefaultSurfacePool, SurfacePool},
    },
};

#[derive(Debug)]
struct CliArgs {
    composition: PathBuf,
    output: PathBuf,
    media_root: Option<PathBuf>,
    encoder: Option<String>,
    frame: Option<u32>,
}

const ENCODER_FRAME_QUEUE_CAPACITY: usize = 2;

struct EncoderFrame {
    frame: u32,
    pixels: Vec<u8>,
}

struct FfmpegEncoder {
    frame_tx: Option<mpsc::SyncSender<EncoderFrame>>,
    writer_handle: Option<JoinHandle<Result<()>>>,
}

struct LocalMediaStore {
    root: PathBuf,
    audios: RwLock<HashMap<String, Arc<FfmpegAudioResolver>>>,
    images: RwLock<HashMap<String, Arc<ImageFileResolver>>>,
    videos: RwLock<HashMap<String, Arc<FfmpegVideoResolver>>>,
}

impl std::fmt::Debug for LocalMediaStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalMediaStore")
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

impl LocalMediaStore {
    fn new(root: PathBuf) -> Self {
        Self {
            root,
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
            FfmpegVideoResolver::open(source.to_string())
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

    fn metadata(&self) -> lumen::media::ImageMetadata {
        self.0.metadata()
    }

    fn gpu_image(&self) -> Result<Arc<GpuImageFrame>, lumen::error::MediaError> {
        self.0.gpu_image()
    }
}

#[derive(Clone)]
struct SharedVideoResolver(Arc<FfmpegVideoResolver>);

impl VideoFrameResolver for SharedVideoResolver {
    fn id(&self) -> &str {
        self.0.id()
    }

    fn metadata(&self) -> lumen::media::VideoMetadata {
        self.0.metadata()
    }

    fn enqueue_frame(&self, frame: u32) -> Result<(), lumen::error::MediaError> {
        self.0.enqueue_frame(frame)
    }

    fn frame(&self, frame: u32) -> Result<Arc<GpuImageFrame>, lumen::error::MediaError> {
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

    fn metadata(&self) -> lumen::audio::AudioMetadata {
        self.0.metadata()
    }

    fn resolve_range(
        &self,
        start_sample: u64,
        frames: usize,
    ) -> Result<Arc<lumen::audio::AudioBuffer>, lumen::error::MediaError> {
        self.0.resolve_range(start_sample, frames)
    }
}

fn spawn_ffmpeg_encoder(
    output: &Path,
    width: u32,
    height: u32,
    fps: f32,
    encoder: &str,
    audio_path: Option<&Path>,
) -> Result<Child> {
    let mut command = Command::new("ffmpeg");
    command
        .arg("-y")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-nostdin")
        .arg("-f")
        .arg("rawvideo")
        .arg("-pix_fmt")
        .arg("rgba")
        .arg("-s:v")
        .arg(format!("{width}x{height}"))
        .arg("-r")
        .arg(format!("{fps}"))
        .arg("-i")
        .arg("pipe:0");

    if let Some(audio_path) = audio_path {
        command
            .arg("-f")
            .arg("f32le")
            .arg("-ar")
            .arg(AUDIO_SAMPLE_RATE.to_string())
            .arg("-ac")
            .arg(AUDIO_CHANNELS.to_string())
            .arg("-i")
            .arg(audio_path)
            .arg("-c:a")
            .arg("aac");
    } else {
        command.arg("-an");
    }

    command
        .arg("-c:v")
        .arg(encoder)
        .arg("-pix_fmt")
        .arg("yuv420p")
        .arg("-movflags")
        .arg("+faststart")
        .arg(output)
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to spawn ffmpeg encoder")
}

fn start_ffmpeg_encoder(mut child: Child, total_frames: u32) -> Result<FfmpegEncoder> {
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow!("ffmpeg stdin unavailable"))?;
    let (frame_tx, frame_rx) = mpsc::sync_channel::<EncoderFrame>(ENCODER_FRAME_QUEUE_CAPACITY);
    let writer_handle = thread::spawn(move || {
        while let Ok(encoded_frame) = frame_rx.recv() {
            stdin
                .write_all(encoded_frame.pixels.as_slice())
                .with_context(|| {
                    format!("failed writing frame {} to ffmpeg", encoded_frame.frame)
                })?;

            if encoded_frame.frame == 0
                || encoded_frame.frame + 1 == total_frames
                || encoded_frame.frame % 60 == 0
            {
                println!(
                    "progress frame={}/{}",
                    encoded_frame.frame + 1,
                    total_frames
                );
            }
        }

        drop(stdin);
        finalize_ffmpeg_encoder(child)
    });

    Ok(FfmpegEncoder {
        frame_tx: Some(frame_tx),
        writer_handle: Some(writer_handle),
    })
}

fn finalize_ffmpeg_encoder(child: Child) -> Result<()> {
    let output = child
        .wait_with_output()
        .context("failed waiting for ffmpeg encoder")?;

    if output.status.success() {
        Ok(())
    } else {
        Err(anyhow!(
            "ffmpeg encode failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

impl FfmpegEncoder {
    fn send(&self, frame: EncoderFrame) -> Result<()> {
        self.frame_tx
            .as_ref()
            .ok_or_else(|| anyhow!("ffmpeg encoder writer unavailable"))?
            .send(frame)
            .map_err(|_| anyhow!("ffmpeg encoder writer stopped"))
    }

    fn finish(mut self) -> Result<()> {
        self.frame_tx.take();
        self.writer_handle
            .take()
            .ok_or_else(|| anyhow!("ffmpeg encoder writer thread missing"))?
            .join()
            .map_err(|_| anyhow!("ffmpeg encoder writer thread panicked"))?
    }
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
    let surface_pool = DefaultSurfacePool::new();

    let extension = args
        .output
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_default();

    match extension.as_str() {
        "png" => render_single_png(
            &composition,
            &surface_pool,
            &media_store,
            &args.output,
            args.frame,
        ),
        "mp4" => {
            if args.frame.is_some() {
                return Err(anyhow!("--frame is only supported when output is .png"));
            }
            render_mp4(
                composition,
                surface_pool,
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

    lumen::json::parse(&raw)
        .with_context(|| format!("failed to parse composition {}", path.display()))
}

fn render_single_png(
    composition: &Composition,
    surface_pool: &DefaultSurfacePool,
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

    let orchestrator = RenderOrchestrator::new(
        composition,
        surface_pool,
        media_store,
        RenderOrchestratorConfig { lookahead_count: 8 },
    );

    let rendered = orchestrator
        .render(frame)
        .with_context(|| format!("render failed at frame {frame}"))?;
    surface_pool.flush();
    let (width, height) = rendered.storage_dimensions();
    let mut pixels = vec![0; (width as usize) * (height as usize) * 4];
    rendered
        .read_pixels_into(pixels.as_mut_slice(), (width as usize) * 4)
        .with_context(|| format!("failed reading rendered pixels for frame {frame}"))?;

    write_png(output, width, height, pixels.as_slice())?;

    println!("render complete output={} frame={frame}", output.display());
    Ok(())
}

fn render_mp4(
    composition: Composition,
    surface_pool: DefaultSurfacePool,
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
    let orchestrator = RenderOrchestrator::new(
        &composition,
        &surface_pool,
        &media_store,
        RenderOrchestratorConfig { lookahead_count: 8 },
    );
    let audio_path = if composition
        .audio
        .as_ref()
        .is_some_and(|audio| !audio.clips.is_empty())
    {
        let path = output.with_extension("audio.f32le.tmp");
        render_audio_raw(&composition, &media_store, &path)?;
        Some(path)
    } else {
        None
    };
    let child = spawn_ffmpeg_encoder(
        output,
        width,
        height,
        composition.timeline.fps,
        encoder.as_str(),
        audio_path.as_deref(),
    )?;
    let encoder = start_ffmpeg_encoder(child, total_frames)?;
    let render_started = Instant::now();
    let mut render_readback_ms = 0_u128;

    for frame in 0..total_frames {
        let frame_started = Instant::now();
        let raster = orchestrator
            .render(frame)
            .with_context(|| format!("render failed at frame {frame}"))?;
        surface_pool.flush();
        let (storage_width, storage_height) = raster.storage_dimensions();
        if storage_width != width || storage_height != height {
            return Err(anyhow!(
                "unexpected frame {frame} dimensions {}x{} (expected {}x{})",
                storage_width,
                storage_height,
                width,
                height
            ));
        }
        let mut pixels = vec![0; (width as usize) * (height as usize) * 4];
        raster
            .read_pixels_into(pixels.as_mut_slice(), (width as usize) * 4)
            .with_context(|| format!("failed reading rendered pixels for frame {frame}"))?;
        render_readback_ms = render_readback_ms.saturating_add(frame_started.elapsed().as_millis());
        encoder.send(EncoderFrame { frame, pixels })?;
    }

    encoder.finish()?;
    if let Some(audio_path) = audio_path {
        let _ = fs::remove_file(audio_path);
    }

    let total_ms = render_started.elapsed().as_millis();
    println!(
        "render complete output={} frames={} render_readback_ms={} total_ms={}",
        output.display(),
        total_frames,
        render_readback_ms,
        total_ms,
    );
    Ok(())
}

fn render_audio_raw(
    composition: &Composition,
    media_store: &LocalMediaStore,
    output: &Path,
) -> Result<()> {
    let Some(audio) = composition.audio.as_ref() else {
        return Ok(());
    };

    let total_samples = duration_samples(
        composition.timeline.duration_frames,
        composition.timeline.fps,
    );
    let mixer = AudioMixer::new(audio, media_store);
    let mut file = fs::File::create(output)
        .with_context(|| format!("failed to create audio temp {}", output.display()))?;
    let mut start_sample = 0_u64;
    let chunk_frames = AUDIO_SAMPLE_RATE as usize;

    while start_sample < total_samples {
        let frames = usize::try_from((total_samples - start_sample).min(chunk_frames as u64))
            .unwrap_or(chunk_frames);
        let mixed = mixer
            .mix_range(start_sample, frames)
            .map_err(|err| anyhow!("audio mix failed at sample {start_sample}: {err}"))?;
        for sample in mixed.interleaved_f32() {
            file.write_all(&sample.to_le_bytes())?;
        }
        start_sample = start_sample.saturating_add(frames as u64);
    }

    Ok(())
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
