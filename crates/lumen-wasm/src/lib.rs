use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    slice, str,
    sync::Arc,
};

use lumen::{
    Layer, Project, Rational,
    clip::{
        Clip, ClipType,
        layout::{LayoutContent, LayoutNode},
    },
    json::{JsonDelegateRequest, JsonDelegateStatus, convert_json_delegate, json_delegate_enabled},
    media::{ImageResolver, MediaStore, VideoResolver},
    render::{backend::pixel_len, context::RendererContext, render_scene},
};
use serde::Serialize;

const ABI_MAJOR: u32 = 1;
const ABI_MINOR: u32 = 0;
const RENDERER_CONTRACT_ID: &str = "lumiscia.renderer";
const SCHEMA_REVISION: &str = "chat_story_v1";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WasmContractMetadata {
    abi_major: u32,
    abi_minor: u32,
    renderer_contract: &'static str,
    schema_revision: &'static str,
    capabilities: Vec<&'static str>,
    limits: WasmContractLimits,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WasmContractLimits {
    max_layers: u32,
    max_items_per_layer: u32,
    max_timeline_frames: u32,
}

#[derive(Serialize)]
struct StatusPayload {
    status: &'static str,
    code: &'static str,
    message: String,
}

thread_local! {
    static LAST_STATUS_PAYLOAD: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    static LAST_CONTRACT_METADATA: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

fn set_status_payload(status: &'static str, code: &'static str, message: impl Into<String>) {
    let payload = StatusPayload {
        status,
        code,
        message: message.into(),
    };
    let bytes = serde_json::to_vec(&payload).unwrap_or_default();
    LAST_STATUS_PAYLOAD.with(|buffer| {
        *buffer.borrow_mut() = bytes;
    });
}

fn contract_capabilities() -> Vec<&'static str> {
    let mut capabilities = vec!["media_owned_upload"];
    if json_delegate_enabled() {
        capabilities.insert(0, "json_delegate");
    }
    capabilities
}

fn ensure_contract_metadata() {
    LAST_CONTRACT_METADATA.with(|buffer| {
        if !buffer.borrow().is_empty() {
            return;
        }
        let metadata = WasmContractMetadata {
            abi_major: ABI_MAJOR,
            abi_minor: ABI_MINOR,
            renderer_contract: RENDERER_CONTRACT_ID,
            schema_revision: SCHEMA_REVISION,
            capabilities: contract_capabilities(),
            limits: WasmContractLimits {
                max_layers: 128,
                max_items_per_layer: 2048,
                max_timeline_frames: 259_200,
            },
        };
        let bytes = serde_json::to_vec(&metadata).unwrap_or_default();
        *buffer.borrow_mut() = bytes;
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn lumen_contract_metadata_ptr() -> *const u8 {
    ensure_contract_metadata();
    LAST_CONTRACT_METADATA.with(|buffer| {
        let borrowed = buffer.borrow();
        if borrowed.is_empty() {
            std::ptr::null()
        } else {
            borrowed.as_ptr()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lumen_contract_metadata_len() -> usize {
    ensure_contract_metadata();
    LAST_CONTRACT_METADATA.with(|buffer| buffer.borrow().len())
}

#[unsafe(no_mangle)]
pub extern "C" fn lumen_last_status_ptr() -> *const u8 {
    LAST_STATUS_PAYLOAD.with(|buffer| {
        let borrowed = buffer.borrow();
        if borrowed.is_empty() {
            std::ptr::null()
        } else {
            borrowed.as_ptr()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lumen_last_status_len() -> usize {
    LAST_STATUS_PAYLOAD.with(|buffer| buffer.borrow().len())
}

#[unsafe(no_mangle)]
pub extern "C" fn lumen_renderer_handshake(expected_abi_major: u32) -> u32 {
    ensure_contract_metadata();
    if expected_abi_major != ABI_MAJOR {
        set_status_payload(
            "rejected",
            "contract_mismatch",
            format!(
                "expected abi_major={}, got {}",
                expected_abi_major, ABI_MAJOR
            ),
        );
        return 2;
    }
    set_status_payload("ok", "ok", "handshake successful");
    0
}

#[derive(Debug, Clone)]
struct FrameData {
    width: u32,
    height: u32,
    pixels_rgba: Arc<Vec<u8>>,
}

#[derive(Debug, Clone)]
struct VideoSourceFrames {
    width: u32,
    height: u32,
    frames: HashMap<u64, FrameData>,
}

#[derive(Debug, Clone, Default)]
pub struct WasmMediaStore {
    images: HashMap<String, FrameData>,
    videos: HashMap<String, VideoSourceFrames>,
}

impl WasmMediaStore {
    fn clear(&mut self) {
        self.images.clear();
        self.videos.clear();
    }
}

#[derive(Debug, Clone)]
struct StaticImageResolver {
    id: String,
    frame: FrameData,
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

#[derive(Debug, Clone)]
struct StaticVideoResolver {
    id: String,
    width: u32,
    height: u32,
    frames: HashMap<u64, FrameData>,
}

impl StaticVideoResolver {
    fn zero_frame(&self) -> Vec<u8> {
        pixel_len(self.width, self.height)
            .map(|len| vec![0; len])
            .unwrap_or_default()
    }
}

impl VideoResolver for StaticVideoResolver {
    fn id(&self) -> String {
        self.id.clone()
    }

    fn width(&self) -> u32 {
        self.width
    }

    fn height(&self) -> u32 {
        self.height
    }

    fn resolve_frame(&mut self, frame: u32) -> Vec<u8> {
        if let Some(exact) = self.frames.get(&(frame as u64)) {
            return (*exact.pixels_rgba).clone();
        }

        let mut before: Option<(u64, &FrameData)> = None;
        let mut after: Option<(u64, &FrameData)> = None;
        for (candidate_idx, candidate) in &self.frames {
            if *candidate_idx <= frame as u64 {
                match before {
                    Some((prev_idx, _)) if prev_idx >= *candidate_idx => {}
                    _ => before = Some((*candidate_idx, candidate)),
                }
            }
            if *candidate_idx > frame as u64 {
                match after {
                    Some((next_idx, _)) if next_idx <= *candidate_idx => {}
                    _ => after = Some((*candidate_idx, candidate)),
                }
            }
        }

        if let Some((_, nearest)) = before.or(after) {
            return (*nearest.pixels_rgba).clone();
        }

        self.zero_frame()
    }
}

impl MediaStore for WasmMediaStore {
    fn get_image_resolver(&mut self, id: &str) -> Option<Box<dyn ImageResolver>> {
        let frame = self.images.get(id)?.clone();
        Some(Box::new(StaticImageResolver {
            id: id.to_string(),
            frame,
        }))
    }

    fn get_video_resolver(&mut self, id: &str) -> Option<Box<dyn VideoResolver>> {
        let source = self.videos.get(id)?.clone();
        Some(Box::new(StaticVideoResolver {
            id: id.to_string(),
            width: source.width,
            height: source.height,
            frames: source.frames,
        }))
    }
}

pub struct WasmRenderer {
    project: Arc<Project>,
    renderer_ctx: RendererContext,
    last_frame: Vec<u8>,
    last_requirements: Vec<u8>,
    last_error: Option<String>,
}

impl WasmRenderer {
    fn set_error(&mut self, error: impl Into<String>) {
        self.last_error = Some(error.into());
    }

    fn clear_error(&mut self) {
        self.last_error = None;
    }
}

#[derive(Debug, Serialize)]
struct FrameRequirementsPayload {
    images: Vec<String>,
    videos: Vec<VideoRequirementsPayload>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct VideoRequirementsPayload {
    source_id: String,
    frames: Vec<u64>,
}

fn collect_frame_requirements(
    project: &Project,
    frame: u32,
) -> Result<FrameRequirementsPayload, String> {
    if frame >= project.duration_frames {
        return Err(format!(
            "frame {frame} is out of range for duration {}",
            project.duration_frames
        ));
    }

    let mut images = HashSet::new();
    let mut videos: HashMap<String, HashSet<u64>> = HashMap::new();

    for layer in &project.layers {
        collect_layer_requirements(layer, frame, project.frame_rate, &mut images, &mut videos);
    }

    let mut images: Vec<String> = images.into_iter().collect();
    images.sort();

    let mut videos: Vec<VideoRequirementsPayload> = videos
        .into_iter()
        .map(|(source_id, frames)| {
            let mut frames: Vec<u64> = frames.into_iter().collect();
            frames.sort();
            VideoRequirementsPayload { source_id, frames }
        })
        .collect();
    videos.sort_by(|left, right| left.source_id.cmp(&right.source_id));

    Ok(FrameRequirementsPayload { images, videos })
}

fn collect_layer_requirements(
    layer: &Layer,
    frame: u32,
    fps: Rational,
    images: &mut HashSet<String>,
    videos: &mut HashMap<String, HashSet<u64>>,
) {
    for clip in &layer.clips {
        collect_clip_requirements(clip, frame, fps, images, videos);
    }
}

fn collect_clip_requirements(
    clip: &ClipType,
    frame: u32,
    fps: Rational,
    images: &mut HashSet<String>,
    videos: &mut HashMap<String, HashSet<u64>>,
) {
    if !clip.contains_frame(frame) {
        return;
    }

    match clip {
        ClipType::Group(group) => {
            for child in &group.children {
                collect_clip_requirements(child, frame, fps, images, videos);
            }
        }
        ClipType::Layout(layout) => {
            for node in &layout.children {
                collect_layout_node_requirements(node, frame, fps, images, videos);
            }
        }
        ClipType::Image(image) => {
            images.insert(image.source.clone());
        }
        ClipType::Video(video) => {
            if let Some(source_frame) = video.map_to_source_frame(frame, fps, None) {
                videos
                    .entry(video.source.clone())
                    .or_default()
                    .insert(source_frame as u64);
            }
        }
        ClipType::Shape(_) | ClipType::Text(_) => {}
    }
}

fn collect_layout_node_requirements(
    node: &LayoutNode,
    frame: u32,
    fps: Rational,
    images: &mut HashSet<String>,
    videos: &mut HashMap<String, HashSet<u64>>,
) {
    if let Some(content) = &node.content {
        match content {
            LayoutContent::Shape(_) | LayoutContent::Text(_) => {}
            LayoutContent::Image(image) => {
                if image.contains_frame(frame) {
                    images.insert(image.source.clone());
                }
            }
            LayoutContent::Video(video) => {
                if video.contains_frame(frame) {
                    if let Some(source_frame) = video.map_to_source_frame(frame, fps, None) {
                        videos
                            .entry(video.source.clone())
                            .or_default()
                            .insert(source_frame as u64);
                    }
                }
            }
            LayoutContent::Layout(layout) => {
                if layout.contains_frame(frame) {
                    for child in &layout.children {
                        collect_layout_node_requirements(child, frame, fps, images, videos);
                    }
                }
            }
        }
    }

    for child in &node.children {
        collect_layout_node_requirements(child, frame, fps, images, videos);
    }
}

unsafe fn read_bytes(ptr: *const u8, len: usize) -> Result<Vec<u8>, String> {
    if ptr.is_null() && len > 0 {
        return Err("null pointer".to_string());
    }
    let slice = if len == 0 {
        &[]
    } else {
        unsafe { slice::from_raw_parts(ptr, len) }
    };
    Ok(slice.to_vec())
}

unsafe fn read_string(ptr: *const u8, len: usize) -> Result<String, String> {
    let bytes = unsafe { read_bytes(ptr, len)? };
    String::from_utf8(bytes).map_err(|_| "invalid utf-8 string".to_string())
}

fn validate_rgba_shape(width: u32, height: u32, len: usize) -> bool {
    match pixel_len(width, height) {
        Ok(expected) => expected == len,
        Err(_) => false,
    }
}

unsafe fn adopt_owned_bytes(ptr: *mut u8, len: usize) -> Result<Vec<u8>, String> {
    if ptr.is_null() && len > 0 {
        return Err("null pointer".to_string());
    }
    if len == 0 {
        return Ok(Vec::new());
    }
    Ok(unsafe { Vec::from_raw_parts(ptr, len, len) })
}

#[unsafe(no_mangle)]
pub extern "C" fn lumen_renderer_create(
    project_ptr: *const u8,
    project_len: usize,
    _scale: f32,
) -> *mut WasmRenderer {
    ensure_contract_metadata();
    let project_bytes = unsafe { read_bytes(project_ptr, project_len) };
    let project_bytes = match project_bytes {
        Ok(bytes) => bytes,
        Err(error) => {
            set_status_payload("error", "invalid_input", error);
            return std::ptr::null_mut();
        }
    };

    let input_payload = match str::from_utf8(project_bytes.as_slice()) {
        Ok(value) => value.to_string(),
        Err(error) => {
            set_status_payload(
                "error",
                "invalid_input",
                format!("project payload must be UTF-8 JSON: {error}"),
            );
            return std::ptr::null_mut();
        }
    };

    let delegate_result = convert_json_delegate(&JsonDelegateRequest {
        input_payload,
        input_schema_revision: SCHEMA_REVISION.to_string(),
        caller_context: "lumen-wasm".to_string(),
    });

    let bundle = match delegate_result.status {
        JsonDelegateStatus::Success => match delegate_result.project_bundle {
            Some(bundle) => bundle,
            None => {
                set_status_payload(
                    "error",
                    "internal_error",
                    "delegate returned success without project bundle",
                );
                return std::ptr::null_mut();
            }
        },
        JsonDelegateStatus::CapabilityDisabled => {
            let detail = delegate_result
                .errors
                .first()
                .map(|issue| issue.message.as_str())
                .unwrap_or("json delegate capability is disabled in this build");
            set_status_payload("error", "capability_disabled", detail);
            return std::ptr::null_mut();
        }
        JsonDelegateStatus::ValidationError => {
            let detail = delegate_result
                .errors
                .first()
                .map(|issue| issue.message.as_str())
                .unwrap_or("project payload failed validation");
            set_status_payload("error", "invalid_input", detail);
            return std::ptr::null_mut();
        }
        JsonDelegateStatus::ConversionError => {
            let detail = delegate_result
                .errors
                .first()
                .map(|issue| issue.message.as_str())
                .unwrap_or("project conversion failed");
            set_status_payload("error", "contract_mismatch", detail);
            return std::ptr::null_mut();
        }
    };

    let mut renderer_ctx = match RendererContext::new(
        bundle.project.width,
        bundle.project.height,
        bundle.project.frame_rate,
    ) {
        Ok(context) => context,
        Err(error) => {
            set_status_payload(
                "error",
                "internal_error",
                format!("renderer initialization failed: {error}"),
            );
            return std::ptr::null_mut();
        }
    };
    renderer_ctx.clear_color = ((u32::from(bundle.background[3]) << 24)
        | (u32::from(bundle.background[0]) << 16)
        | (u32::from(bundle.background[1]) << 8)
        | u32::from(bundle.background[2]))
    .into();

    let instance = WasmRenderer {
        project: Arc::new(bundle.project),
        renderer_ctx,
        last_frame: Vec::new(),
        last_requirements: Vec::new(),
        last_error: None,
    };
    set_status_payload("ok", "ok", "renderer created");

    Box::into_raw(Box::new(instance))
}

#[unsafe(no_mangle)]
pub extern "C" fn lumen_renderer_destroy(renderer_ptr: *mut WasmRenderer) {
    if renderer_ptr.is_null() {
        return;
    }
    unsafe {
        drop(Box::from_raw(renderer_ptr));
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn lumen_renderer_width(renderer_ptr: *mut WasmRenderer) -> u32 {
    let renderer = unsafe { renderer_ptr.as_ref() };
    renderer.map(|value| value.project.width).unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "C" fn lumen_renderer_height(renderer_ptr: *mut WasmRenderer) -> u32 {
    let renderer = unsafe { renderer_ptr.as_ref() };
    renderer.map(|value| value.project.height).unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "C" fn lumen_renderer_last_frame_len(renderer_ptr: *mut WasmRenderer) -> usize {
    let renderer = unsafe { renderer_ptr.as_ref() };
    renderer.map(|value| value.last_frame.len()).unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "C" fn lumen_renderer_render_frame(
    renderer_ptr: *mut WasmRenderer,
    frame: u64,
    media_ptr: *mut WasmMediaStore,
) -> *const u8 {
    let renderer = match unsafe { renderer_ptr.as_mut() } {
        Some(renderer) => renderer,
        None => {
            set_status_payload("error", "invalid_input", "renderer pointer is null");
            return std::ptr::null();
        }
    };
    let media = match unsafe { media_ptr.as_ref() } {
        Some(media) => media,
        None => {
            renderer.set_error("missing media store");
            set_status_payload("error", "invalid_input", "missing media store");
            return std::ptr::null();
        }
    };

    let frame = match u32::try_from(frame) {
        Ok(frame) => frame,
        Err(_) => {
            renderer.set_error("frame index exceeds u32 range");
            set_status_payload("error", "invalid_input", "frame index exceeds u32 range");
            return std::ptr::null();
        }
    };

    if frame >= renderer.project.duration_frames {
        renderer.set_error(format!(
            "frame {frame} is out of range for duration {}",
            renderer.project.duration_frames
        ));
        set_status_payload("error", "invalid_input", "frame is out of range");
        return std::ptr::null();
    }

    renderer
        .renderer_ctx
        .set_media_store(Box::new(media.clone()));

    match render_scene(renderer.project.as_ref(), frame, &mut renderer.renderer_ctx) {
        Ok(pixels) => {
            renderer.clear_error();
            renderer.last_frame = pixels;
            set_status_payload("ok", "ok", "frame rendered");
            renderer.last_frame.as_ptr()
        }
        Err(err) => {
            renderer.set_error(err.to_string());
            set_status_payload("error", "internal_error", format!("render failed: {err}"));
            std::ptr::null()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn lumen_renderer_frame_requirements(
    renderer_ptr: *mut WasmRenderer,
    frame: u64,
) -> *const u8 {
    let renderer = match unsafe { renderer_ptr.as_mut() } {
        Some(renderer) => renderer,
        None => {
            set_status_payload("error", "invalid_input", "renderer pointer is null");
            return std::ptr::null();
        }
    };

    let frame = match u32::try_from(frame) {
        Ok(frame) => frame,
        Err(_) => {
            renderer.set_error("frame index exceeds u32 range");
            set_status_payload("error", "invalid_input", "frame index exceeds u32 range");
            return std::ptr::null();
        }
    };

    let payload = match collect_frame_requirements(renderer.project.as_ref(), frame) {
        Ok(payload) => payload,
        Err(err) => {
            renderer.set_error(err.clone());
            set_status_payload(
                "error",
                "invalid_input",
                format!("frame requirements failed: {err}"),
            );
            return std::ptr::null();
        }
    };

    match serde_json::to_vec(&payload) {
        Ok(data) => {
            renderer.clear_error();
            renderer.last_requirements = data;
            set_status_payload("ok", "ok", "frame requirements ready");
            renderer.last_requirements.as_ptr()
        }
        Err(_) => {
            renderer.set_error("failed to serialize frame requirements");
            set_status_payload(
                "error",
                "internal_error",
                "failed to serialize frame requirements",
            );
            std::ptr::null()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn lumen_renderer_frame_requirements_len(renderer_ptr: *mut WasmRenderer) -> usize {
    let renderer = unsafe { renderer_ptr.as_ref() };
    renderer
        .map(|value| value.last_requirements.len())
        .unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "C" fn lumen_renderer_last_error_ptr(renderer_ptr: *mut WasmRenderer) -> *const u8 {
    let renderer = unsafe { renderer_ptr.as_ref() };
    let Some(renderer) = renderer else {
        return std::ptr::null();
    };
    renderer
        .last_error
        .as_ref()
        .map(|err| err.as_ptr())
        .unwrap_or(std::ptr::null())
}

#[unsafe(no_mangle)]
pub extern "C" fn lumen_renderer_last_error_len(renderer_ptr: *mut WasmRenderer) -> usize {
    let renderer = unsafe { renderer_ptr.as_ref() };
    let Some(renderer) = renderer else {
        return 0;
    };
    renderer
        .last_error
        .as_ref()
        .map(|err| err.len())
        .unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "C" fn lumen_media_create() -> *mut WasmMediaStore {
    Box::into_raw(Box::new(WasmMediaStore::default()))
}

#[unsafe(no_mangle)]
pub extern "C" fn lumen_media_destroy(media_ptr: *mut WasmMediaStore) {
    if media_ptr.is_null() {
        return;
    }
    unsafe {
        drop(Box::from_raw(media_ptr));
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn lumen_media_clear(media_ptr: *mut WasmMediaStore) {
    let Some(media) = (unsafe { media_ptr.as_mut() }) else {
        return;
    };
    media.clear();
}

#[unsafe(no_mangle)]
pub extern "C" fn lumen_media_clear_videos(media_ptr: *mut WasmMediaStore) {
    let Some(media) = (unsafe { media_ptr.as_mut() }) else {
        return;
    };
    media.videos.clear();
}

#[unsafe(no_mangle)]
pub extern "C" fn lumen_media_has_image(
    media_ptr: *mut WasmMediaStore,
    source_ptr: *const u8,
    source_len: usize,
) -> u8 {
    let media = match unsafe { media_ptr.as_ref() } {
        Some(media) => media,
        None => return 0,
    };
    let source_id = match unsafe { read_string(source_ptr, source_len) } {
        Ok(value) => value,
        Err(_) => return 0,
    };
    if media.images.contains_key(&source_id) {
        1
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn lumen_media_set_image(
    media_ptr: *mut WasmMediaStore,
    source_ptr: *const u8,
    source_len: usize,
    width: u32,
    height: u32,
    rgba_ptr: *const u8,
    rgba_len: usize,
) -> u8 {
    let media = match unsafe { media_ptr.as_mut() } {
        Some(media) => media,
        None => return 0,
    };
    let source_id = match unsafe { read_string(source_ptr, source_len) } {
        Ok(value) => value,
        Err(_) => return 0,
    };
    let rgba = match unsafe { read_bytes(rgba_ptr, rgba_len) } {
        Ok(value) => value,
        Err(_) => return 0,
    };
    if !validate_rgba_shape(width, height, rgba.len()) {
        return 0;
    }

    media.images.insert(
        source_id,
        FrameData {
            width,
            height,
            pixels_rgba: Arc::new(rgba),
        },
    );
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn lumen_media_set_video_frame(
    media_ptr: *mut WasmMediaStore,
    source_ptr: *const u8,
    source_len: usize,
    source_frame: u64,
    width: u32,
    height: u32,
    rgba_ptr: *const u8,
    rgba_len: usize,
) -> u8 {
    let media = match unsafe { media_ptr.as_mut() } {
        Some(media) => media,
        None => return 0,
    };
    let source_id = match unsafe { read_string(source_ptr, source_len) } {
        Ok(value) => value,
        Err(_) => return 0,
    };
    let rgba = match unsafe { read_bytes(rgba_ptr, rgba_len) } {
        Ok(value) => value,
        Err(_) => return 0,
    };
    if !validate_rgba_shape(width, height, rgba.len()) {
        return 0;
    }

    let source = media
        .videos
        .entry(source_id)
        .or_insert_with(|| VideoSourceFrames {
            width,
            height,
            frames: HashMap::new(),
        });
    if source.width != width || source.height != height {
        return 0;
    }

    source.frames.insert(
        source_frame,
        FrameData {
            width,
            height,
            pixels_rgba: Arc::new(rgba),
        },
    );
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn lumen_media_set_image_owned(
    media_ptr: *mut WasmMediaStore,
    source_ptr: *const u8,
    source_len: usize,
    width: u32,
    height: u32,
    rgba_ptr: *mut u8,
    rgba_len: usize,
) -> u8 {
    let rgba = match unsafe { adopt_owned_bytes(rgba_ptr, rgba_len) } {
        Ok(value) => value,
        Err(_) => return 0,
    };

    let media = match unsafe { media_ptr.as_mut() } {
        Some(media) => media,
        None => return 0,
    };
    let source_id = match unsafe { read_string(source_ptr, source_len) } {
        Ok(value) => value,
        Err(_) => return 0,
    };
    if !validate_rgba_shape(width, height, rgba.len()) {
        return 0;
    }

    media.images.insert(
        source_id,
        FrameData {
            width,
            height,
            pixels_rgba: Arc::new(rgba),
        },
    );
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn lumen_media_set_video_frame_owned(
    media_ptr: *mut WasmMediaStore,
    source_ptr: *const u8,
    source_len: usize,
    source_frame: u64,
    width: u32,
    height: u32,
    rgba_ptr: *mut u8,
    rgba_len: usize,
) -> u8 {
    let rgba = match unsafe { adopt_owned_bytes(rgba_ptr, rgba_len) } {
        Ok(value) => value,
        Err(_) => return 0,
    };

    let media = match unsafe { media_ptr.as_mut() } {
        Some(media) => media,
        None => return 0,
    };
    let source_id = match unsafe { read_string(source_ptr, source_len) } {
        Ok(value) => value,
        Err(_) => return 0,
    };
    if !validate_rgba_shape(width, height, rgba.len()) {
        return 0;
    }

    let source = media
        .videos
        .entry(source_id)
        .or_insert_with(|| VideoSourceFrames {
            width,
            height,
            frames: HashMap::new(),
        });
    if source.width != width || source.height != height {
        return 0;
    }

    source.frames.insert(
        source_frame,
        FrameData {
            width,
            height,
            pixels_rgba: Arc::new(rgba),
        },
    );
    1
}

#[cfg(test)]
mod tests {
    use super::collect_frame_requirements;
    use lumen::{
        Layer, Project, Rational,
        clip::{
            ClipMeta, ClipType,
            media::{ImageClip, ImageFit, LoopMode, VideoClip},
            style::{BaseStyle, StyleProperty, TransformStyle},
        },
        scene::BlendMode,
    };

    fn base_style() -> BaseStyle {
        BaseStyle {
            visible: Default::default(),
            opacity: Default::default(),
            blend_mode: Default::default(),
            blur: Default::default(),
            shadows: Vec::new(),
            clip_radius: [
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
            ],
            transform: TransformStyle {
                translate: [Default::default(), Default::default()],
                scale: [Default::default(), Default::default()],
                rotation: Default::default(),
                skew: [Default::default(), Default::default()],
                origin: [Default::default(), Default::default()],
            },
            alignment: [Default::default(), Default::default()],
            mask: None,
        }
    }

    fn scene_with_image_and_video() -> Project {
        let image = ClipType::Image(ImageClip::new(
            ClipMeta {
                id: Some("image-clip".to_string()),
                start_frame: 0,
                end_frame: 10,
            },
            "image_0",
            ImageFit::Contain,
            base_style(),
        ));
        let video = ClipType::Video(
            VideoClip::new(
                ClipMeta {
                    id: Some("video-clip".to_string()),
                    start_frame: 0,
                    end_frame: 10,
                },
                "video_0",
                ImageFit::Contain,
                base_style(),
            )
            .with_trim(Some(1.0..3.0))
            .with_speed(1.0)
            .with_loop_mode(LoopMode::Repeat),
        );

        Project {
            width: 320,
            height: 180,
            frame_rate: Rational::new(30, 1),
            duration_frames: 12,
            layers: vec![Layer {
                id: "layer_0".to_string(),
                clips: vec![image, video],
                blend_mode: BlendMode::Normal,
                opacity: StyleProperty::Value(Default::default()),
                visible: true,
            }],
        }
    }

    #[test]
    fn frame_requirements_include_expected_sources() {
        let scene = scene_with_image_and_video();
        let requirements = collect_frame_requirements(&scene, 1).expect("requirements");

        assert_eq!(requirements.images, vec!["image_0".to_string()]);
        assert_eq!(requirements.videos.len(), 1);
        assert_eq!(requirements.videos[0].source_id, "video_0");
        assert_eq!(requirements.videos[0].frames, vec![31]);
    }

    #[test]
    fn frame_requirements_reject_out_of_range_frame() {
        let scene = scene_with_image_and_video();
        let err = collect_frame_requirements(&scene, 99).expect_err("out of range");
        assert!(err.contains("out of range"));
    }
}
