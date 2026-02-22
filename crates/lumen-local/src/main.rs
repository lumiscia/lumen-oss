mod json_delegate;

use std::{
	env, fs,
	io::Write,
	path::{Component, Path, PathBuf},
	process::{Command, Stdio},
	sync::Arc,
};

use anyhow::{Context, Result, anyhow};
use image::ImageReader;
use json_delegate::{JsonProject, ProjectBundle};
use lumen::{
	Project,
	media::{ImageResolver, MediaStore, VideoResolver},
	render::{context::RendererContext, render_scene},
};

#[derive(Debug)]
struct CliArgs {
	project: PathBuf,
	output: PathBuf,
	media_root: Option<PathBuf>,
	encoder: Option<String>,
	frame: Option<u32>,
}

#[derive(Debug)]
struct LocalMediaStore {
	image_sources: std::collections::HashMap<String, PathBuf>,
	image_cache: std::collections::HashMap<String, ImageFrame>,
}

#[derive(Debug, Clone)]
struct ImageFrame {
	width: u32,
	height: u32,
	pixels_rgba: Arc<Vec<u8>>,
}

#[derive(Debug, Clone)]
struct StaticImageResolver {
	id: String,
	frame: ImageFrame,
}

impl StaticImageResolver {
	fn new(id: String, frame: ImageFrame) -> Self {
		Self { id, frame }
	}
}

impl ImageResolver for StaticImageResolver {
	fn id(&self) -> String {
		self.id.clone()
	}

	fn width(&self) -> u32 {
		self.frame.width
	}

	fn height(&self) -> u32 {
		self.frame.height
	}

	fn resolve(&mut self) -> Vec<u8> {
		(*self.frame.pixels_rgba).clone()
	}
}

impl MediaStore for LocalMediaStore {
	fn get_image_resolver(&mut self, id: &str) -> Option<Box<dyn ImageResolver>> {
		if !self.image_cache.contains_key(id) {
			let source = self.image_sources.get(id)?.clone();
			let frame = load_image_rgba(&source).ok()?;
			self.image_cache.insert(id.to_string(), frame);
		}
		let frame = self.image_cache.get(id)?.clone();
		Some(Box::new(StaticImageResolver::new(id.to_string(), frame)))
	}

	fn get_video_resolver(&mut self, _id: &str) -> Option<Box<dyn VideoResolver>> {
		None
	}
}

