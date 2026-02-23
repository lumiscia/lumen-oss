use std::{
    collections::HashMap,
    env, fs,
    io::Write,
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    sync::Arc,
};

use anyhow::{Context, Result, anyhow};
use image::ImageReader;
use lumen::{
    Project,
    json::{JsonDelegateRequest, JsonDelegateStatus, ProjectBundle, convert_json_delegate},
    media::{ImageResolver, MediaStore, VideoResolver},
    render::{context::RendererContext, render_scene},
};
use serde_json::{Map, Value};

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
    image_sources: HashMap<String, PathBuf>,
    image_cache: HashMap<String, ImageFrame>,
    video_sources: HashMap<String, VideoSource>,
}

#[derive(Debug, Clone)]
struct ImageFrame {
    width: u32,
    height: u32,
    pixels_rgba: Arc<Vec<u8>>,
}

#[derive(Debug, Clone)]
struct VideoSource {
    id: String,
    path: PathBuf,
    width: u32,
    height: u32,
}

#[derive(Debug, Clone)]
struct StaticImageResolver {
    id: String,
    frame: ImageFrame,
}

#[derive(Debug, Clone)]
struct StaticVideoResolver {
    source: VideoSource,
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

impl VideoResolver for StaticVideoResolver {
    fn id(&self) -> String {
        self.source.id.clone()
    }

    fn width(&self) -> u32 {
        self.source.width
    }

    fn height(&self) -> u32 {
        self.source.height
    }

