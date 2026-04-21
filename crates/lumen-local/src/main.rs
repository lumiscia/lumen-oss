use std::{
    env, fs,
    io::Write,
    path::{Component, Path, PathBuf},
    process::{Child, Command, Stdio},
};

use anyhow::{Context, Result, anyhow};
use image::{ImageEncoder, codecs::png::PngEncoder};
use lumen::{
    composition::Composition,
    ffmpeg::FfmpegVideoResolver,
    image::ImageFileResolver,
    media::{ImageResolver, MediaStore, VideoFrameResolver},
    raster::RasterFrame,
    render::{
        LumenRenderer,
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

#[derive(Debug)]
struct LocalMediaStore {
    root: PathBuf,
}

impl LocalMediaStore {
    fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn resolve_source(&self, source: &str) -> Option<String> {
        resolve_local_path_with_root(source, &self.root)
            .ok()
            .map(|path| path.to_string_lossy().to_string())
    }
}

impl MediaStore for LocalMediaStore {
    fn get_image_resolver(&self, source: &str) -> Option<Box<dyn ImageResolver>> {
        let resolved = self.resolve_source(source)?;
        ImageFileResolver::open(resolved)
            .ok()
            .map(|r| Box::new(r) as _)
    }

    fn get_video_resolver(&self, stream_id: &str) -> Option<Box<dyn VideoFrameResolver>> {
        let resolved = self.resolve_source(stream_id)?;
        FfmpegVideoResolver::open(resolved)
            .ok()
            .map(|r| Box::new(r) as _)
    }
}

fn spawn_ffmpeg_encoder(
    output: &Path,
    width: u32,
    height: u32,
    fps: f32,
    encoder: &str,
) -> Result<Child> {
    Command::new("ffmpeg")
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
        .arg("pipe:0")
        .arg("-an")
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

fn write_frame_to_ffmpeg(
    child: &mut Child,
    frame: u32,
    total_frames: u32,
    expected_width: u32,
    expected_height: u32,
    raster: &RasterFrame,
    frame_buffer: &mut [u8],
) -> Result<()> {
    let (storage_width, storage_height) = raster.storage_dimensions();
    if storage_width != expected_width || storage_height != expected_height {
        return Err(anyhow!(
            "unexpected frame {frame} dimensions {}x{} (expected {}x{})",
            storage_width,
            storage_height,
            expected_width,
            expected_height
        ));
    }

    raster
        .read_pixels_into(frame_buffer, (expected_width as usize) * 4)
        .with_context(|| format!("failed reading rendered pixels for frame {frame}"))?;

    let stdin = child
        .stdin
        .as_mut()
        .ok_or_else(|| anyhow!("ffmpeg stdin unavailable"))?;
    stdin
        .write_all(frame_buffer)
        .with_context(|| format!("failed writing frame {frame} to ffmpeg"))?;

    if frame == 0 || frame + 1 == total_frames || frame % 60 == 0 {
        println!("progress frame={}/{}", frame + 1, total_frames);
    }

    Ok(())
}

fn finalize_ffmpeg_encoder(mut child: Child) -> Result<()> {
    let _ = child.stdin.take();
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

    let mut renderer = LumenRenderer::new(composition, surface_pool, media_store)
        .context("failed to create renderer")?;

    let rendered = renderer
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
    let mut renderer = LumenRenderer::new(&composition, &surface_pool, &media_store)
        .context("failed to create renderer")?;
    let mut child = spawn_ffmpeg_encoder(
        output,
        width,
        height,
        composition.timeline.fps,
        encoder.as_str(),
    )?;
    let mut frame_buffer = vec![0; (width as usize) * (height as usize) * 4];

    for frame in 0..total_frames {
        let raster = renderer
            .render(frame)
            .with_context(|| format!("render failed at frame {frame}"))?;
        surface_pool.flush();
        write_frame_to_ffmpeg(
            &mut child,
            frame,
            total_frames,
            width,
            height,
            &raster,
            frame_buffer.as_mut_slice(),
        )?;
    }

    finalize_ffmpeg_encoder(child)?;

    println!(
        "render complete output={} frames={}",
        output.display(),
        total_frames,
    );
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