fn parse_args() -> Result<CliArgs> {
	let mut args = env::args().skip(1);
	let mut project = None;
	let mut output = None;
	let mut media_root = None;
	let mut encoder = None;
	let mut frame = None;

	while let Some(flag) = args.next() {
		match flag.as_str() {
			"--project" => {
				let value = args
					.next()
					.ok_or_else(|| anyhow!("missing value for --project"))?;
				project = Some(PathBuf::from(value));
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
			unknown => return Err(anyhow!("unknown argument: {unknown}")),
		}
	}

	let project = project.ok_or_else(|| anyhow!("--project is required"))?;
	let output = output.ok_or_else(|| anyhow!("--output is required"))?;

	Ok(CliArgs {
		project,
		output,
		media_root,
		encoder,
		frame,
	})
}

fn print_usage() {
	eprintln!(
		"usage: lumen-local --project <path> --output <path.[png|mp4]> [--media-root <path>] [--encoder <name>] [--frame <n>]"
	)
}

fn main() {
	if let Err(err) = run() {
		eprintln!("lumen-local failed: {err:#}");
		std::process::exit(1);
	}
}

fn run() -> Result<()> {
	let args = parse_args().inspect_err(|_| print_usage())?;
	let raw = fs::read_to_string(&args.project)
		.with_context(|| format!("failed to read project file {}", args.project.display()))?;
	let delegate: JsonProject =
		serde_json::from_str(raw.as_str()).context("failed to parse project JSON delegate")?;
	let bundle: ProjectBundle = delegate.try_into().context("failed to convert project")?;

	let media_root = media_root(args.media_root.as_deref())?;
	let image_sources = resolve_image_sources(&bundle.image_sources, &media_root)?;

	let mut renderer_ctx = RendererContext::new(
		bundle.project.width,
		bundle.project.height,
		bundle.project.frame_rate,
	)
	.map_err(|err| anyhow!(err.to_string()))?;
	renderer_ctx.clear_color = skia_safe::Color::from_argb(
		bundle.background[3],
		bundle.background[0],
		bundle.background[1],
		bundle.background[2],
	);
	renderer_ctx.set_media_store(Box::new(LocalMediaStore {
		image_sources,
		image_cache: std::collections::HashMap::new(),
	}));

	let extension = args
		.output
		.extension()
		.and_then(|value| value.to_str())
		.map(|value| value.to_ascii_lowercase())
		.unwrap_or_default();

	match extension.as_str() {
		"png" => render_single_png(&bundle.project, &args.output, args.frame, &mut renderer_ctx),
		"mp4" => render_mp4(
			&bundle.project,
			&args.output,
			args.encoder.as_deref(),
			&mut renderer_ctx,
		),
		_ => Err(anyhow!(
			"unsupported output extension; use .png or .mp4 (got `{}`)",
			args.output.display()
		)),
	}
}

fn render_single_png(
	project: &Project,
	output: &Path,
	frame_override: Option<u32>,
	renderer_ctx: &mut RendererContext,
) -> Result<()> {
	let frame = frame_override.unwrap_or(0);
	if frame >= project.duration_frames {
		return Err(anyhow!(
			"requested frame {frame} is out of range for duration {}",
			project.duration_frames
		));
	}
	let rgba =
		render_scene(project, frame, renderer_ctx).map_err(|err| anyhow!("render failed: {err}"))?;
	write_png(output, project.width, project.height, rgba)
}

fn render_mp4(
	project: &Project,
	output: &Path,
	override_encoder: Option<&str>,
	renderer_ctx: &mut RendererContext,
) -> Result<()> {
	if project.frame_rate.den == 0 {
		return Err(anyhow!("invalid fps denominator: 0"));
	}
	if let Some(parent) = output.parent() {
		fs::create_dir_all(parent)
			.with_context(|| format!("failed to create output dir {}", parent.display()))?;
	}

	let encoder = choose_video_encoder(override_encoder);
	let mut child = Command::new("ffmpeg")
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
		.arg(format!("{}x{}", project.width, project.height))
		.arg("-r")
		.arg(format!("{}/{}", project.frame_rate.num, project.frame_rate.den))
		.arg("-i")
		.arg("pipe:0")
		.arg("-an")
		.arg("-c:v")
		.arg(&encoder)
		.arg("-pix_fmt")
		.arg("yuv420p")
		.arg("-movflags")
		.arg("+faststart")
		.arg(output)
		.stdin(Stdio::piped())
		.stderr(Stdio::piped())
		.spawn()
		.context("failed to spawn ffmpeg encoder")?;

	{
		let stdin = child
			.stdin
			.as_mut()
			.ok_or_else(|| anyhow!("ffmpeg stdin unavailable"))?;
		for frame in 0..project.duration_frames {
			let rgba = render_scene(project, frame, renderer_ctx)
				.map_err(|err| anyhow!("render failed at frame {frame}: {err}"))?;
			stdin
				.write_all(rgba.as_slice())
				.with_context(|| format!("failed writing frame {frame} to ffmpeg"))?;
			if frame == 0 || frame + 1 == project.duration_frames || frame % 60 == 0 {
				println!(
					"progress frame={}/{}",
					frame + 1,
					project.duration_frames
				);
			}
		}
	}

	let output_result = child
		.wait_with_output()
		.context("failed waiting for ffmpeg encoder")?;
	if !output_result.status.success() {
		return Err(anyhow!(
			"ffmpeg encode failed with encoder `{encoder}`: {}",
			String::from_utf8_lossy(&output_result.stderr)
		));
	}

	println!(
		"render complete output={} frames={}",
		output.display(),
		project.duration_frames
	);
	Ok(())
}

fn write_png(output: &Path, width: u32, height: u32, rgba: Vec<u8>) -> Result<()> {
	let image = image::RgbaImage::from_raw(width, height, rgba)
		.ok_or_else(|| anyhow!("rendered RGBA buffer shape mismatch"))?;
	if let Some(parent) = output.parent() {
		fs::create_dir_all(parent)
			.with_context(|| format!("failed to create output dir {}", parent.display()))?;
	}
	image
		.save(output)
		.with_context(|| format!("failed to write PNG {}", output.display()))
}

fn media_root(override_root: Option<&Path>) -> Result<PathBuf> {
	let root = match override_root {
		Some(path) => path.to_path_buf(),
		None => env::current_dir().context("failed to read current directory")?,
	};
	root.canonicalize()
		.with_context(|| format!("failed to canonicalize media root {}", root.display()))
}

fn resolve_image_sources(
	sources: &std::collections::HashMap<String, String>,
	root: &Path,
) -> Result<std::collections::HashMap<String, PathBuf>> {
	let mut resolved = std::collections::HashMap::new();
	for (id, source) in sources {
		if is_http_url(source) {
			continue;
		}
		let path = resolve_local_path_with_root(source, root)
			.with_context(|| format!("failed resolving image source `{id}` -> `{source}`"))?;
		resolved.insert(id.clone(), path);
	}
	Ok(resolved)
}

fn resolve_local_path_with_root(source: &str, root: &Path) -> Result<PathBuf> {
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
	let candidate = candidate
		.canonicalize()
		.with_context(|| format!("failed to canonicalize asset path `{}`", candidate.display()))?;
	if !candidate.starts_with(root) {
		return Err(anyhow!(
			"asset path escapes allowed media root: `{}`",
			candidate.display()
		));
	}

	Ok(candidate)
}

fn load_image_rgba(path: &Path) -> Result<ImageFrame> {
	let image = ImageReader::open(path)
		.with_context(|| format!("failed to open image `{}`", path.display()))?
		.decode()
		.with_context(|| format!("failed to decode image `{}`", path.display()))?;
	let rgba = image.into_rgba8();
	Ok(ImageFrame {
		width: rgba.width(),
		height: rgba.height(),
		pixels_rgba: Arc::new(rgba.into_raw()),
	})
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

fn is_http_url(source: &str) -> bool {
	source.starts_with("http://") || source.starts_with("https://")
}

#[cfg(test)]
mod tests {
	use super::{choose_video_encoder, resolve_local_path_with_root};

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
}