    fn resolve_frame(&mut self, frame: u32) -> Vec<u8> {
        decode_video_frame_rgba(
            &self.source.path,
            self.source.width,
            self.source.height,
            frame,
        )
        .unwrap_or_else(|_| {
            rgba_byte_len(self.source.width, self.source.height)
                .map(|len| vec![0; len])
                .unwrap_or_default()
        })
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

    fn get_video_resolver(&mut self, id: &str) -> Option<Box<dyn VideoResolver>> {
        let source = self.video_sources.get(id)?.clone();
        Some(Box::new(StaticVideoResolver { source }))
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
    let normalized_payload = normalize_delegate_payload(raw.as_str())?;

    let delegate_request = JsonDelegateRequest {
        input_payload: normalized_payload.clone(),
        input_schema_revision: "chat_story_v1".to_string(),
        caller_context: "lumen-local".to_string(),
    };
    let delegate_result = convert_json_delegate(&delegate_request);
    let bundle: ProjectBundle = match delegate_result.status {
        JsonDelegateStatus::Success => delegate_result
            .project_bundle
            .ok_or_else(|| anyhow!("delegate returned success without project bundle"))?,
        JsonDelegateStatus::CapabilityDisabled => {
            return Err(anyhow!(
                "json delegate capability is disabled; rebuild lumen with the `json` feature"
            ));
        }
        JsonDelegateStatus::ValidationError | JsonDelegateStatus::ConversionError => {
            let detail = delegate_result
                .errors
                .first()
                .map(|issue| format!("{}: {}", issue.code, issue.message))
                .unwrap_or_else(|| "unknown conversion failure".to_string());
            return Err(anyhow!("failed to convert project JSON delegate: {detail}"));
        }
    };

    let media_root = media_root(args.media_root.as_deref())?;
    let image_sources = resolve_image_sources(&bundle.image_sources, &media_root)?;
    let video_source_specs = extract_video_sources(&normalized_payload)?;
    let video_sources = resolve_video_sources(&video_source_specs, &media_root)?;

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
        image_cache: HashMap::new(),
        video_sources,
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

fn normalize_delegate_payload(raw: &str) -> Result<String> {
    let mut payload: Value = serde_json::from_str(raw).context("project file is not valid JSON")?;

    let Some(project) = payload.as_object_mut() else {
        return Ok(raw.to_string());
    };

    normalize_timeline(project);
    normalize_sources(project);
    normalize_layers(project);

    serde_json::to_string(&payload).context("failed to serialize normalized project payload")
}

fn normalize_timeline(project: &mut Map<String, Value>) {
    let Some(timeline) = project.get_mut("timeline").and_then(Value::as_object_mut) else {
        return;
    };

    let duration_frames = timeline.remove("duration_frames");
    if timeline.contains_key("total_frames") {
        return;
    }

    if let Some(duration_frames) = duration_frames {
        timeline.insert("total_frames".to_string(), duration_frames);
    }
}

fn normalize_sources(project: &mut Map<String, Value>) {
    let Some(sources) = project.get_mut("sources").and_then(Value::as_array_mut) else {
        return;
    };

    for source in sources {
        let Some(source_obj) = source.as_object_mut() else {
            continue;
        };

        let kind = source_obj
            .get("kind")
            .and_then(|value| value.as_object())
            .cloned();

        let Some(kind_obj) = kind else {
            continue;
        };

        if let Some(kind_type) = kind_obj.get("type").cloned() {
            source_obj.insert("kind".to_string(), kind_type);
        }
        for key in ["path", "url", "filter"] {
            if source_obj.contains_key(key) {
                continue;
            }
            if let Some(value) = kind_obj.get(key).cloned() {
                source_obj.insert(key.to_string(), value);
            }
        }
    }
}

fn normalize_layers(project: &mut Map<String, Value>) {
    let Some(layers) = project.get_mut("layers").and_then(Value::as_array_mut) else {
        return;
    };

    for layer in layers {
        let Some(layer_obj) = layer.as_object_mut() else {
            continue;
        };
        let Some(items) = layer_obj.get("items").and_then(Value::as_array) else {
            continue;
        };

        let mut normalized_items = Vec::new();
        for item in items {
            collect_normalized_clips(item, &mut normalized_items);
        }
        layer_obj.insert("items".to_string(), Value::Array(normalized_items));
    }
}

fn collect_normalized_clips(item: &Value, out: &mut Vec<Value>) {
    let Some(item_obj) = item.as_object() else {
        return;
    };
    let kind = item_kind(item_obj);

    if kind == "group" {
        let Some(children) = item_obj.get("items").and_then(Value::as_array) else {
            return;
        };
        for child in children {
            collect_normalized_clips(child, out);
        }
        return;
    }

    if kind != "clip" {
        return;
    }

    if let Some(clip) = normalize_clip_item(item_obj) {
        out.push(clip);
    }
}

fn normalize_clip_item(item: &Map<String, Value>) -> Option<Value> {
    let id = item.get("id")?.as_str()?.to_string();
    let start_frame = value_u32(item.get("start_frame")).unwrap_or(0);
    let duration_frames = value_u32(item.get("duration_frames")).unwrap_or(0);
    if duration_frames == 0 {
        return None;
    }

    let style = item.get("style").and_then(Value::as_object);
    let transform = normalize_transform(item, style);
    let opacity = value_or_default(
        item.get("opacity"),
        style.and_then(|s| s.get("opacity")),
        1.0,
    );
    let content = normalize_content(item.get("content")?, style)?;

    let mut normalized = Map::new();
    normalized.insert("kind".to_string(), Value::String("clip".to_string()));
    normalized.insert("id".to_string(), Value::String(id));
    normalized.insert("start_frame".to_string(), Value::from(start_frame));
    normalized.insert("duration_frames".to_string(), Value::from(duration_frames));
    normalized.insert("opacity".to_string(), opacity);
    normalized.insert("transform".to_string(), transform);
    normalized.insert("content".to_string(), content);

    Some(Value::Object(normalized))
}

fn normalize_transform(item: &Map<String, Value>, style: Option<&Map<String, Value>>) -> Value {
    let direct = item.get("transform").and_then(Value::as_object);
    let style_transform = style
        .and_then(|s| s.get("transform"))
        .and_then(Value::as_object);

    let x = value_or_default(
        direct.and_then(|t| t.get("x")),
        style_transform.and_then(|t| t.get("x")),
        0.0,
    );
    let y = value_or_default(
        direct.and_then(|t| t.get("y")),
        style_transform.and_then(|t| t.get("y")),
        0.0,
    );
    let width = value_or_default(
        direct.and_then(|t| t.get("width")),
        style_transform.and_then(|t| t.get("width")),
        100.0,
    );
    let height = value_or_default(
        direct.and_then(|t| t.get("height")),
        style_transform.and_then(|t| t.get("height")),
        100.0,
    );
    let rotation_degrees = value_or_default(
        direct.and_then(|t| t.get("rotation_degrees")),
        direct.and_then(|t| t.get("rotation")).or_else(|| {
            style_transform
                .and_then(|t| t.get("rotation_degrees"))
                .or_else(|| style_transform.and_then(|t| t.get("rotation")))
        }),
        0.0,
    );

    let mut transform = Map::new();
    transform.insert("x".to_string(), x);
    transform.insert("y".to_string(), y);
    transform.insert("width".to_string(), width);
    transform.insert("height".to_string(), height);
    transform.insert("rotation_degrees".to_string(), rotation_degrees);
    Value::Object(transform)
}

fn normalize_content(content: &Value, style: Option<&Map<String, Value>>) -> Option<Value> {
    let content_obj = content.as_object()?;
    let content_type = content_obj.get("type")?.as_str()?;

    match content_type {
        "shape" => {
            let shape = content_obj
                .get("shape")
                .and_then(Value::as_str)
                .map(normalize_shape_kind)
                .or_else(|| {
                    content_obj
                        .get("geometry")
                        .and_then(Value::as_object)
                        .and_then(|geometry| geometry.get("kind"))
                        .and_then(Value::as_str)
                        .map(normalize_shape_kind)
                })
                .unwrap_or_else(|| "rectangle".to_string());
            let fill = content_obj
                .get("fill")
                .cloned()
                .or_else(|| style.and_then(|s| s.get("fill")).cloned());
            let radius = content_obj.get("radius").cloned().or_else(|| {
                style
                    .and_then(|s| s.get("corner_radius"))
                    .and_then(Value::as_array)
                    .and_then(|corner| corner.first().cloned())
            });

            let mut shape_content = Map::new();
            shape_content.insert("type".to_string(), Value::String("shape".to_string()));
            shape_content.insert("shape".to_string(), Value::String(shape));
            if let Some(fill) = fill {
                shape_content.insert("fill".to_string(), fill);
            }
            if let Some(radius) = radius {
                shape_content.insert("radius".to_string(), radius);
            }
            Some(Value::Object(shape_content))
        }
        "text" => {
            let text = content_obj
                .get("text")
                .or_else(|| content_obj.get("content"))
                .and_then(Value::as_str)?
                .to_string();

            let mut text_content = Map::new();
            text_content.insert("type".to_string(), Value::String("text".to_string()));
            text_content.insert("text".to_string(), Value::String(text));
            if let Some(font_size) = content_obj
                .get("font_size")
                .cloned()
                .or_else(|| style.and_then(|s| s.get("font_size")).cloned())
            {
                text_content.insert("font_size".to_string(), font_size);
            }
            if let Some(color) = content_obj
                .get("color")
                .cloned()
                .or_else(|| style.and_then(|s| s.get("color")).cloned())
            {
                text_content.insert("color".to_string(), color);
            }
            if let Some(align) = content_obj
                .get("align")
                .cloned()
                .or_else(|| style.and_then(|s| s.get("align")).cloned())
            {
                text_content.insert("align".to_string(), align);
            }
            Some(Value::Object(text_content))
        }
        "image" => {
            let source = content_obj.get("source")?.as_str()?.to_string();
            let mut image_content = Map::new();
            image_content.insert("type".to_string(), Value::String("image".to_string()));
            image_content.insert("source".to_string(), Value::String(source));
            if let Some(fit) = content_obj
                .get("fit")
                .cloned()
                .or_else(|| style.and_then(|s| s.get("fit")).cloned())
            {
                image_content.insert("fit".to_string(), fit);
            }
            Some(Value::Object(image_content))
        }
        "video" => {
            let source = content_obj.get("source")?.as_str()?.to_string();
            let mut video_content = Map::new();
            video_content.insert("type".to_string(), Value::String("video".to_string()));
            video_content.insert("source".to_string(), Value::String(source));
            if let Some(fit) = content_obj
                .get("fit")
                .cloned()
                .or_else(|| style.and_then(|s| s.get("fit")).cloned())
            {
                video_content.insert("fit".to_string(), fit);
            }
            if let Some(pipeline) = content_obj.get("pipeline").cloned() {
                video_content.insert("pipeline".to_string(), normalize_video_pipeline(pipeline));
            }
            Some(Value::Object(video_content))
        }
        "layout" => {
            let text = extract_layout_text(content_obj.get("root")).unwrap_or_default();
            let mut fallback = Map::new();
            fallback.insert("type".to_string(), Value::String("text".to_string()));
            fallback.insert("text".to_string(), Value::String(text));
            if let Some(font_size) = style.and_then(|s| s.get("font_size")).cloned() {
                fallback.insert("font_size".to_string(), font_size);
            }
            if let Some(color) = style.and_then(|s| s.get("color")).cloned() {
                fallback.insert("color".to_string(), color);
            }
            if let Some(align) = style.and_then(|s| s.get("align")).cloned() {
                fallback.insert("align".to_string(), align);
            }
            Some(Value::Object(fallback))
        }
        _ => None,
    }
}

fn normalize_video_pipeline(pipeline: Value) -> Value {
    let Some(mut pipeline_obj) = pipeline.as_object().cloned() else {
        return pipeline;
    };
    if !pipeline_obj.contains_key("looping") {
        if let Some(loop_mode) = pipeline_obj.remove("loop") {
            let mut looping = Map::new();
            looping.insert("mode".to_string(), loop_mode);
            pipeline_obj.insert("looping".to_string(), Value::Object(looping));
        }
    }
    Value::Object(pipeline_obj)
}

fn extract_layout_text(root: Option<&Value>) -> Option<String> {
    let mut lines = Vec::new();
    if let Some(root) = root {
        collect_layout_text(root, &mut lines);
    }

    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

fn collect_layout_text(node: &Value, lines: &mut Vec<String>) {
    let Some(node_obj) = node.as_object() else {
        return;
    };

    if let Some(kind_obj) = node_obj.get("kind").and_then(Value::as_object) {
        if kind_obj
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| kind == "text")
        {
            if let Some(text) = kind_obj.get("content").and_then(Value::as_str) {
                lines.push(text.to_string());
            }
        }
        if let Some(children) = kind_obj.get("children").and_then(Value::as_array) {
            for child in children {
                collect_layout_text(child, lines);
            }
        }
    }
}

fn normalize_shape_kind(kind: &str) -> String {
    match kind {
        "rect" => "rectangle".to_string(),
        "ellipse" | "circle" => "ellipse".to_string(),
        other => other.to_string(),
    }
}

fn item_kind(item: &Map<String, Value>) -> &str {
    item.get("kind")
        .or_else(|| item.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("clip")
}

fn value_u32(value: Option<&Value>) -> Option<u32> {
    let number = value?.as_u64()?;
    u32::try_from(number).ok()
}

fn value_or_default(primary: Option<&Value>, fallback: Option<&Value>, default: f64) -> Value {
    primary
        .cloned()
        .or_else(|| fallback.cloned())
        .unwrap_or_else(|| Value::from(default))
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
    let rgba = render_scene(project, frame, renderer_ctx)
        .map_err(|err| anyhow!("render failed: {err}"))?;
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
        .arg(format!(
            "{}/{}",
            project.frame_rate.num, project.frame_rate.den
        ))
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
                println!("progress frame={}/{}", frame + 1, project.duration_frames);
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
    sources: &HashMap<String, String>,
    root: &Path,
) -> Result<HashMap<String, PathBuf>> {
    let mut resolved = HashMap::new();
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

fn extract_video_sources(payload: &str) -> Result<HashMap<String, String>> {
    let root: Value =
        serde_json::from_str(payload).context("failed to parse normalized payload")?;
    let Some(project) = root.as_object() else {
        return Ok(HashMap::new());
    };
    let Some(sources) = project.get("sources").and_then(Value::as_array) else {
        return Ok(HashMap::new());
    };

    let mut video_sources = HashMap::new();
    for source in sources {
        let Some(source_obj) = source.as_object() else {
            continue;
        };
        let media = source_obj
            .get("media")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if media != "video" {
            continue;
        }

        let Some(id) = source_obj.get("id").and_then(Value::as_str) else {
            continue;
        };
        let Some(path) = source_obj
            .get("path")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .or_else(|| {
                source_obj
                    .get("kind")
                    .and_then(Value::as_object)
                    .and_then(|kind| kind.get("path"))
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
            })
        else {
            continue;
        };

        video_sources.insert(id.to_string(), path);
    }

    Ok(video_sources)
}

fn resolve_video_sources(
    sources: &HashMap<String, String>,
    root: &Path,
) -> Result<HashMap<String, VideoSource>> {
    let mut resolved = HashMap::new();
    for (id, source) in sources {
        if is_http_url(source) {
            continue;
        }
        let path = resolve_local_path_with_root(source, root)
            .with_context(|| format!("failed resolving video source `{id}` -> `{source}`"))?;
        let (width, height) = probe_video_dimensions(&path)
            .with_context(|| format!("failed to read video dimensions for `{}`", path.display()))?;
        resolved.insert(
            id.clone(),
            VideoSource {
                id: id.clone(),
                path,
                width,
                height,
            },
        );
    }
    Ok(resolved)
}

fn probe_video_dimensions(path: &Path) -> Result<(u32, u32)> {
    let output = Command::new("ffprobe")
        .arg("-v")
        .arg("error")
        .arg("-select_streams")
        .arg("v:0")
        .arg("-show_entries")
        .arg("stream=width,height")
        .arg("-of")
        .arg("csv=p=0:s=x")
        .arg(path)
        .output()
        .with_context(|| format!("failed to run ffprobe for `{}`", path.display()))?;

    if !output.status.success() {
        return Err(anyhow!(
            "ffprobe failed for `{}`: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let dims = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let mut parts = dims.split('x');
    let width = parts
        .next()
        .ok_or_else(|| anyhow!("missing width from ffprobe output"))?
        .parse::<u32>()
        .context("invalid width in ffprobe output")?;
    let height = parts
        .next()
        .ok_or_else(|| anyhow!("missing height from ffprobe output"))?
        .parse::<u32>()
        .context("invalid height in ffprobe output")?;
    Ok((width.max(1), height.max(1)))
}

fn decode_video_frame_rgba(path: &Path, width: u32, height: u32, frame: u32) -> Result<Vec<u8>> {
    let filter = format!("select=eq(n\\,{frame})");
    let output = Command::new("ffmpeg")
        .arg("-v")
        .arg("error")
        .arg("-i")
        .arg(path)
        .arg("-vf")
        .arg(filter)
        .arg("-frames:v")
        .arg("1")
        .arg("-f")
        .arg("rawvideo")
        .arg("-pix_fmt")
        .arg("rgba")
        .arg("pipe:1")
        .output()
        .with_context(|| format!("failed to run ffmpeg decoder for `{}`", path.display()))?;

    if !output.status.success() {
        return Err(anyhow!(
            "ffmpeg frame decode failed for `{}`: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let expected = rgba_byte_len(width, height).ok_or_else(|| anyhow!("video frame too large"))?;
    if output.stdout.len() < expected {
        return Err(anyhow!(
            "ffmpeg returned {} bytes for frame {frame}, expected at least {expected}",
            output.stdout.len()
        ));
    }

    Ok(output.stdout[..expected].to_vec())
}

fn rgba_byte_len(width: u32, height: u32) -> Option<usize> {
    let pixels = u64::from(width).checked_mul(u64::from(height))?;
    let bytes = pixels.checked_mul(4)?;
    usize::try_from(bytes).ok()
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

    let candidate = canonicalize_candidate_or_alias(&candidate).with_context(|| {
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

fn canonicalize_candidate_or_alias(candidate: &Path) -> Result<PathBuf> {
    if let Ok(path) = candidate.canonicalize() {
        return Ok(path);
    }

    let Some(name) = candidate.file_name().and_then(|name| name.to_str()) else {
        return candidate
            .canonicalize()
            .context("unable to resolve asset path");
    };
    let parent = candidate
        .parent()
        .ok_or_else(|| anyhow!("asset path has no parent"))?;

    if name.contains('-') {
        let fallback = parent.join(name.replace('-', "_"));
        if let Ok(path) = fallback.canonicalize() {
            return Ok(path);
        }
    }

    if name.contains('_') {
        let fallback = parent.join(name.replace('_', "-"));
        if let Ok(path) = fallback.canonicalize() {
            return Ok(path);
        }
    }

    candidate
        .canonicalize()
        .context("unable to resolve asset path")
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
    use super::{
        choose_video_encoder, extract_video_sources, normalize_delegate_payload,
        resolve_local_path_with_root,
    };
    use std::path::Path;

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
    fn resolve_path_uses_dash_underscore_alias_when_primary_missing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let media = tmp.path().join("chat_bg.mp4");
        std::fs::write(&media, b"stub").expect("write media");
        let root = tmp.path().canonicalize().expect("canonical root");
        let resolved =
            resolve_local_path_with_root("chat-bg.mp4", &root).expect("resolve dash alias");
        assert!(resolved.ends_with("chat_bg.mp4"));
    }

    #[test]
    fn normalize_payload_flattens_group_items_and_layout_text() {
        let raw = r#"{
            "canvas": { "width": 360, "height": 640 },
            "timeline": { "fps": { "num": 30, "den": 1 }, "duration_frames": 60 },
            "layers": [{
                "id": "layer-1",
                "items": [{
                    "type": "group",
                    "id": "group-1",
                    "items": [{
                        "type": "clip",
                        "id": "clip-1",
                        "start_frame": 0,
                        "duration_frames": 60,
                        "content": {
                            "type": "layout",
                            "root": {
                                "kind": {
                                    "type": "container",
                                    "children": [{
                                        "kind": { "type": "text", "content": "hello" }
                                    }]
                                }
                            }
                        },
                        "style": {
                            "transform": { "x": 10, "y": 20, "width": 100, "height": 40 },
                            "opacity": 1
                        }
                    }]
                }]
            }],
            "sources": []
        }"#;

        let normalized = normalize_delegate_payload(raw).expect("normalize");
        let payload: serde_json::Value = serde_json::from_str(&normalized).expect("json");
        let items = payload
            .get("layers")
            .and_then(serde_json::Value::as_array)
            .and_then(|layers| layers.first())
            .and_then(|layer| layer.get("items"))
            .and_then(serde_json::Value::as_array)
            .expect("items");

        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0].get("kind").and_then(serde_json::Value::as_str),
            Some("clip")
        );
        assert_eq!(
            items[0]
                .get("content")
                .and_then(|content| content.get("type"))
                .and_then(serde_json::Value::as_str),
            Some("text")
        );
    }

    #[test]
    fn extract_video_sources_reads_kind_object_path() {
        let payload = r#"{
            "sources": [
                {
                    "id": "video_1",
                    "media": "video",
                    "kind": { "type": "file", "path": "chat-bg.mp4" }
                },
                {
                    "id": "image_1",
                    "media": "image",
                    "kind": "file",
                    "path": "still.png"
                }
            ]
        }"#;

        let sources = extract_video_sources(payload).expect("video sources");
        assert_eq!(
            sources.get("video_1").map(String::as_str),
            Some("chat-bg.mp4")
        );
        assert!(!sources.contains_key("image_1"));
    }

    #[test]
    fn normalized_generated_fixture_converts_with_delegate() {
        let generated_path = Path::new("../..").join("generated.json");
        if !generated_path.exists() {
            return;
        }

        let raw = std::fs::read_to_string(&generated_path).expect("read generated fixture");
        let normalized = normalize_delegate_payload(raw.as_str()).expect("normalize generated");
        let parse_error = serde_json::from_str::<lumen::json::JsonProject>(&normalized).err();
        assert!(
            parse_error.is_none(),
            "json project parse failed: {parse_error:?}"
        );

        let result = lumen::json::convert_json_delegate(&lumen::json::JsonDelegateRequest {
            input_payload: normalized,
            input_schema_revision: "chat_story_v1".to_string(),
            caller_context: "lumen-local-test".to_string(),
        });
        assert!(
            matches!(result.status, lumen::json::JsonDelegateStatus::Success),
            "delegate status={:?} first_error={:?}",
            result.status,
            result.errors.first()
        );
    }
}
