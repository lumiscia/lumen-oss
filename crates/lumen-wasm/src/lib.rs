use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap},
    ffi::{c_char, c_void},
    ptr,
    sync::{Arc, RwLock},
};

use lumen::{
    AssetCache, Composition, Connection, Graph, InputPort, MediaStore, NodeId, NodeKind,
    OutputPort, RenderContext, RenderSettings, RuntimeCapabilityProfile, SharedAssetCache,
    SurfacePool, TimelineSettings,
    media::{ImageResolver, VideoFrameResolver, premultiply_rgba_in_place_if_needed},
    node::{Node, media_output::MediaOutput, solid_color::SolidColor},
};
use serde::{Deserialize, Serialize};

static VERSION: &[u8] = b"lumen-wasm-next\0";
static EMPTY_FRAME_REQUIREMENTS: &[u8] = br#"{"images":[],"videos":[]}"#;

unsafe extern "C" {
    fn free(ptr: *mut c_void);
}

thread_local! {
    static REGISTRY: RefCell<Registry> = RefCell::new(Registry::default());
}

struct Registry {
    next_handle: u32,
    last_status: Vec<u8>,
    renderers: HashMap<u32, RendererSession>,
    media_stores: HashMap<u32, Arc<InMemoryMediaStore>>,
}

impl Registry {
    fn alloc_handle(&mut self) -> u32 {
        if self.next_handle == 0 {
            self.next_handle = 1;
        }
        for _ in 0..u32::MAX {
            let handle = self.next_handle;
            self.next_handle = self.next_handle.wrapping_add(1);
            if handle != 0
                && !self.renderers.contains_key(&handle)
                && !self.media_stores.contains_key(&handle)
            {
                return handle;
            }
        }
        0
    }

    fn set_status(&mut self, status: &str, code: &str, message: &str) {
        self.last_status = encode_status(status, code, message);
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            next_handle: 1,
            last_status: encode_status("ok", "ready", "ready"),
            renderers: HashMap::new(),
            media_stores: HashMap::new(),
        }
    }
}

struct RendererSession {
    composition: Composition,
    asset_cache: SharedAssetCache,
    last_frame: Vec<u8>,
    last_frame_requirements: Vec<u8>,
    last_error: Vec<u8>,
}

impl RendererSession {
    fn new(composition: Composition) -> Self {
        Self {
            composition,
            asset_cache: Arc::new(RwLock::new(AssetCache::new())),
            last_frame: Vec::new(),
            last_frame_requirements: EMPTY_FRAME_REQUIREMENTS.to_vec(),
            last_error: Vec::new(),
        }
    }

    fn width(&self) -> u32 {
        self.composition.render_settings.width
    }

    fn height(&self) -> u32 {
        self.composition.render_settings.height
    }
}

#[derive(Default)]
struct InMemoryMediaStore {
    images: RwLock<HashMap<String, StoredImage>>,
    videos: RwLock<HashMap<String, StoredVideo>>,
}

#[derive(Clone)]
struct StoredImage {
    width: u32,
    height: u32,
    pixels: Arc<Vec<u8>>,
}

#[derive(Clone, Default)]
struct StoredVideo {
    width: u32,
    height: u32,
    frames: BTreeMap<u32, Arc<Vec<u8>>>,
}

#[derive(Clone)]
struct WasmImageResolver {
    id: String,
    entry: StoredImage,
}

#[derive(Clone)]
struct WasmVideoResolver {
    id: String,
    entry: StoredVideo,
}

impl ImageResolver for WasmImageResolver {
    fn id(&self) -> &str {
        &self.id
    }

    fn width(&self) -> u32 {
        self.entry.width
    }

    fn height(&self) -> u32 {
        self.entry.height
    }

    fn resolve(&self) -> Result<Arc<Vec<u8>>, lumen::error::MediaError> {
        Ok(Arc::clone(&self.entry.pixels))
    }
}

impl VideoFrameResolver for WasmVideoResolver {
    fn id(&self) -> &str {
        &self.id
    }

    fn width(&self) -> u32 {
        self.entry.width
    }

    fn height(&self) -> u32 {
        self.entry.height
    }

    fn frame_count(&self) -> u32 {
        self.entry
            .frames
            .keys()
            .next_back()
            .and_then(|frame| frame.checked_add(1))
            .unwrap_or(0)
    }

