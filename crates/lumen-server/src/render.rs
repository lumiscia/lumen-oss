use std::{
    env,
    io::Write,
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, RwLock, mpsc},
};

use anyhow::{Context, anyhow};
use image::{ImageEncoder, codecs::png::PngEncoder};
use lumen::json::JsonDelegateStatus;
use lumen::{
    AssetCache, Composition, FfmpegMediaStore, ImageResolver, MediaStore, NullMediaStore,
    RenderContext, RuntimeCapabilityProfile, SinkType, SurfacePool, VideoFrameResolver,
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

#[derive(Debug, Clone)]
pub struct ProjectInfo {
    pub width: u32,
    pub height: u32,
    pub duration_frames: u32,
}

pub struct ProjectBundle {
    pub project: ProjectInfo,
    pub composition: Composition,
}

struct LocalMediaStore {
    root: PathBuf,
    backend: FfmpegMediaStore,
}

impl LocalMediaStore {
    fn new(root: PathBuf) -> Self {
        Self {
            root,
            backend: FfmpegMediaStore::new(),
        }
    }

    fn resolve_source(&self, source: &str) -> Option<String> {
        if is_http_url(source) {
            return None;
        }

        resolve_local_path_with_root(source, &self.root)
            .ok()
            .map(|path| path.to_string_lossy().to_string())
    }
}

impl MediaStore for LocalMediaStore {
    fn get_image_resolver(&self, source: &str) -> Option<Box<dyn ImageResolver>> {
        let resolved = self.resolve_source(source)?;
        self.backend.get_image_resolver(&resolved)
    }

    fn get_video_resolver(&self, source: &str) -> Option<Box<dyn VideoFrameResolver>> {
        let resolved = self.resolve_source(source)?;
        self.backend.get_video_resolver(&resolved)
    }
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

    let result = Composition::from_json(&normalized);
    match result.status {
        JsonDelegateStatus::Success => {
            let composition = result.composition.ok_or_else(|| RenderError {
                code: "conversion_error",
                message: "delegate returned success without composition".to_string(),
                retryable: false,
            })?;
            Ok(ProjectBundle {
                project: ProjectInfo {
                    width: composition.render_settings.width,
                    height: composition.render_settings.height,
                    duration_frames: composition.timeline.duration_frames,
                },
                composition,
            })
        }
        JsonDelegateStatus::ValidationError | JsonDelegateStatus::ConversionError => {
            let detail = result
                .errors
                .first()
                .map(std::string::ToString::to_string)
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
    let composition = &bundle.composition;
    let width = composition.render_settings.width;
    let height = composition.render_settings.height;
    let fps = composition.timeline.fps;
    let total_frames = composition.timeline.duration_frames;

    if fps <= 0.0 {
        return Err(RenderError {
            code: "invalid_project_payload",
            message: format!("invalid timeline fps: {fps}"),
            retryable: false,
        });
    }

    if total_frames == 0 {
        return Err(RenderError {
            code: "invalid_project_payload",
            message: "composition duration_frames must be greater than zero".to_string(),
            retryable: false,
        });
    }

    let media_root = media_root(options.media_root.as_deref()).map_err(|err| RenderError {
        code: "media_root_error",
        message: err.to_string(),
        retryable: false,
    })?;
    let media_store: Arc<dyn MediaStore> = Arc::new(LocalMediaStore::new(media_root));
    let mut renderer_ctx = create_renderer_context(composition, media_store, true);
    let encoder = choose_video_encoder(options.video_encoder.as_deref());

    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    let encode_handle =
        std::thread::spawn(move || encode_rgba_stream(width, height, fps, encoder, rx));

    for frame in 0..total_frames {
        let bitmap = composition
            .render_frame(frame, &mut renderer_ctx)
            .map_err(|err| RenderError {
                code: "render_failed",
                message: format!("render failed at frame {frame}: {err}"),
                retryable: true,
            })?
            .into_bitmap_frame()
            .map_err(|err| RenderError {
                code: "render_failed",
                message: format!("failed to convert frame {frame} to bitmap: {err}"),
                retryable: true,
            })?;

        if bitmap.storage_width != width || bitmap.storage_height != height {
            return Err(RenderError {
                code: "render_failed",
                message: format!(
                    "frame {frame} dimensions {}x{} do not match composition {}x{}",
                    bitmap.storage_width, bitmap.storage_height, width, height
                ),
                retryable: false,
            });
        }

        tx.send((*bitmap.pixels).clone()).map_err(|_| RenderError {
            code: "encode_failed",
            message: "ffmpeg encoder thread is unavailable".to_string(),
            retryable: true,
        })?;

        let completed = frame.saturating_add(1);
        let ratio = (completed as f32 / total_frames as f32).clamp(0.0, 1.0);
        on_progress(RenderProgress {
            stage: "rendering",
            frame: completed,
            total_frames,
            ratio,
        });
    }

    drop(tx);

    encode_handle
        .join()
        .map_err(|_| RenderError {
            code: "encode_failed",
            message: "ffmpeg encoder thread panicked".to_string(),
            retryable: true,
        })?
        .map_err(|err| RenderError {
            code: "encode_failed",
            message: err.to_string(),
            retryable: true,
        })
}

pub fn render_project_frame_png(
    bundle: &ProjectBundle,
    frame: u32,
) -> Result<Vec<u8>, RenderError> {
    let composition = &bundle.composition;

    if frame >= composition.timeline.duration_frames {
        return Err(RenderError {
            code: "frame_out_of_range",
            message: format!(
                "requested frame {frame} is out of range for duration {}",
                composition.timeline.duration_frames
            ),
            retryable: false,
        });
    }

    let media_store: Arc<dyn MediaStore> = Arc::new(NullMediaStore);
    let mut renderer_ctx = create_renderer_context(composition, media_store, false);
    let rendered = composition
        .render_frame(frame, &mut renderer_ctx)
        .map_err(|err| RenderError {
            code: "render_failed",
            message: err.to_string(),
            retryable: false,
        })?
        .into_bitmap_frame()
        .map_err(|err| RenderError {
            code: "render_failed",
            message: err.to_string(),
            retryable: false,
        })?;

    let mut png = Vec::new();
    let encoder = PngEncoder::new(&mut png);
    encoder
        .write_image(
            rendered.pixels.as_slice(),
            rendered.storage_width,
            rendered.storage_height,
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
    composition: &Composition,
    media_store: Arc<dyn MediaStore>,
    has_media_resolvers: bool,
) -> RenderContext {
    let profile = RuntimeCapabilityProfile {
        has_image_resolver: has_media_resolvers,
        has_video_resolver: has_media_resolvers,
        has_threading: false,
        sink_types: vec![SinkType::Bitmap, SinkType::Video],
    };

    RenderContext::new(
        composition,
        Arc::new(SurfacePool::new()),
        Arc::new(RwLock::new(AssetCache::new())),
        media_store,
        profile,
    )
}

// ---------------------------------------------------------------------------
// ffmpeg encoding (subprocess-based, produces bytes in memory)
// ---------------------------------------------------------------------------

fn encode_rgba_stream(
    width: u32,
    height: u32,
    fps: f32,
    encoder: String,
    rx: mpsc::Receiver<Vec<u8>>,
) -> anyhow::Result<Vec<u8>> {
    let tmp = tempfile::tempdir().context("failed to create temp dir")?;
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
        .arg(format!("{fps}"))
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
        .context("failed to spawn ffmpeg")?;

    {
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| anyhow!("ffmpeg stdin unavailable"))?;

        for frame in rx {
            if stdin.write_all(&frame).is_err() {
                break;
            }
        }
    }

    let output = child.wait_with_output().context("ffmpeg wait failed")?;

    if !output.status.success() {
        return Err(anyhow!(
            "ffmpeg encode failed with encoder `{encoder}`: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    std::fs::read(&output_path).context("failed to read encoded output")
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

// ---------------------------------------------------------------------------
// JSON payload normalization (ported from lumen-local)
// ---------------------------------------------------------------------------

fn normalize_delegate_payload(payload: &serde_json::Value) -> anyhow::Result<String> {
    let mut payload = payload.clone();
    let Some(project) = payload.as_object_mut() else {
        return serde_json::to_string(&payload).context("serialize payload");
    };

    let looks_like_composition = project.contains_key("schema_revision")
        && project.contains_key("graph")
        && project.contains_key("render_settings");
    if looks_like_composition {
        return serde_json::to_string(&payload).context("serialize payload");
    }

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
            let normalized_kind = match kind_type {
                serde_json::Value::String(kind) if kind == "path" => {
                    serde_json::Value::String("file".to_string())
                }
                other => other,
            };
            source_obj.insert("kind".to_string(), normalized_kind);
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
