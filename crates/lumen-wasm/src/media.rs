use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, RwLock},
};

use lumen::{
    error::MediaError,
    media::{ImageMetadata, ImageResolver, MediaStore, VideoFrameResolver, VideoMetadata},
    raster::ImageFrame,
};
use wasm_bindgen::prelude::*;

use crate::utils::{image_frame_from_rgba, validate_rgba_len};
use crate::webgl::image_frame_from_video_frame;

const DEFAULT_VIDEO_FRAME_CAPACITY: usize = 96;

// ── VideoFrameCache ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub(crate) struct VideoFrameCache {
    capacity: usize,
    order: VecDeque<u32>,
    entries: HashMap<u32, Arc<ImageFrame>>,
}

impl VideoFrameCache {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            order: VecDeque::new(),
            entries: HashMap::new(),
        }
    }

    pub fn clear(&mut self) {
        self.order.clear();
        self.entries.clear();
    }

    pub fn contains(&self, frame: u32) -> bool {
        self.entries.contains_key(&frame)
    }

    pub fn get(&self, frame: u32) -> Option<Arc<ImageFrame>> {
        self.entries.get(&frame).cloned()
    }

    pub fn insert(&mut self, frame: u32, image: Arc<ImageFrame>) {
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

// ── Stored entry types ────────────────────────────────────────────────────────

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

// ── WasmMediaStore ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub(crate) struct WasmMediaStore {
    inner: Arc<WasmMediaStoreInner>,
}

#[derive(Debug, Default)]
struct WasmMediaStoreInner {
    images: RwLock<HashMap<String, StoredImage>>,
    videos: RwLock<HashMap<String, StoredVideo>>,
}

impl WasmMediaStore {
    pub fn clear(&self) -> Result<(), &'static str> {
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

    pub fn clear_video_frames(&self) -> Result<(), &'static str> {
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

    pub fn clear_video_frames_for_stream(&self, stream_id: &str) -> Result<(), &'static str> {
        let mut videos = self
            .inner
            .videos
            .write()
            .map_err(|_| "media store lock poisoned")?;
        if let Some(video) = videos.get_mut(stream_id) {
            video.frames.clear();
        }
        Ok(())
    }

    pub fn remove_image(&self, image_id: &str) -> Result<(), &'static str> {
        self.inner
            .images
            .write()
            .map_err(|_| "media store lock poisoned")?
            .remove(image_id);
        Ok(())
    }

    pub fn remove_video(&self, stream_id: &str) -> Result<(), &'static str> {
        self.inner
            .videos
            .write()
            .map_err(|_| "media store lock poisoned")?
            .remove(stream_id);
        Ok(())
    }

    pub fn has_image(&self, image_id: &str) -> bool {
        self.inner
            .images
            .read()
            .ok()
            .is_some_and(|images| images.contains_key(image_id))
    }

    pub fn has_video_frame(&self, stream_id: &str, frame: u32) -> bool {
        let Ok(videos) = self.inner.videos.read() else {
            return false;
        };
        videos
            .get(stream_id)
            .is_some_and(|video| video.frames.contains(frame))
    }

    pub fn set_image(&self, source: String, frame: ImageFrame) -> Result<(), &'static str> {
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

    pub fn set_video_metadata(
        &self,
        stream_id: String,
        metadata: VideoMetadata,
    ) -> Result<(), &'static str> {
        let mut videos = self
            .inner
            .videos
            .write()
            .map_err(|_| "media store lock poisoned")?;
        let entry = videos.entry(stream_id).or_default();
        entry.metadata = metadata;
        Ok(())
    }

    pub fn set_video_frame(
        &self,
        stream_id: String,
        frame: u32,
        image: ImageFrame,
    ) -> Result<(), &'static str> {
        let mut videos = self
            .inner
            .videos
            .write()
            .map_err(|_| "media store lock poisoned")?;
        let entry = videos.entry(stream_id).or_default();
        entry.metadata.width = image.storage_width;
        entry.metadata.height = image.storage_height;
        entry.metadata.frame_count = entry.metadata.frame_count.max(frame.saturating_add(1));
        entry.frames.insert(frame, Arc::new(image));
        Ok(())
    }
}

impl MediaStore for WasmMediaStore {
    fn get_image_resolver(&self, image_id: &str) -> Option<Box<dyn ImageResolver>> {
        Some(Box::new(WasmImageResolver {
            id: image_id.to_string(),
            store: Arc::clone(&self.inner),
        }))
    }

    fn get_video_resolver(&self, stream_id: &str) -> Option<Box<dyn VideoFrameResolver>> {
        Some(Box::new(WasmVideoResolver {
            id: stream_id.to_string(),
            store: Arc::clone(&self.inner),
        }))
    }
}

// ── Resolvers ─────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct WasmImageResolver {
    id: String,
    store: Arc<WasmMediaStoreInner>,
}

impl ImageResolver for WasmImageResolver {
    fn id(&self) -> &str {
        &self.id
    }

    fn metadata(&self) -> ImageMetadata {
        self.store
            .images
            .read()
            .ok()
            .and_then(|images| images.get(&self.id).map(|entry| entry.metadata))
            .unwrap_or_default()
    }

    fn resolve_image(&self) -> Result<Arc<ImageFrame>, MediaError> {
        self.store
            .images
            .read()
            .map_err(|_| MediaError::SourceNotFound {
                media_source: self.id.clone(),
            })?
            .get(&self.id)
            .map(|entry| Arc::clone(&entry.frame))
            .ok_or_else(|| MediaError::SourceNotFound {
                media_source: self.id.clone(),
            })
    }
}

#[derive(Clone)]
struct WasmVideoResolver {
    id: String,
    store: Arc<WasmMediaStoreInner>,
}