    fn resolve_frame(&self, frame: u32) -> Result<Arc<Vec<u8>>, lumen::error::MediaError> {
        let Some(bytes) = self.entry.frames.get(&frame) else {
            return Err(lumen::error::MediaError::FrameOutOfRange {
                media_source: self.id.clone(),
                frame,
                frame_count: self.frame_count(),
            });
        };
        Ok(Arc::clone(bytes))
    }
}

impl MediaStore for InMemoryMediaStore {
    fn get_image_resolver(&self, source: &str) -> Option<Box<dyn ImageResolver>> {
        let images = self.images.read().ok()?;
        let entry = images.get(source)?.clone();
        Some(Box::new(WasmImageResolver {
            id: source.to_string(),
            entry,
        }))
    }

    fn get_video_resolver(&self, source: &str) -> Option<Box<dyn VideoFrameResolver>> {
        let videos = self.videos.read().ok()?;
        let entry = videos.get(source)?.clone();
        Some(Box::new(WasmVideoResolver {
            id: source.to_string(),
            entry,
        }))
    }
}

#[derive(Serialize)]
struct StatusPayload<'a> {
    status: &'a str,
    code: &'a str,
    message: &'a str,
}

#[derive(Deserialize)]
struct PreviewProjectInput {
    canvas: PreviewCanvasInput,
    timeline: PreviewTimelineInput,
    #[serde(default)]
    sources: Vec<serde_json::Value>,
    #[serde(default)]
    layers: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
struct PreviewCanvasInput {
    width: u32,
    height: u32,
    #[serde(default = "default_background")]
    background: [u8; 4],
}

#[derive(Deserialize)]
struct PreviewTimelineInput {
    fps: PreviewFpsInput,
    total_frames: Option<u32>,
    duration_frames: Option<u32>,
}

#[derive(Deserialize)]
struct PreviewFpsInput {
    num: u32,
    den: u32,
}

const fn default_background() -> [u8; 4] {
    [0, 0, 0, 255]
}

fn encode_status(status: &str, code: &str, message: &str) -> Vec<u8> {
    serde_json::to_vec(&StatusPayload {
        status,
        code,
        message,
    })
    .unwrap_or_else(|_| {
        format!(
            r#"{{"status":"{}","code":"{}","message":"{}"}}"#,
            escape_json(status),
            escape_json(code),
            escape_json(message)
        )
        .into_bytes()
    })
}

fn escape_json(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(ch),
        }
    }
    out
}

unsafe fn read_bytes(ptr_in: *const u8, len: usize) -> Result<Vec<u8>, &'static str> {
    if len == 0 {
        return Ok(Vec::new());
    }
    if ptr_in.is_null() {
        return Err("null pointer for non-empty buffer");
    }
    let slice = unsafe { std::slice::from_raw_parts(ptr_in, len) };
    Ok(slice.to_vec())
}

unsafe fn read_string(ptr_in: *const u8, len: usize) -> Result<String, &'static str> {
    let bytes = unsafe { read_bytes(ptr_in, len)? };
    String::from_utf8(bytes).map_err(|_| "input is not valid utf-8")
}

unsafe fn take_owned_bytes(ptr_in: *mut u8, len: usize) -> Result<Vec<u8>, &'static str> {
    let bytes = unsafe { read_bytes(ptr_in.cast_const(), len)? };
    if !ptr_in.is_null() {
        unsafe { free(ptr_in.cast()) };
    }
    Ok(bytes)
}

fn scaled_dimension(value: u32, scale: f32) -> Result<u32, String> {
    let scaled = (value as f32) * scale;
    if !scaled.is_finite() || scaled <= 0.0 {
        return Err("scaled dimension must be > 0".to_string());
    }
    Ok(scaled.round().max(1.0) as u32)
}

