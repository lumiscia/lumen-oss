use std::{
    collections::{HashMap, HashSet},
    slice, str,
    sync::Arc,
};

use lumen::{
    backend::{FrameImage, FrameProvider, ProviderError, Renderer},
    compile::{
        CompiledLayoutNode, CompiledLayoutNodeKind, CompiledOperationKind, CompiledTimeline,
        compile_project_with_scale,
    },
    model::Project,
};
use serde::Serialize;

#[derive(Default)]
pub struct WasmMediaStore {
    images: HashMap<String, FrameImage>,
    videos: HashMap<String, HashMap<u64, FrameImage>>,
}

impl WasmMediaStore {
    fn clear(&mut self) {
        self.images.clear();
        self.videos.clear();
    }
}

impl FrameProvider for WasmMediaStore {
    fn image(&mut self, source_id: &str) -> Result<Option<FrameImage>, ProviderError> {
        Ok(self.images.get(source_id).cloned())
    }

    fn video_frame(
        &mut self,
        source_id: &str,
        source_frame: u64,
    ) -> Result<Option<FrameImage>, ProviderError> {
        Ok(self
            .videos
            .get(source_id)
            .and_then(|frames| frames.get(&source_frame).cloned()))
    }
}

pub struct WasmRenderer {
    timeline: Arc<CompiledTimeline>,
    renderer: lumen::backend::skia::SkiaRenderer,
    last_frame: Vec<u8>,
    last_requirements: Vec<u8>,
    last_error: Option<String>,
}

impl WasmRenderer {
    fn set_error(&mut self, error: impl ToString) {
        self.last_error = Some(error.to_string());
    }

    fn clear_error(&mut self) {
        self.last_error = None;
    }
}

