use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{Arc, RwLock},
};

use lumen::{
    error::MediaError,
    media::{
        CpuMediaFrame, ImageMetadata, ImageResolver, MediaFrame, MediaStore, VideoFrameResolver,
        VideoMetadata,
    },
};
use wasm_bindgen::prelude::*;

use crate::utils::{image_frame_from_rgba, validate_rgba_len};

const DEFAULT_VIDEO_FRAME_CACHE_CAPACITY: usize = 96;

// ── VideoFrameCache ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub(crate) struct VideoFrameCache {
    capacity: usize,
    entries: HashMap<u32, Arc<CpuMediaFrame>>,
    order: VecDeque<u32>,
    pinned: HashSet<u32>,
}

impl VideoFrameCache {
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_VIDEO_FRAME_CACHE_CAPACITY)
    }

    fn with_capacity(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            entries: HashMap::new(),
            order: VecDeque::new(),
            pinned: HashSet::new(),
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
        self.pinned.clear();
    }

    pub fn contains(&self, frame: u32) -> bool {
        self.entries.contains_key(&frame)
    }

    pub fn get(&self, frame: u32) -> Option<Arc<CpuMediaFrame>> {
        self.entries.get(&frame).cloned()
    }

    pub fn insert(&mut self, frame: u32, image: Arc<CpuMediaFrame>) {
        self.entries.insert(frame, image);
        self.touch(frame);
        self.evict_over_capacity();
    }

    pub fn retain(&mut self, frames: &[u32]) {
        self.pinned = frames.iter().copied().collect();
        self.evict_over_capacity();
    }

    fn touch(&mut self, frame: u32) {
        if let Some(index) = self.order.iter().position(|existing| *existing == frame) {
            self.order.remove(index);
        }
        self.order.push_back(frame);
    }

    fn evict_over_capacity(&mut self) {
        let target_capacity = self.capacity.max(self.pinned.len());
        let mut attempts_remaining = self.order.len().saturating_add(self.entries.len());

        while self.entries.len() > target_capacity && attempts_remaining > 0 {
            attempts_remaining -= 1;
            let Some(candidate) = self.order.pop_front() else {
                break;
            };

            if !self.entries.contains_key(&candidate) {
                continue;
            }

            if self.pinned.contains(&candidate) {
                self.order.push_back(candidate);
                continue;
            }

            self.entries.remove(&candidate);
        }
    }
}

// ── Stored entry types ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct StoredImage {
    metadata: ImageMetadata,
    frame: Arc<CpuMediaFrame>,
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
            frames: VideoFrameCache::new(),
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

    pub fn set_image(&self, source: String, frame: CpuMediaFrame) -> Result<(), &'static str> {
        let metadata = ImageMetadata {
            width: frame.width,
            height: frame.height,
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
        image: CpuMediaFrame,
    ) -> Result<(), &'static str> {
        let mut videos = self
            .inner
            .videos
            .write()
            .map_err(|_| "media store lock poisoned")?;
        let entry = videos.entry(stream_id).or_default();
        entry.metadata.width = image.width;
        entry.metadata.height = image.height;
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

    fn frame(&self) -> Result<MediaFrame, MediaError> {
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
            .map(MediaFrame::CpuRgba)
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

    fn enqueue_frame(&self, _frame: u32) -> Result<(), MediaError> {
        Ok(())
    }

    fn frame(&self, frame: u32) -> Result<MediaFrame, MediaError> {
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
        entry
            .frames
            .get(frame)
            .ok_or(MediaError::FrameOutOfRange {
                media_source: self.id.clone(),
                frame,
                frame_count: entry.metadata.frame_count,
            })
            .map(MediaFrame::CpuRgba)
    }

    fn retain_frames(&self, frames: &[u32]) {
        let Ok(mut videos) = self.store.videos.write() else {
            return;
        };
        if let Some(entry) = videos.get_mut(&self.id) {
            entry.frames.retain(frames);
        }
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

    #[wasm_bindgen(js_name = "setVideoMetadata")]
    pub fn set_video_metadata(
        &self,
        stream_id: &str,
        width: u32,
        height: u32,
        frame_count: u32,
        fps: f32,
    ) -> Result<(), JsValue> {
        if width == 0 || height == 0 {
            return Err(JsValue::from_str("video dimensions must be > 0"));
        }
        if !fps.is_finite() || fps <= 0.0 {
            return Err(JsValue::from_str("video fps must be a finite value > 0"));
        }
        self.store
            .set_video_metadata(
                stream_id.to_string(),
                VideoMetadata {
                    width,
                    height,
                    frame_count,
                    fps,
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
    fn video_frame_cache_keeps_pinned_frames_and_recent_entries() {
        let mut cache = VideoFrameCache::with_capacity(3);
        for frame in 0..3 {
            cache.insert(frame, Arc::new(test_frame()));
        }

        cache.retain(&[0]);
        cache.insert(3, Arc::new(test_frame()));
        cache.insert(4, Arc::new(test_frame()));

        assert!(cache.contains(0), "pinned frame should remain cached");
        assert!(cache.contains(3), "recent frame should remain cached");
        assert!(cache.contains(4), "most recent frame should remain cached");
        assert_eq!(cache.entries.len(), 3);
    }

    #[test]
    fn video_frame_cache_evicts_old_unpinned_frames() {
        let mut cache = VideoFrameCache::with_capacity(3);
        for frame in 0..5 {
            cache.insert(frame, Arc::new(test_frame()));
        }

        assert!(!cache.contains(0));
        assert!(!cache.contains(1));
        assert!(cache.contains(2));
        assert!(cache.contains(3));
        assert!(cache.contains(4));
        assert_eq!(cache.entries.len(), 3);
    }

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
                    fps: 30.0,
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
            resolver.frame(0),
            Err(MediaError::FrameOutOfRange { .. })
        ));
    }

    fn test_frame() -> CpuMediaFrame {
        crate::utils::image_frame_from_rgba(1, 1, vec![255, 0, 0, 255]).expect("frame")
    }
}