fn preview_project_to_composition(
    project: PreviewProjectInput,
    scale: f32,
) -> Result<Composition, String> {
    if !(scale.is_finite() && scale > 0.0) {
        return Err("scale must be finite and > 0".to_string());
    }
    if project.timeline.fps.num == 0 || project.timeline.fps.den == 0 {
        return Err("timeline fps num/den must be > 0".to_string());
    }
    let total_frames = match (
        project.timeline.total_frames,
        project.timeline.duration_frames,
    ) {
        (Some(value), _) if value > 0 => value,
        (None, Some(value)) if value > 0 => value,
        _ => return Err("timeline.total_frames must be > 0".to_string()),
    };
    if !project.sources.is_empty() || !project.layers.is_empty() {
        return Err(
            "preview project conversion is not implemented for sources/layers yet".to_string(),
        );
    }

    let width = scaled_dimension(project.canvas.width, scale)?;
    let height = scaled_dimension(project.canvas.height, scale)?;
    let fps = (project.timeline.fps.num as f32) / (project.timeline.fps.den as f32);

    let mut graph = Graph::new();
    let solid = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::SolidColor(SolidColor {
            color: project.canvas.background,
            width: Some(width),
            height: Some(height),
        }),
    ));
    let output = graph.add_node(Node::new(NodeId(0), NodeKind::MediaOutput(MediaOutput)));
    graph
        .connect(Connection {
            from_node: solid,
            from_port: OutputPort::default(),
            to_node: output,
            to_port: InputPort::named("source"),
        })
        .map_err(|error| error.to_string())?;

    Ok(Composition::new(
        graph,
        TimelineSettings {
            fps,
            duration_frames: total_frames,
        },
        RenderSettings {
            width,
            height,
            background_color: project.canvas.background,
        },
    ))
}

fn renderer_from_json(bytes: &[u8], scale: f32) -> Result<RendererSession, String> {
    if let Ok(project) = serde_json::from_slice::<PreviewProjectInput>(bytes) {
        return preview_project_to_composition(project, scale).map(RendererSession::new);
    }

    let payload =
        std::str::from_utf8(bytes).map_err(|_| "project payload is not valid utf-8".to_string())?;
    let result = Composition::from_json(payload);
    if let Some(composition) = result.composition {
        return Ok(RendererSession::new(composition));
    }
    let message = result
        .errors
        .first()
        .map(ToString::to_string)
        .unwrap_or_else(|| "failed to parse composition json".to_string());
    Err(message)
}

fn validate_rgba_len(width: u32, height: u32, len: usize) -> bool {
    u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|px| px.checked_mul(4))
        .and_then(|bytes| usize::try_from(bytes).ok())
        .is_some_and(|expected| expected == len)
}

fn render_into_session(
    session: &mut RendererSession,
    frame: u32,
    media_store: Arc<InMemoryMediaStore>,
) -> Result<*const u8, String> {
    let capability_profile = RuntimeCapabilityProfile {
        has_image_resolver: true,
        has_video_resolver: true,
        has_threading: false,
        sink_types: vec![lumen::SinkType::Bitmap],
    };
    let surface_pool = Arc::new(SurfacePool::new());
    let media_store: Arc<dyn MediaStore> = media_store;
    let mut ctx = RenderContext::new(
        &session.composition,
        Arc::clone(&surface_pool),
        Arc::clone(&session.asset_cache),
        media_store,
        capability_profile,
    );
    let raster = session
        .composition
        .render_frame(frame, &mut ctx)
        .map_err(|error| error.to_string())?;
    let bitmap = raster.to_bitmap().map_err(|error| error.to_string())?;
    let Some(bytes) = bitmap.as_bitmap_bytes() else {
        return Err("render did not produce bitmap bytes".to_string());
    };
    session.last_frame.clear();
    session.last_frame.extend_from_slice(bytes);
    session.last_error.clear();
    Ok(session.last_frame.as_ptr())
}

#[unsafe(no_mangle)]
pub extern "C" fn lumen_wasm_version() -> *const c_char {
    VERSION.as_ptr() as *const c_char
}

#[unsafe(no_mangle)]
pub extern "C" fn lumen_wasm_last_status_ptr() -> *const u8 {
    REGISTRY.with(|registry| registry.borrow().last_status.as_ptr())
}

#[unsafe(no_mangle)]
pub extern "C" fn lumen_wasm_last_status_len() -> usize {
    REGISTRY.with(|registry| registry.borrow().last_status.len())
}

