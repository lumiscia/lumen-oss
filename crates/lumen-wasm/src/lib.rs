use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    ffi::{c_char, c_void},
    ptr,
    sync::{Arc, RwLock},
};

use lumen::{
    composition::{Composition, RenderSettings, TimelineSettings},
    graph::{Connection, Graph},
    media::{
        FrameRequirements, ImageMetadata, ImageResolver, MediaStore, VideoFrameRequirement,
        VideoFrameResolver, VideoMetadata, collect_frame_requirements,
        premultiply_rgba_in_place_if_needed,
    },
    node::{
        NodeId, NodeKind, NodeProperty, PortRef, media_output::MediaOutput,
        source::solid_color::SolidColor,
    },
    raster::{AlphaMode, ImageFrame, RectI},
    render::{LumenRenderer, surface::DefaultSurfacePool},
};
use serde::{Deserialize, Serialize};

static VERSION: &[u8] = b"lumen-wasm-next\0";
static EMPTY_FRAME_REQUIREMENTS: &[u8] = br#"{"images":[],"videos":[]}"#;
const DEFAULT_VIDEO_FRAME_CAPACITY: usize = 96;

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
    media_stores: HashMap<u32, WasmMediaStore>,
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
    surface_pool: DefaultSurfacePool,
    last_frame: Vec<u8>,
    last_frame_requirements: Vec<u8>,
    last_error: Vec<u8>,
}

impl RendererSession {
    fn new(composition: Composition) -> Self {
        Self {
            composition,
            surface_pool: DefaultSurfacePool::new(),
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

#[derive(Debug, Clone, Default)]
struct WasmMediaStore {
    inner: Arc<WasmMediaStoreInner>,
}

#[derive(Debug, Default)]
struct WasmMediaStoreInner {
    images: RwLock<HashMap<String, StoredImage>>,
    videos: RwLock<HashMap<String, StoredVideo>>,
}

#[derive(Debug, Clone)]
struct StoredImage {
    metadata: ImageMetadata,
    frame: Arc<ImageFrame>,
}

#[derive(Debug, Clone)]
struct StoredVideo {
    metadata: VideoMetadata,
    frames: VideoFrameCache,
}

impl Default for StoredVideo {
    fn default() -> Self {
        Self {
            metadata: VideoMetadata::default(),
            frames: VideoFrameCache::with_capacity(DEFAULT_VIDEO_FRAME_CAPACITY),
        }
    }
}

#[derive(Debug, Clone)]
struct VideoFrameCache {
    capacity: usize,
    order: VecDeque<u32>,
    entries: HashMap<u32, Arc<ImageFrame>>,
}

impl VideoFrameCache {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            order: VecDeque::new(),
            entries: HashMap::new(),
        }
    }

    fn clear(&mut self) {
        self.order.clear();
        self.entries.clear();
    }

    fn get(&self, frame: u32) -> Option<Arc<ImageFrame>> {
        self.entries.get(&frame).cloned()
    }

    fn insert(&mut self, frame: u32, image: Arc<ImageFrame>) {
        if let std::collections::hash_map::Entry::Occupied(mut existing) = self.entries.entry(frame)
        {
            existing.insert(image);
            self.touch(frame);
            return;
        }

        self.entries.insert(frame, image);
        self.order.push_back(frame);
        while self.entries.len() > self.capacity {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            } else {
                break;
            }
        }
    }

    fn touch(&mut self, frame: u32) {
        if let Some(index) = self.order.iter().position(|existing| *existing == frame) {
            self.order.remove(index);
        }
        self.order.push_back(frame);
    }
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

    fn metadata(&self) -> ImageMetadata {
        self.entry.metadata
    }

    fn resolve_image(&self) -> Result<Arc<ImageFrame>, lumen::error::MediaError> {
        Ok(Arc::clone(&self.entry.frame))
    }
}

impl VideoFrameResolver for WasmVideoResolver {
    fn id(&self) -> &str {
        &self.id
    }

    fn metadata(&self) -> VideoMetadata {
        self.entry.metadata
    }