impl VideoFrameResolver for WasmVideoResolver {
    fn id(&self) -> &str {
        &self.id
    }

    fn metadata(&self) -> VideoMetadata {
        self.store
            .videos
            .read()
            .ok()
            .and_then(|videos| videos.get(&self.id).map(|entry| entry.metadata))
            .unwrap_or_default()
    }

    fn resolve_frame_image(&self, frame: u32) -> Result<Arc<ImageFrame>, MediaError> {
        let videos = self
            .store
            .videos
            .read()
            .map_err(|_| MediaError::SourceNotFound {
                media_source: self.id.clone(),
            })?;
        let entry = videos
            .get(&self.id)
            .ok_or_else(|| MediaError::SourceNotFound {
                media_source: self.id.clone(),
            })?;
        entry.frames.get(frame).ok_or(MediaError::FrameOutOfRange {
            media_source: self.id.clone(),
            frame,
            frame_count: entry.metadata.frame_count,
        })
    }
}

// ── LumenMediaStore (wasm-bindgen public API) ─────────────────────────────────

#[wasm_bindgen]
pub struct LumenMediaStore {
    store: WasmMediaStore,
}

#[wasm_bindgen]
impl LumenMediaStore {
    #[wasm_bindgen(constructor)]
    pub fn new() -> LumenMediaStore {
        LumenMediaStore {
            store: WasmMediaStore::default(),
        }
    }

    pub fn clear(&self) -> Result<(), JsValue> {
        self.store.clear().map_err(JsValue::from_str)
    }

    #[wasm_bindgen(js_name = "clearVideos")]
    pub fn clear_videos(&self) -> Result<(), JsValue> {
        self.store.clear_video_frames().map_err(JsValue::from_str)
    }

    #[wasm_bindgen(js_name = "clearVideoSource")]
    pub fn clear_video_source(&self, stream_id: &str) -> Result<(), JsValue> {
        self.store
            .clear_video_frames_for_stream(stream_id)
            .map_err(JsValue::from_str)
    }

    #[wasm_bindgen(js_name = "removeImageSource")]
    pub fn remove_image_source(&self, image_id: &str) -> Result<(), JsValue> {
        self.store.remove_image(image_id).map_err(JsValue::from_str)
    }

    #[wasm_bindgen(js_name = "removeVideoSource")]
    pub fn remove_video_source(&self, stream_id: &str) -> Result<(), JsValue> {
        self.store
            .remove_video(stream_id)
            .map_err(JsValue::from_str)
    }

    #[wasm_bindgen(js_name = "hasImage")]
    pub fn has_image(&self, image_id: &str) -> bool {
        self.store.has_image(image_id)
    }

    #[wasm_bindgen(js_name = "hasVideoFrame")]
    pub fn has_video_frame(&self, stream_id: &str, frame: u32) -> bool {
        self.store.has_video_frame(stream_id, frame)
    }

    #[wasm_bindgen(js_name = "setImage")]
    pub fn set_image(
        &self,
        image_id: &str,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) -> Result<(), JsValue> {
        if width == 0 || height == 0 {
            return Err(JsValue::from_str("image dimensions must be > 0"));
        }
        if !validate_rgba_len(width, height, rgba.len()) {
            return Err(JsValue::from_str("invalid image rgba buffer length"));
        }
        let frame = image_frame_from_rgba(width, height, rgba.to_vec())
            .map_err(|error| JsValue::from_str(&error))?;
        self.store
            .set_image(image_id.to_string(), frame)
            .map_err(JsValue::from_str)
    }

    #[wasm_bindgen(js_name = "setVideoFrame")]
    pub fn set_video_frame(
        &self,
        stream_id: &str,
        frame: u32,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) -> Result<(), JsValue> {
        if width == 0 || height == 0 {
            return Err(JsValue::from_str("video frame dimensions must be > 0"));
        }
        if !validate_rgba_len(width, height, rgba.len()) {
            return Err(JsValue::from_str("invalid video rgba buffer length"));
        }
        let image = image_frame_from_rgba(width, height, rgba.to_vec())
            .map_err(|error| JsValue::from_str(&error))?;
        self.store
            .set_video_frame(stream_id.to_string(), frame, image)
            .map_err(JsValue::from_str)
    }

    #[wasm_bindgen(js_name = "setVideoFrameObject")]
    pub fn set_video_frame_object(
        &self,
        stream_id: &str,
        frame: u32,
        video_frame: &web_sys::VideoFrame,
        width: u32,
        height: u32,
    ) -> Result<(), JsValue> {
        if width == 0 || height == 0 {
            return Err(JsValue::from_str("video frame dimensions must be > 0"));
        }
        let image = image_frame_from_video_frame(video_frame, width, height)?;
        self.store
            .set_video_frame(stream_id.to_string(), frame, image)
            .map_err(JsValue::from_str)
    }

    #[wasm_bindgen(js_name = "setVideoMetadata")]
    pub fn set_video_metadata(
        &self,
        stream_id: &str,
        width: u32,
        height: u32,
        frame_count: u32,
    ) -> Result<(), JsValue> {
        if width == 0 || height == 0 {
            return Err(JsValue::from_str("video dimensions must be > 0"));
        }
        self.store
            .set_video_metadata(
                stream_id.to_string(),
                VideoMetadata {
                    width,
                    height,
                    frame_count,
                },
            )
            .map_err(JsValue::from_str)
    }
}

impl LumenMediaStore {
    pub(crate) fn as_wasm_store(&self) -> &WasmMediaStore {
        &self.store
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
        let frame = crate::utils::image_frame_from_rgba(1, 1, vec![255, 0, 0, 255]).expect("frame");
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
            Err(MediaError::FrameOutOfRange { .. })
        ));
    }
}