#[unsafe(no_mangle)]
pub extern "C" fn lumen_wasm_load_project(
    project_ptr: *const u8,
    project_len: usize,
    scale: f32,
) -> u32 {
    let bytes = unsafe { read_bytes(project_ptr, project_len) };
    let bytes = match bytes {
        Ok(bytes) => bytes,
        Err(message) => {
            REGISTRY.with(|registry| {
                registry
                    .borrow_mut()
                    .set_status("error", "invalid_input", message)
            });
            return 0;
        }
    };
    let session = match renderer_from_json(&bytes, scale) {
        Ok(session) => session,
        Err(message) => {
            REGISTRY.with(|registry| {
                registry
                    .borrow_mut()
                    .set_status("error", "invalid_input", &message)
            });
            return 0;
        }
    };

    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        let handle = registry.alloc_handle();
        if handle == 0 {
            registry.set_status(
                "error",
                "internal_error",
                "unable to allocate renderer handle",
            );
            return 0;
        }
        registry.renderers.insert(handle, session);
        registry.set_status("ok", "ok", "project loaded");
        handle
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lumen_wasm_unload_project(renderer: u32) {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        if registry.renderers.remove(&renderer).is_some() {
            registry.set_status("ok", "ok", "renderer destroyed")
        } else {
            registry.set_status("error", "invalid_input", "renderer handle not found")
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lumen_wasm_project_width(renderer: u32) -> u32 {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        let width = registry
            .renderers
            .get(&renderer)
            .map(RendererSession::width)
            .unwrap_or(0);
        if width == 0 {
            registry.set_status("error", "invalid_input", "renderer handle not found")
        }
        width
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lumen_wasm_project_height(renderer: u32) -> u32 {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        let height = registry
            .renderers
            .get(&renderer)
            .map(RendererSession::height)
            .unwrap_or(0);
        if height == 0 {
            registry.set_status("error", "invalid_input", "renderer handle not found")
        }
        height
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lumen_wasm_request_frame(renderer: u32, frame: u64, media: u32) -> *const u8 {
    let frame = match u32::try_from(frame) {
        Ok(value) => value,
        Err(_) => {
            REGISTRY.with(|registry| {
                registry.borrow_mut().set_status(
                    "error",
                    "invalid_input",
                    "frame index out of range",
                )
            });
            return ptr::null();
        }
    };

    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        let Some(media_store) = registry.media_stores.get(&media).cloned() else {
            registry.set_status("error", "invalid_input", "media store handle not found");
            return ptr::null();
        };
        let render_result = {
            let Some(session) = registry.renderers.get_mut(&renderer) else {
                registry.set_status("error", "invalid_input", "renderer handle not found");
                return ptr::null();
            };
            render_into_session(session, frame, media_store)
        };
        match render_result {
            Ok(ptr_out) => {
                registry.set_status("ok", "ok", "frame rendered");
                ptr_out
            }
            Err(message) => {
                if let Some(session) = registry.renderers.get_mut(&renderer) {
                    session.last_error = message.as_bytes().to_vec();
                }
                registry.set_status("error", "internal_error", &message);
                ptr::null()
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lumen_wasm_request_frame_len(renderer: u32) -> usize {
    REGISTRY.with(|registry| {
        registry
            .borrow()
            .renderers
            .get(&renderer)
            .map(|session| session.last_frame.len())
            .unwrap_or(0)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lumen_wasm_request_frame_requirements(renderer: u32, _frame: u64) -> *const u8 {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        let ptr_out = {
            let Some(session) = registry.renderers.get_mut(&renderer) else {
                registry.set_status("error", "invalid_input", "renderer handle not found");
                return ptr::null();
            };
            session.last_frame_requirements.as_ptr()
        };
        registry.set_status("ok", "ok", "frame requirements ready");
        ptr_out
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lumen_wasm_request_frame_requirements_len(renderer: u32) -> usize {
    REGISTRY.with(|registry| {
        registry
            .borrow()
            .renderers
            .get(&renderer)
            .map(|session| session.last_frame_requirements.len())
            .unwrap_or(0)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lumen_wasm_last_error_ptr(renderer: u32) -> *const u8 {
    REGISTRY.with(|registry| {
        registry
            .borrow()
            .renderers
            .get(&renderer)
            .map(|session| session.last_error.as_ptr())
            .unwrap_or(ptr::null())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lumen_wasm_last_error_len(renderer: u32) -> usize {
    REGISTRY.with(|registry| {
        registry
            .borrow()
            .renderers
            .get(&renderer)
            .map(|session| session.last_error.len())
            .unwrap_or(0)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lumen_wasm_media_store_create() -> u32 {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        let handle = registry.alloc_handle();
        if handle == 0 {
            registry.set_status(
                "error",
                "internal_error",
                "unable to allocate media store handle",
            );
            return 0;
        }
        registry
            .media_stores
            .insert(handle, Arc::new(InMemoryMediaStore::default()));
        registry.set_status("ok", "ok", "media store created");
        handle
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lumen_wasm_media_store_destroy(media: u32) {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        if registry.media_stores.remove(&media).is_some() {
            registry.set_status("ok", "ok", "media store destroyed")
        } else {
            registry.set_status("error", "invalid_input", "media store handle not found")
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lumen_wasm_media_store_clear(media: u32) {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        let Some(store) = registry.media_stores.get(&media).cloned() else {
            registry.set_status("error", "invalid_input", "media store handle not found");
            return;
        };
        let images_ok = store.images.write().map(|mut map| map.clear()).is_ok();
        let videos_ok = store.videos.write().map(|mut map| map.clear()).is_ok();
        if images_ok && videos_ok {
            registry.set_status("ok", "ok", "media store cleared")
        } else {
            registry.set_status("error", "internal_error", "media store lock poisoned")
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lumen_wasm_media_store_clear_videos(media: u32) {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        let Some(store) = registry.media_stores.get(&media).cloned() else {
            registry.set_status("error", "invalid_input", "media store handle not found");
            return;
        };
        if store.videos.write().map(|mut map| map.clear()).is_ok() {
            registry.set_status("ok", "ok", "video frames cleared")
        } else {
            registry.set_status("error", "internal_error", "media store lock poisoned")
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lumen_wasm_media_store_has_image(
    media: u32,
    source_ptr: *const u8,
    source_len: usize,
) -> i32 {
    let source = unsafe { read_string(source_ptr, source_len) };
    let source = match source {
        Ok(source) => source,
        Err(message) => {
            REGISTRY.with(|registry| {
                registry
                    .borrow_mut()
                    .set_status("error", "invalid_input", message)
            });
            return 0;
        }
    };
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        let Some(store) = registry.media_stores.get(&media).cloned() else {
            registry.set_status("error", "invalid_input", "media store handle not found");
            return 0;
        };
        let has = store
            .images
            .read()
            .ok()
            .is_some_and(|images| images.contains_key(&source));
        registry.set_status("ok", "ok", "media lookup complete");
        if has { 1 } else { 0 }
    })
}

fn insert_image(
    media: u32,
    source_ptr: *const u8,
    source_len: usize,
    width: u32,
    height: u32,
    mut rgba: Vec<u8>,
) -> i32 {
    if width == 0 || height == 0 {
        REGISTRY.with(|registry| {
            registry.borrow_mut().set_status(
                "error",
                "invalid_input",
                "image dimensions must be > 0",
            )
        });
        return 0;
    }
    if !validate_rgba_len(width, height, rgba.len()) {
        REGISTRY.with(|registry| {
            registry.borrow_mut().set_status(
                "error",
                "invalid_input",
                "invalid image rgba buffer length",
            )
        });
        return 0;
    }
    let source = unsafe { read_string(source_ptr, source_len) };
    let source = match source {
        Ok(source) if !source.is_empty() => source,
        Ok(_) => {
            REGISTRY.with(|registry| {
                registry.borrow_mut().set_status(
                    "error",
                    "invalid_input",
                    "source id must be non-empty",
                )
            });
            return 0;
        }
        Err(message) => {
            REGISTRY.with(|registry| {
                registry
                    .borrow_mut()
                    .set_status("error", "invalid_input", message)
            });
            return 0;
        }
    };

    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        let Some(store) = registry.media_stores.get(&media).cloned() else {
            registry.set_status("error", "invalid_input", "media store handle not found");
            return 0;
        };
        premultiply_rgba_in_place_if_needed(&mut rgba);
        let result = store.images.write().map(|mut images| {
            images.insert(
                source,
                StoredImage {
                    width,
                    height,
                    pixels: Arc::new(rgba),
                },
            );
        });
        if result.is_ok() {
            registry.set_status("ok", "ok", "image uploaded");
            1
        } else {
            registry.set_status("error", "internal_error", "media store lock poisoned");
            0
        }
    })
}

fn insert_video_frame(
    media: u32,
    source_ptr: *const u8,
    source_len: usize,
    frame: u64,
    width: u32,
    height: u32,
    mut rgba: Vec<u8>,
) -> i32 {
    let frame = match u32::try_from(frame) {
        Ok(frame) => frame,
        Err(_) => {
            REGISTRY.with(|registry| {
                registry.borrow_mut().set_status(
                    "error",
                    "invalid_input",
                    "video frame index out of range",
                )
            });
            return 0;
        }
    };
    if width == 0 || height == 0 {
        REGISTRY.with(|registry| {
            registry.borrow_mut().set_status(
                "error",
                "invalid_input",
                "video frame dimensions must be > 0",
            )
        });
        return 0;
    }
    if !validate_rgba_len(width, height, rgba.len()) {
        REGISTRY.with(|registry| {
            registry.borrow_mut().set_status(
                "error",
                "invalid_input",
                "invalid video rgba buffer length",
            )
        });
        return 0;
    }
    let source = unsafe { read_string(source_ptr, source_len) };
    let source = match source {
        Ok(source) if !source.is_empty() => source,
        Ok(_) => {
            REGISTRY.with(|registry| {
                registry.borrow_mut().set_status(
                    "error",
                    "invalid_input",
                    "source id must be non-empty",
                )
            });
            return 0;
        }
        Err(message) => {
            REGISTRY.with(|registry| {
                registry
                    .borrow_mut()
                    .set_status("error", "invalid_input", message)
            });
            return 0;
        }
    };

    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        let Some(store) = registry.media_stores.get(&media).cloned() else {
            registry.set_status("error", "invalid_input", "media store handle not found");
            return 0;
        };
        premultiply_rgba_in_place_if_needed(&mut rgba);
        let result = store.videos.write().map(|mut videos| {
            let entry = videos.entry(source).or_default();
            entry.width = width;
            entry.height = height;
            entry.frames.insert(frame, Arc::new(rgba));
        });
        if result.is_ok() {
            registry.set_status("ok", "ok", "video frame uploaded");
            1
        } else {
            registry.set_status("error", "internal_error", "media store lock poisoned");
            0
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lumen_wasm_media_store_set_image(
    media: u32,
    source_ptr: *const u8,
    source_len: usize,
    width: u32,
    height: u32,
    rgba_ptr: *const u8,
    rgba_len: usize,
) -> i32 {
    let rgba = unsafe { read_bytes(rgba_ptr, rgba_len) };
    match rgba {
        Ok(rgba) => insert_image(media, source_ptr, source_len, width, height, rgba),
        Err(message) => {
            REGISTRY.with(|registry| {
                registry
                    .borrow_mut()
                    .set_status("error", "invalid_input", message)
            });
            0
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn lumen_wasm_media_store_set_image_owned(
    media: u32,
    source_ptr: *const u8,
    source_len: usize,
    width: u32,
    height: u32,
    rgba_ptr: *mut u8,
    rgba_len: usize,
) -> i32 {
    let rgba = unsafe { take_owned_bytes(rgba_ptr, rgba_len) };
    match rgba {
        Ok(rgba) => insert_image(media, source_ptr, source_len, width, height, rgba),
        Err(message) => {
            REGISTRY.with(|registry| {
                registry
                    .borrow_mut()
                    .set_status("error", "invalid_input", message)
            });
            0
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn lumen_wasm_media_store_set_video_frame(
    media: u32,
    source_ptr: *const u8,
    source_len: usize,
    frame: u64,
    width: u32,
    height: u32,
    rgba_ptr: *const u8,
    rgba_len: usize,
) -> i32 {
    let rgba = unsafe { read_bytes(rgba_ptr, rgba_len) };
    match rgba {
        Ok(rgba) => insert_video_frame(media, source_ptr, source_len, frame, width, height, rgba),
        Err(message) => {
            REGISTRY.with(|registry| {
                registry
                    .borrow_mut()
                    .set_status("error", "invalid_input", message)
            });
            0
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn lumen_wasm_media_store_set_video_frame_owned(
    media: u32,
    source_ptr: *const u8,
    source_len: usize,
    frame: u64,
    width: u32,
    height: u32,
    rgba_ptr: *mut u8,
    rgba_len: usize,
) -> i32 {
    let rgba = unsafe { take_owned_bytes(rgba_ptr, rgba_len) };
    match rgba {
        Ok(rgba) => insert_video_frame(media, source_ptr, source_len, frame, width, height, rgba),
        Err(message) => {
            REGISTRY.with(|registry| {
                registry
                    .borrow_mut()
                    .set_status("error", "invalid_input", message)
            });
            0
        }
    }
}