    fn resolve_frame_image(&self, frame: u32) -> Result<Arc<ImageFrame>, lumen::error::MediaError> {
        let Some(image) = self.entry.frames.get(frame) else {
            return Err(lumen::error::MediaError::FrameOutOfRange {
                media_source: self.id.clone(),
                frame,
                frame_count: self.entry.metadata.frame_count,
            });
        };
        Ok(image)
    }
}

impl WasmMediaStore {
    fn clear(&self) -> Result<(), &'static str> {
        self.inner
            .images
            .write()
            .map_err(|_| "media store lock poisoned")?
            .clear();
        self.inner
            .videos
            .write()
            .map_err(|_| "media store lock poisoned")?
            .clear();
        Ok(())
    }

    fn clear_video_frames(&self) -> Result<(), &'static str> {
        let mut videos = self
            .inner
            .videos
            .write()
            .map_err(|_| "media store lock poisoned")?;
        for video in videos.values_mut() {
            video.frames.clear();
        }
        Ok(())
    }

    fn clear_video_frames_for_source(&self, source: &str) -> Result<bool, &'static str> {
        let mut videos = self
            .inner
            .videos
            .write()
            .map_err(|_| "media store lock poisoned")?;
        let Some(video) = videos.get_mut(source) else {
            return Ok(false);
        };
        video.frames.clear();
        Ok(true)
    }

    fn has_image(&self, source: &str) -> bool {
        self.inner
            .images
            .read()
            .ok()
            .is_some_and(|images| images.contains_key(source))
    }

    fn set_image(&self, source: String, frame: ImageFrame) -> Result<(), &'static str> {
        let metadata = ImageMetadata {
            width: frame.storage_width,
            height: frame.storage_height,
        };
        self.inner
            .images
            .write()
            .map_err(|_| "media store lock poisoned")?
            .insert(
                source,
                StoredImage {
                    metadata,
                    frame: Arc::new(frame),
                },
            );
        Ok(())
    }

    fn set_video_metadata(
        &self,
        source: String,
        metadata: VideoMetadata,
    ) -> Result<(), &'static str> {
        let mut videos = self
            .inner
            .videos
            .write()
            .map_err(|_| "media store lock poisoned")?;
        let entry = videos.entry(source).or_default();
        entry.metadata = metadata;
        Ok(())
    }

    fn set_video_frame(
        &self,
        source: String,
        frame: u32,
        image: ImageFrame,
    ) -> Result<(), &'static str> {
        let mut videos = self
            .inner
            .videos
            .write()
            .map_err(|_| "media store lock poisoned")?;
        let entry = videos.entry(source).or_default();
        entry.metadata.width = image.storage_width;
        entry.metadata.height = image.storage_height;
        entry.metadata.frame_count = entry.metadata.frame_count.max(frame.saturating_add(1));
        entry.frames.insert(frame, Arc::new(image));
        Ok(())
    }
}

impl MediaStore for WasmMediaStore {
    fn get_image_resolver(&self, source: &str) -> Option<Box<dyn ImageResolver>> {
        let images = self.inner.images.read().ok()?;
        let entry = images.get(source)?.clone();
        Some(Box::new(WasmImageResolver {
            id: source.to_string(),
            entry,
        }))
    }