#[derive(Serialize)]
struct FrameRequirementsPayload {
    images: Vec<String>,
    videos: Vec<VideoRequirementsPayload>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct VideoRequirementsPayload {
    source_id: String,
    frames: Vec<u64>,
}

fn collect_frame_requirements(
    timeline: &CompiledTimeline,
    frame: u64,
) -> Result<FrameRequirementsPayload, String> {
    let indices = timeline
        .operation_indices_for_frame(frame)
        .map_err(|err| err.to_string())?;

    let mut images = HashSet::new();
    let mut videos: HashMap<String, HashSet<u64>> = HashMap::new();

    for index in indices {
        let operation = timeline
            .operation(*index)
            .ok_or_else(|| format!("missing operation index {index}"))?;
        match &operation.kind {
            CompiledOperationKind::Image(image) => {
                if let Some(source) = timeline.source(image.source_index) {
                    images.insert(source.id.clone());
                }
            }
            CompiledOperationKind::Layout(layout) => {
                collect_layout_image_requirements(timeline, &layout.root, &mut images);
            }
            CompiledOperationKind::Video(video) => {
                if let Some(source) = timeline.source(video.source_index) {
                    let source_frame = operation
                        .source_frame_at(frame, u64::MAX / 4)
                        .unwrap_or(operation.local_frame(frame));
                    videos
                        .entry(source.id.clone())
                        .or_default()
                        .insert(source_frame);
                }
            }
            _ => {}
        }
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

fn collect_layout_image_requirements(
    timeline: &CompiledTimeline,
    node: &CompiledLayoutNode,
    images: &mut HashSet<String>,
) {
    match &node.kind {
        CompiledLayoutNodeKind::Container { children } => {
            for child in children {
                collect_layout_image_requirements(timeline, child, images);
            }
        }
        CompiledLayoutNodeKind::Text { .. } => {}
        CompiledLayoutNodeKind::Image { source_index } => {
            if let Some(source) = timeline.source(*source_index) {
                images.insert(source.id.clone());
            }
        }
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

#[unsafe(no_mangle)]
pub extern "C" fn lumen_renderer_create(
    project_ptr: *const u8,
    project_len: usize,
    scale: f32,
) -> *mut WasmRenderer {
    let project_bytes = unsafe { read_bytes(project_ptr, project_len) };
    let project_bytes = match project_bytes {
        Ok(bytes) => bytes,
        Err(_) => return std::ptr::null_mut(),
    };

    let project: Project = match serde_json::from_slice(&project_bytes) {
        Ok(project) => project,
        Err(_) => return std::ptr::null_mut(),
    };

    let timeline = match compile_project_with_scale(&project, scale) {
        Ok(timeline) => timeline,
        Err(_) => return std::ptr::null_mut(),
    };

    let renderer = match lumen::backend::skia::SkiaRenderer::new(
        timeline.canvas.width,
        timeline.canvas.height,
    ) {
        Ok(renderer) => renderer,
        Err(_) => return std::ptr::null_mut(),
    };

    let instance = WasmRenderer {
        timeline,
        renderer,
        last_frame: Vec::new(),
        last_requirements: Vec::new(),
        last_error: None,
    };

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
    renderer
        .map(|value| value.timeline.canvas.width)
        .unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "C" fn lumen_renderer_height(renderer_ptr: *mut WasmRenderer) -> u32 {
    let renderer = unsafe { renderer_ptr.as_ref() };
    renderer
        .map(|value| value.timeline.canvas.height)
        .unwrap_or(0)
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
        None => return std::ptr::null(),
    };
    let media = match unsafe { media_ptr.as_mut() } {
        Some(media) => media,
        None => {
            renderer.set_error("missing media store");
            return std::ptr::null();
        }
    };

    match renderer
        .renderer
        .render_frame(renderer.timeline.as_ref(), frame, media)
    {
        Ok(pixels) => {
            renderer.clear_error();
            renderer.last_frame = pixels;
            renderer.last_frame.as_ptr()
        }
        Err(err) => {
            renderer.set_error(err.to_string());
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
        None => return std::ptr::null(),
    };

    let payload = match collect_frame_requirements(renderer.timeline.as_ref(), frame) {
        Ok(payload) => payload,
        Err(err) => {
            renderer.set_error(err);
            return std::ptr::null();
        }
    };

    match serde_json::to_vec(&payload) {
        Ok(data) => {
            renderer.clear_error();
            renderer.last_requirements = data;
            renderer.last_requirements.as_ptr()
        }
        Err(_) => {
            renderer.set_error("failed to serialize frame requirements");
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
    let image = match FrameImage::new(width, height, rgba) {
        Ok(image) => image,
        Err(_) => return 0,
    };
    media.images.insert(source_id, image);
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
    let image = match FrameImage::new(width, height, rgba) {
        Ok(image) => image,
        Err(_) => return 0,
    };
    media
        .videos
        .entry(source_id)
        .or_default()
        .insert(source_frame, image);
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
    let media = match unsafe { media_ptr.as_mut() } {
        Some(media) => media,
        None => return 0,
    };
    let source_id = match unsafe { read_string(source_ptr, source_len) } {
        Ok(value) => value,
        Err(_) => return 0,
    };
    // SAFETY: In wasm32-unknown-emscripten, Rust and Emscripten share the same
    // dlmalloc allocator, so memory allocated via JS `_malloc` can be safely
    // adopted by a Vec and freed by Rust's global allocator.
    let rgba = if rgba_ptr.is_null() && rgba_len > 0 {
        return 0;
    } else if rgba_len == 0 {
        Vec::new()
    } else {
        unsafe { Vec::from_raw_parts(rgba_ptr, rgba_len, rgba_len) }
    };
    let image = match FrameImage::new(width, height, rgba) {
        Ok(image) => image,
        Err(_) => return 0,
    };
    media.images.insert(source_id, image);
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
    let media = match unsafe { media_ptr.as_mut() } {
        Some(media) => media,
        None => return 0,
    };
    let source_id = match unsafe { read_string(source_ptr, source_len) } {
        Ok(value) => value,
        Err(_) => return 0,
    };
    // SAFETY: Same as lumen_media_set_image_owned — shared dlmalloc allocator.
    let rgba = if rgba_ptr.is_null() && rgba_len > 0 {
        return 0;
    } else if rgba_len == 0 {
        Vec::new()
    } else {
        unsafe { Vec::from_raw_parts(rgba_ptr, rgba_len, rgba_len) }
    };
    let image = match FrameImage::new(width, height, rgba) {
        Ok(image) => image,
        Err(_) => return 0,
    };
    media
        .videos
        .entry(source_id)
        .or_default()
        .insert(source_frame, image);
    1
}
