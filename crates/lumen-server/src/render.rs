use std::{
    collections::HashMap,
    env,
    io::Write,
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex, mpsc},
};

use anyhow::{Context, anyhow};
use image::{ImageEncoder, codecs::png::PngEncoder};
use lumen::{
    Project,
    ffmpeg::{FfmpegError, worker::VideoDecodeWorker},
    json::{JsonDelegateRequest, JsonDelegateStatus, ProjectBundle, convert_json_delegate},
    media::{ImageResolver, MediaStore, VideoResolver},
    render::{context::RendererContext, render_scene},
    time::Rational,
};

pub const MEDIA_ROOT_ENV: &str = "LUMEN_MEDIA_ROOT";

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct RenderError {
    pub code: &'static str,
    pub message: String,
    pub retryable: bool,
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for RenderError {}

// ---------------------------------------------------------------------------
// Metrics / progress
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct RenderMetrics {
    pub convert_ms: u128,
    pub render_ms: u128,
    pub total_frames: u32,
}

#[derive(Debug, Clone)]
pub struct RenderProgress {
    pub stage: &'static str,
    pub frame: u32,
    pub total_frames: u32,
    pub ratio: f32,
}

// ---------------------------------------------------------------------------
// Options
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct RenderOptions {
    pub media_root: Option<PathBuf>,
    pub video_encoder: Option<String>,
}

// ---------------------------------------------------------------------------
// JSON conversion
// ---------------------------------------------------------------------------

pub fn convert_project_payload(payload: &serde_json::Value) -> Result<ProjectBundle, RenderError> {
    let normalized = normalize_delegate_payload(payload).map_err(|err| RenderError {
        code: "invalid_project_payload",
        message: err.to_string(),
        retryable: false,
    })?;

    let request = JsonDelegateRequest {
        input_payload: normalized,
        input_schema_revision: "chat_story_v1".to_string(),
        caller_context: "lumen-server".to_string(),
    };

    let result = convert_json_delegate(&request);
    match result.status {
        JsonDelegateStatus::Success => result.project_bundle.ok_or_else(|| RenderError {
            code: "conversion_error",
            message: "delegate returned success without project bundle".to_string(),
            retryable: false,
        }),
        JsonDelegateStatus::CapabilityDisabled => Err(RenderError {
            code: "capability_disabled",
            message: "json delegate is disabled in this build".to_string(),
            retryable: false,
        }),
        JsonDelegateStatus::ValidationError | JsonDelegateStatus::ConversionError => {
            let detail = result
                .errors
                .first()
                .map(|issue| format!("{}: {}", issue.code, issue.message))
                .unwrap_or_else(|| "unknown conversion failure".to_string());
            Err(RenderError {
                code: "invalid_project_payload",
                message: detail,
                retryable: false,
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Render entry points
// ---------------------------------------------------------------------------

pub fn render_project_mp4(
    bundle: &ProjectBundle,
    options: &RenderOptions,
    on_progress: &mut dyn FnMut(RenderProgress),
) -> Result<Vec<u8>, RenderError> {
    let project = &bundle.project;
    let mut renderer_ctx = create_renderer_context(project, bundle.background)?;

    let media_root = media_root(options.media_root.as_deref()).map_err(|err| RenderError {
        code: "media_root_error",
        message: err.to_string(),
        retryable: false,
    })?;

    let media_store = build_media_store(bundle, &media_root).map_err(|err| RenderError {
        code: "media_setup_error",
        message: err.to_string(),
        retryable: true,
    })?;
    renderer_ctx.set_media_store(Box::new(media_store));

    let width = project.width;
    let height = project.height;
    let fps = project.frame_rate;
    let total_frames = project.duration_frames;
    let encoder = choose_video_encoder(options.video_encoder.as_deref());

    let output: Arc<Mutex<Option<Result<Vec<u8>, FfmpegError>>>> = Arc::new(Mutex::new(None));
    let output_capture = Arc::clone(&output);

    lumen::ffmpeg::worker::render_to_mp4(
        total_frames,
        |frame| {
            render_scene(project, frame, &mut renderer_ctx)
                .map_err(|err| FfmpegError::Init(format!("render failed at frame {frame}: {err}")))
        },
        move |rx| {
            let result = encode_rgba_stream(width, height, fps, encoder, rx);
            *output_capture.lock().expect("output lock") = Some(result);
            Ok(())
        },
        |frame, total| {
            let ratio = if total == 0 {
                0.0
            } else {
                (frame as f32 / total as f32).clamp(0.0, 1.0)
            };
            on_progress(RenderProgress {
                stage: "rendering",
                frame,
                total_frames: total,
                ratio,
            });
        },
    )
    .map_err(|err| RenderError {
        code: "render_failed",
        message: err.to_string(),
        retryable: true,
    })?;

    let result = output
        .lock()
        .expect("output lock")
        .take()
        .ok_or_else(|| RenderError {
            code: "render_failed",
            message: "encode thread did not produce output".to_string(),
            retryable: false,
        })?;

    result.map_err(|err| RenderError {
        code: "encode_failed",
        message: err.to_string(),
        retryable: true,
    })
}

pub fn render_project_frame_png(
    bundle: &ProjectBundle,
    frame: u32,
) -> Result<Vec<u8>, RenderError> {
    let project = &bundle.project;

    if frame >= project.duration_frames {
        return Err(RenderError {
            code: "frame_out_of_range",
            message: format!(
                "requested frame {frame} is out of range for duration {}",
                project.duration_frames
            ),
            retryable: false,
        });
    }

    let mut renderer_ctx = create_renderer_context(project, bundle.background)?;

    // For frame preview, we don't set up a full media store — images/video won't resolve,
    // but text/shape clips will render fine. A production deploy would want media here.
    let rgba = render_scene(project, frame, &mut renderer_ctx).map_err(|err| RenderError {
        code: "render_failed",
        message: err.to_string(),
        retryable: false,
    })?;

    let mut png = Vec::new();
    let encoder = PngEncoder::new(&mut png);
    encoder
        .write_image(
            &rgba,
            project.width,
            project.height,
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|err| RenderError {
            code: "png_encode_failed",
            message: err.to_string(),
            retryable: false,
        })?;

    Ok(png)
}

// ---------------------------------------------------------------------------
// RendererContext factory
// ---------------------------------------------------------------------------

fn create_renderer_context(
    project: &Project,
    background: [u8; 4],
) -> Result<RendererContext, RenderError> {
    let mut ctx =
        RendererContext::new(project.width, project.height, project.frame_rate).map_err(|err| {
            RenderError {
                code: "renderer_init_failed",
                message: err.to_string(),
                retryable: false,
            }
        })?;
    ctx.clear_color =
        skia_safe::Color::from_argb(background[3], background[0], background[1], background[2]);
    Ok(ctx)
}

// ---------------------------------------------------------------------------
// MediaStore implementation for server
// ---------------------------------------------------------------------------

struct ServerMediaStore {
    image_sources: HashMap<String, PathBuf>,
    image_cache: HashMap<String, CachedImage>,
    video_workers: HashMap<String, VideoDecodeWorker>,
}

#[derive(Clone)]
struct CachedImage {
    width: u32,
    height: u32,
    pixels_rgba: Arc<Vec<u8>>,
}

struct ServerImageResolver {
    id: String,
    image: CachedImage,
}

struct ServerVideoResolver {
    id: String,
    worker: *mut VideoDecodeWorker,
}

// SAFETY: VideoDecodeWorker is accessed exclusively through the MediaStore which is
// single-threaded within a render context. The pointer is never shared across threads.
unsafe impl Send for ServerVideoResolver {}

impl ImageResolver for ServerImageResolver {
    fn id(&self) -> String {
        self.id.clone()
    }

    fn width(&self) -> u32 {
        self.image.width
    }

    fn height(&self) -> u32 {
        self.image.height
    }

    fn resolve(&mut self) -> Vec<u8> {
        (*self.image.pixels_rgba).clone()
    }
}

impl VideoResolver for ServerVideoResolver {
    fn id(&self) -> String {
        self.id.clone()
    }

    fn width(&self) -> u32 {
        // Video dimensions are determined by the decoder, but we don't have easy access
        // to them here. Return 0 and let the renderer handle it.
        0
    }

    fn height(&self) -> u32 {
        0
    }

    fn resolve_frame(&mut self, frame: u32) -> Vec<u8> {
        // SAFETY: see unsafe impl Send above
        let worker = unsafe { &*self.worker };
        match worker.get_frame(frame as u64) {
            Ok(Some(image)) => (*image.pixels_rgba).clone(),
            _ => Vec::new(),
        }
    }
}

impl MediaStore for ServerMediaStore {
    fn get_image_resolver(&mut self, id: &str) -> Option<Box<dyn ImageResolver>> {
        if !self.image_cache.contains_key(id) {
            let path = self.image_sources.get(id)?.clone();
            let image = load_image_rgba(&path).ok()?;
            self.image_cache.insert(id.to_string(), image);
        }
        let image = self.image_cache.get(id)?.clone();
        Some(Box::new(ServerImageResolver {
            id: id.to_string(),
            image,
        }))
    }

    fn get_video_resolver(&mut self, id: &str) -> Option<Box<dyn VideoResolver>> {
        let worker = self.video_workers.get_mut(id)?;
        Some(Box::new(ServerVideoResolver {
            id: id.to_string(),
            worker: worker as *mut VideoDecodeWorker,
        }))
    }
}

fn build_media_store(
    bundle: &ProjectBundle,
    media_root: &Path,
) -> anyhow::Result<ServerMediaStore> {
    let mut image_sources = HashMap::new();
    for (id, source) in &bundle.image_sources {
        if is_http_url(source) {
            // TODO: URL image download support
            continue;
        }
        let path = resolve_local_path_with_root(source, media_root)
            .with_context(|| format!("failed resolving image source `{id}` -> `{source}`"))?;
        image_sources.insert(id.clone(), path);
    }

    // Video workers would be set up here from sources in the raw JSON.
    // For now, video sources require the ffmpeg libav decode path.
    let video_workers = HashMap::new();

    Ok(ServerMediaStore {
        image_sources,
        image_cache: HashMap::new(),
        video_workers,
    })
}

// ---------------------------------------------------------------------------
// ffmpeg encoding (subprocess-based, produces bytes in memory)
// ---------------------------------------------------------------------------

fn encode_rgba_stream(
    width: u32,
    height: u32,
    fps: Rational,
    encoder: String,
    rx: mpsc::Receiver<Vec<u8>>,
) -> Result<Vec<u8>, FfmpegError> {
    let tmp = tempfile::tempdir()
        .map_err(|err| FfmpegError::Init(format!("failed to create temp dir: {err}")))?;
    let output_path = tmp.path().join("output.mp4");

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
        .arg(format!("{width}x{height}"))
        .arg("-r")
        .arg(format!("{}/{}", fps.num, fps.den))
        .arg("-i")
        .arg("pipe:0")
        .arg("-an")
        .arg("-c:v")
        .arg(&encoder)
        .arg("-pix_fmt")
        .arg("yuv420p")
        .arg("-movflags")
        .arg("+faststart")
        .arg(&output_path)
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| FfmpegError::Init(format!("failed to spawn ffmpeg: {err}")))?;

    {
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| FfmpegError::Init("ffmpeg stdin unavailable".to_string()))?;

        for frame in rx {
            if stdin.write_all(&frame).is_err() {
                break;
            }
        }
    }

    let output = child
        .wait_with_output()
        .map_err(|err| FfmpegError::Init(format!("ffmpeg wait failed: {err}")))?;

    if !output.status.success() {
        return Err(FfmpegError::Init(format!(
            "ffmpeg encode failed with encoder `{encoder}`: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    std::fs::read(&output_path)
        .map_err(|err| FfmpegError::Init(format!("failed to read encoded output: {err}")))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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

pub fn media_root(override_root: Option<&Path>) -> anyhow::Result<PathBuf> {
    let path = match override_root {
        Some(path) => path.to_path_buf(),
        None => {
            let raw = env::var(MEDIA_ROOT_ENV).unwrap_or_else(|_| ".".to_string());
            PathBuf::from(raw)
        }
    };
    path.canonicalize()
        .with_context(|| format!("failed to resolve media root from {MEDIA_ROOT_ENV}"))
}

fn resolve_local_path_with_root(source: &str, root: &Path) -> anyhow::Result<PathBuf> {
    let root = root
        .canonicalize()
        .with_context(|| format!("failed to canonicalize media root `{}`", root.display()))?;

    if source.contains("://") && !source.starts_with("file://") {
        return Err(anyhow!("unsupported asset URI scheme for `{source}`"));
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

    if !candidate.starts_with(&root) {
        return Err(anyhow!(
            "asset path escapes allowed media root: `{}`",
            candidate.display()
        ));
    }

    Ok(candidate)
}

fn is_http_url(source: &str) -> bool {
    source.starts_with("http://") || source.starts_with("https://")
}

fn load_image_rgba(path: &Path) -> anyhow::Result<CachedImage> {
    let image = image::ImageReader::open(path)
        .with_context(|| format!("failed to open image `{}`", path.display()))?
        .decode()
        .with_context(|| format!("failed to decode image `{}`", path.display()))?;
    let rgba = image.into_rgba8();
    Ok(CachedImage {
        width: rgba.width(),
        height: rgba.height(),
        pixels_rgba: Arc::new(rgba.into_raw()),
    })
}

// ---------------------------------------------------------------------------
// JSON payload normalization (ported from lumen-local)
// ---------------------------------------------------------------------------

fn normalize_delegate_payload(payload: &serde_json::Value) -> anyhow::Result<String> {
    let mut payload = payload.clone();
    let Some(project) = payload.as_object_mut() else {
        return serde_json::to_string(&payload).context("serialize payload");
    };

    normalize_timeline(project);
    normalize_sources(project);
    normalize_layers(project);

    serde_json::to_string(&payload).context("serialize normalized payload")
}

fn normalize_timeline(project: &mut serde_json::Map<String, serde_json::Value>) {
    let Some(timeline) = project
        .get_mut("timeline")
        .and_then(serde_json::Value::as_object_mut)
    else {
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

fn normalize_sources(project: &mut serde_json::Map<String, serde_json::Value>) {
    let Some(sources) = project
        .get_mut("sources")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return;
    };

    for source in sources {
        let Some(source_obj) = source.as_object_mut() else {
            continue;
        };
        let kind = source_obj.get("kind").and_then(|v| v.as_object()).cloned();
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

fn normalize_layers(project: &mut serde_json::Map<String, serde_json::Value>) {
    let Some(layers) = project
        .get_mut("layers")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return;
    };

    for layer in layers {
        let Some(layer_obj) = layer.as_object_mut() else {
            continue;
        };
        let Some(items) = layer_obj.get("items").and_then(serde_json::Value::as_array) else {
            continue;
        };
        let mut normalized = Vec::new();
        for item in items {
            collect_normalized_clips(item, &mut normalized);
        }
        layer_obj.insert("items".to_string(), serde_json::Value::Array(normalized));
    }
}

fn collect_normalized_clips(item: &serde_json::Value, out: &mut Vec<serde_json::Value>) {
    let Some(item_obj) = item.as_object() else {
        return;
    };
    let kind = item_kind(item_obj);

    if kind == "group" {
        let Some(children) = item_obj.get("items").and_then(serde_json::Value::as_array) else {
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

fn normalize_clip_item(
    item: &serde_json::Map<String, serde_json::Value>,
) -> Option<serde_json::Value> {
    let id = item.get("id")?.as_str()?.to_string();
    let start_frame = item
        .get("start_frame")
        .and_then(|v| v.as_u64())
        .and_then(|v| u32::try_from(v).ok())
        .unwrap_or(0);
    let duration_frames = item
        .get("duration_frames")
        .and_then(|v| v.as_u64())
        .and_then(|v| u32::try_from(v).ok())
        .unwrap_or(0);
    if duration_frames == 0 {
        return None;
    }

    let style = item.get("style").and_then(serde_json::Value::as_object);
    let transform = normalize_transform(item, style);
    let opacity = value_or_default(
        item.get("opacity"),
        style.and_then(|s| s.get("opacity")),
        1.0,
    );
    let content = normalize_content(item.get("content")?, style)?;

    let mut normalized = serde_json::Map::new();
    normalized.insert(
        "kind".to_string(),
        serde_json::Value::String("clip".to_string()),
    );
    normalized.insert("id".to_string(), serde_json::Value::String(id));
    normalized.insert(
        "start_frame".to_string(),
        serde_json::Value::from(start_frame),
    );
    normalized.insert(
        "duration_frames".to_string(),
        serde_json::Value::from(duration_frames),
    );
    normalized.insert("opacity".to_string(), opacity);
    normalized.insert("transform".to_string(), transform);
    normalized.insert("content".to_string(), content);

    Some(serde_json::Value::Object(normalized))
}

fn normalize_transform(
    item: &serde_json::Map<String, serde_json::Value>,
    style: Option<&serde_json::Map<String, serde_json::Value>>,
) -> serde_json::Value {
    let direct = item.get("transform").and_then(serde_json::Value::as_object);
    let style_transform = style
        .and_then(|s| s.get("transform"))
        .and_then(serde_json::Value::as_object);

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

    let mut transform = serde_json::Map::new();
    transform.insert("x".to_string(), x);
    transform.insert("y".to_string(), y);
    transform.insert("width".to_string(), width);
    transform.insert("height".to_string(), height);
    transform.insert("rotation_degrees".to_string(), rotation_degrees);
    serde_json::Value::Object(transform)
}

fn normalize_content(
    content: &serde_json::Value,
    style: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Option<serde_json::Value> {
    let content_obj = content.as_object()?;
    let content_type = content_obj.get("type")?.as_str()?;

    match content_type {
        "shape" => {
            let shape = content_obj
                .get("shape")
                .and_then(serde_json::Value::as_str)
                .map(normalize_shape_kind)
                .unwrap_or_else(|| "rectangle".to_string());
            let fill = content_obj
                .get("fill")
                .cloned()
                .or_else(|| style.and_then(|s| s.get("fill")).cloned());
            let radius = content_obj.get("radius").cloned().or_else(|| {
                style
                    .and_then(|s| s.get("corner_radius"))
                    .and_then(serde_json::Value::as_array)
                    .and_then(|corner| corner.first().cloned())
            });

            let mut out = serde_json::Map::new();
            out.insert(
                "type".to_string(),
                serde_json::Value::String("shape".to_string()),
            );
            out.insert("shape".to_string(), serde_json::Value::String(shape));
            if let Some(fill) = fill {
                out.insert("fill".to_string(), fill);
            }
            if let Some(radius) = radius {
                out.insert("radius".to_string(), radius);
            }
            Some(serde_json::Value::Object(out))
        }
        "text" => {
            let text = content_obj
                .get("text")
                .or_else(|| content_obj.get("content"))
                .and_then(serde_json::Value::as_str)?
                .to_string();

            let mut out = serde_json::Map::new();
            out.insert(
                "type".to_string(),
                serde_json::Value::String("text".to_string()),
            );
            out.insert("text".to_string(), serde_json::Value::String(text));
            if let Some(font_size) = content_obj
                .get("font_size")
                .cloned()
                .or_else(|| style.and_then(|s| s.get("font_size")).cloned())
            {
                out.insert("font_size".to_string(), font_size);
            }
            if let Some(color) = content_obj
                .get("color")
                .cloned()
                .or_else(|| style.and_then(|s| s.get("color")).cloned())
            {
                out.insert("color".to_string(), color);
            }
            if let Some(align) = content_obj
                .get("align")
                .cloned()
                .or_else(|| style.and_then(|s| s.get("align")).cloned())
            {
                out.insert("align".to_string(), align);
            }
            Some(serde_json::Value::Object(out))
        }
        "image" => {
            let source = content_obj.get("source")?.as_str()?.to_string();
            let mut out = serde_json::Map::new();
            out.insert(
                "type".to_string(),
                serde_json::Value::String("image".to_string()),
            );
            out.insert("source".to_string(), serde_json::Value::String(source));
            if let Some(fit) = content_obj.get("fit").cloned() {
                out.insert("fit".to_string(), fit);
            }
            Some(serde_json::Value::Object(out))
        }
        "video" => {
            let source = content_obj.get("source")?.as_str()?.to_string();
            let mut out = serde_json::Map::new();
            out.insert(
                "type".to_string(),
                serde_json::Value::String("video".to_string()),
            );
            out.insert("source".to_string(), serde_json::Value::String(source));
            if let Some(fit) = content_obj.get("fit").cloned() {
                out.insert("fit".to_string(), fit);
            }
            if let Some(pipeline) = content_obj.get("pipeline").cloned() {
                out.insert("pipeline".to_string(), pipeline);
            }
            Some(serde_json::Value::Object(out))
        }
        _ => None,
    }
}

fn normalize_shape_kind(kind: &str) -> String {
    match kind {
        "rect" => "rectangle".to_string(),
        "ellipse" | "circle" => "ellipse".to_string(),
        other => other.to_string(),
    }
}

fn item_kind(item: &serde_json::Map<String, serde_json::Value>) -> &str {
    item.get("kind")
        .or_else(|| item.get("type"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("clip")
}

fn value_or_default(
    primary: Option<&serde_json::Value>,
    fallback: Option<&serde_json::Value>,
    default: f64,
) -> serde_json::Value {
    primary
        .cloned()
        .or_else(|| fallback.cloned())
        .unwrap_or_else(|| serde_json::Value::from(default))
}