    fn get_video_resolver(&self, source: &str) -> Option<Box<dyn VideoFrameResolver>> {
        let videos = self.inner.videos.read().ok()?;
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

#[derive(Serialize)]
struct FrameRequirementsPayload {
    images: Vec<String>,
    videos: Vec<FrameRequirementsVideoPayload>,
}

#[derive(Serialize)]
struct FrameRequirementsVideoPayload {
    #[serde(rename = "sourceId")]
    source_id: String,
    frames: Vec<u32>,
}

impl From<FrameRequirements> for FrameRequirementsPayload {
    fn from(value: FrameRequirements) -> Self {
        Self {
            images: value.images,
            videos: value
                .videos
                .into_iter()
                .map(|video| FrameRequirementsVideoPayload::from(video))
                .collect(),
        }
    }
}

impl From<VideoFrameRequirement> for FrameRequirementsVideoPayload {
    fn from(value: VideoFrameRequirement) -> Self {
        Self {
            source_id: value.source_id,
            frames: value.frames,
        }
    }
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

fn read_non_empty_source_id(
    source_ptr: *const u8,
    source_len: usize,
) -> Result<String, &'static str> {
    match unsafe { read_string(source_ptr, source_len) } {
        Ok(source) if !source.is_empty() => Ok(source),
        Ok(_) => Err("source id must be non-empty"),
        Err(message) => Err(message),
    }
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

    let solid_id = NodeId::new(1);
    let output_id = NodeId::new(2);
    let mut graph = Graph::new();
    graph.nodes.insert(
        solid_id,
        NodeKind::SolidColor(SolidColor {
            id: solid_id,
            color: NodeProperty::Color(project.canvas.background),
            width: NodeProperty::Int(i64::from(width)),
            height: NodeProperty::Int(i64::from(height)),
        }),
    );
    graph.nodes.insert(
        output_id,
        NodeKind::MediaOutput(MediaOutput {
            id: output_id,
            source: PortRef::new(solid_id, "output".to_string()),
        }),
    );
    graph
        .connect(Connection {
            from_node: solid_id,
            from_port: "output".to_string(),
            to_node: output_id,
            to_port: "source".to_string(),
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
    let composition = lumen::json::parse(payload).map_err(|error| error.to_string())?;
    Ok(RendererSession::new(composition))
}

fn validate_rgba_len(width: u32, height: u32, len: usize) -> bool {
    u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|px| px.checked_mul(4))
        .and_then(|bytes| usize::try_from(bytes).ok())
        .is_some_and(|expected| expected == len)
}

fn image_frame_from_rgba(width: u32, height: u32, mut rgba: Vec<u8>) -> Result<ImageFrame, String> {
    premultiply_rgba_in_place_if_needed(&mut rgba);
    let rect = RectI::from_size(width, height);
    ImageFrame::from_rgba_bytes(
        rgba.as_slice(),
        width,
        height,
        (width as usize) * 4,
        AlphaMode::Premultiplied,
        rect,
        rect,
    )
    .map_err(|error| error.to_string())
}

fn render_into_session(
    session: &mut RendererSession,
    frame: u32,
    media_store: &WasmMediaStore,
) -> Result<*const u8, String> {
    let mut renderer = LumenRenderer::new(&session.composition, &session.surface_pool, media_store)
        .map_err(|error| error.to_string())?;
    let raster = renderer.render(frame).map_err(|error| error.to_string())?;
    let (storage_width, storage_height) = raster.storage_dimensions();
    let mut pixels = vec![0; (storage_width as usize) * (storage_height as usize) * 4];
    raster
        .read_pixels_into(pixels.as_mut_slice(), (storage_width as usize) * 4)
        .map_err(|error| error.to_string())?;
    session.last_frame.clear();
    session.last_frame.extend_from_slice(pixels.as_slice());
    session.last_error.clear();
    Ok(session.last_frame.as_ptr())
}

fn collect_requirements_into_session(
    session: &mut RendererSession,
    frame: u32,
    media_store: &WasmMediaStore,
) -> Result<*const u8, String> {
    let payload = collect_frame_requirements(&session.composition, media_store, frame)
        .map(FrameRequirementsPayload::from)
        .map_err(|error| error.to_string())?;
    session.last_frame_requirements =
        serde_json::to_vec(&payload).map_err(|error| error.to_string())?;
    session.last_error.clear();
    Ok(session.last_frame_requirements.as_ptr())
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
            render_into_session(session, frame, &media_store)
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
pub extern "C" fn lumen_wasm_request_frame_requirements(
    renderer: u32,
    frame: u64,
    media: u32,
) -> *const u8 {
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
        let requirements_result = {
            let Some(session) = registry.renderers.get_mut(&renderer) else {
                registry.set_status("error", "invalid_input", "renderer handle not found");
                return ptr::null();
            };
            collect_requirements_into_session(session, frame, &media_store)
        };
        match requirements_result {
            Ok(ptr_out) => {
                registry.set_status("ok", "ok", "frame requirements ready");
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
            .insert(handle, WasmMediaStore::default());
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
        match store.clear() {
            Ok(()) => registry.set_status("ok", "ok", "media store cleared"),
            Err(message) => registry.set_status("error", "internal_error", message),
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
        match store.clear_video_frames() {
            Ok(()) => registry.set_status("ok", "ok", "video frames cleared"),
            Err(message) => registry.set_status("error", "internal_error", message),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lumen_wasm_media_store_clear_video_source(
    media: u32,
    source_ptr: *const u8,
    source_len: usize,
) {
    let source = match read_non_empty_source_id(source_ptr, source_len) {
        Ok(source) => source,
        Err(message) => {
            REGISTRY.with(|registry| {
                registry
                    .borrow_mut()
                    .set_status("error", "invalid_input", message)
            });
            return;
        }
    };

    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        let Some(store) = registry.media_stores.get(&media).cloned() else {
            registry.set_status("error", "invalid_input", "media store handle not found");
            return;
        };
        match store.clear_video_frames_for_source(&source) {
            Ok(true) => registry.set_status("ok", "ok", "video source frames cleared"),
            Ok(false) => registry.set_status("error", "invalid_input", "video source not found"),
            Err(message) => registry.set_status("error", "internal_error", message),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lumen_wasm_media_store_has_image(
    media: u32,
    source_ptr: *const u8,
    source_len: usize,
) -> i32 {
    let source = match read_non_empty_source_id(source_ptr, source_len) {
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
        let has = store.has_image(&source);
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
    rgba: Vec<u8>,
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
    let source = match read_non_empty_source_id(source_ptr, source_len) {
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
    let frame = match image_frame_from_rgba(width, height, rgba) {
        Ok(frame) => frame,
        Err(message) => {
            REGISTRY.with(|registry| {
                registry
                    .borrow_mut()
                    .set_status("error", "internal_error", &message)
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
        match store.set_image(source, frame) {
            Ok(()) => {
                registry.set_status("ok", "ok", "image uploaded");
                1
            }
            Err(message) => {
                registry.set_status("error", "internal_error", message);
                0
            }
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
    rgba: Vec<u8>,
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
    let source = match read_non_empty_source_id(source_ptr, source_len) {
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
    let image = match image_frame_from_rgba(width, height, rgba) {
        Ok(frame) => frame,
        Err(message) => {
            REGISTRY.with(|registry| {
                registry
                    .borrow_mut()
                    .set_status("error", "internal_error", &message)
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
        match store.set_video_frame(source, frame, image) {
            Ok(()) => {
                registry.set_status("ok", "ok", "video frame uploaded");
                1
            }
            Err(message) => {
                registry.set_status("error", "internal_error", message);
                0
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lumen_wasm_media_store_set_video_metadata(
    media: u32,
    source_ptr: *const u8,
    source_len: usize,
    width: u32,
    height: u32,
    frame_count: u64,
) -> i32 {
    let frame_count = match u32::try_from(frame_count) {
        Ok(value) => value,
        Err(_) => {
            REGISTRY.with(|registry| {
                registry.borrow_mut().set_status(
                    "error",
                    "invalid_input",
                    "video frame_count is out of range",
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
                "video dimensions must be > 0",
            )
        });
        return 0;
    }
    let source = match read_non_empty_source_id(source_ptr, source_len) {
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
        match store.set_video_metadata(
            source,
            VideoMetadata {
                width,
                height,
                frame_count,
            },
        ) {
            Ok(()) => {
                registry.set_status("ok", "ok", "video metadata stored");
                1
            }
            Err(message) => {
                registry.set_status("error", "internal_error", message);
                0
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clear_videos_preserves_metadata() {
        let store = WasmMediaStore::default();
        store
            .set_video_metadata(
                "intro".to_string(),
                VideoMetadata {
                    width: 1920,
                    height: 1080,
                    frame_count: 240,
                },
            )
            .expect("set metadata");
        let frame = image_frame_from_rgba(1, 1, vec![255, 0, 0, 255]).expect("frame");
        store
            .set_video_frame("intro".to_string(), 0, frame)
            .expect("set frame");

        store.clear_video_frames().expect("clear video frames");

        let resolver = store
            .get_video_resolver("intro")
            .expect("video resolver should still exist");
        assert_eq!(resolver.metadata().frame_count, 240);
        assert!(matches!(
            resolver.resolve_frame_image(0),
            Err(lumen::error::MediaError::FrameOutOfRange { .. })
        ));
    }
}
