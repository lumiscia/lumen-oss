use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, RwLock},
};

use lumen_engine::{
    error::MediaError,
    media::{
        CpuMediaFrame, FontResolver, ImageMetadata, ImageResolver, MediaFrame, MediaStore,
        VideoFrameRequirement, VideoFrameResolver, VideoMetadata,
    },
};
use wasm_bindgen::prelude::*;

use crate::utils::{image_frame_from_rgba, validate_rgba_len};

/// Total bytes of decoded video frames kept across all streams of one media store.
pub(crate) const DEFAULT_VIDEO_FRAME_CACHE_BYTES: usize = 512 * 1024 * 1024;

// ── VideoFrameCache ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct CachedFrame {
    image: Arc<CpuMediaFrame>,
    inserted: u64,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct VideoFrameCache {
    entries: HashMap<u32, CachedFrame>,
    pinned: HashSet<u32>,
    bytes: usize,
}

impl VideoFrameCache {
    pub fn clear(&mut self) {
        self.entries.clear();
        self.pinned.clear();
        self.bytes = 0;
    }

    pub fn contains(&self, frame: u32) -> bool {
        self.entries.contains_key(&frame)
    }

    pub fn get(&self, frame: u32) -> Option<Arc<CpuMediaFrame>> {
        self.entries
            .get(&frame)
            .map(|entry| Arc::clone(&entry.image))
    }

    fn insert(&mut self, frame: u32, image: Arc<CpuMediaFrame>, inserted: u64) {
        self.bytes += image.rgba.len();
        if let Some(previous) = self.entries.insert(frame, CachedFrame { image, inserted }) {
            self.bytes -= previous.image.rgba.len();
        }
    }

    fn remove(&mut self, frame: u32) {
        if let Some(previous) = self.entries.remove(&frame) {
            self.bytes -= previous.image.rgba.len();
        }
    }

    fn pin(&mut self, frames: &[u32]) {
        self.pinned = frames.iter().copied().collect();
    }

    /// The unpinned frame to evict first: frames behind the playhead (the earliest pinned frame)
    /// go before frames ahead of it, farthest first. Without a playhead the oldest insert goes.
    fn eviction_candidate(&self, keep: Option<u32>) -> Option<(EvictionRank, u32)> {
        let playhead = self.pinned.iter().min().copied();
        self.entries
            .iter()
            .filter(|(frame, _)| Some(**frame) != keep && !self.pinned.contains(frame))
            .map(|(frame, entry)| {
                let rank = match playhead {
                    None => (2, u64::MAX - entry.inserted),
                    Some(playhead) if *frame < playhead => (1, u64::from(playhead - frame)),
                    Some(playhead) => (0, u64::from(frame - playhead)),
                };
                (rank, *frame)
            })
            .max()
    }
}

type EvictionRank = (u8, u64);

#[derive(Debug)]
struct VideoStore {
    streams: HashMap<String, StoredVideo>,
    budget_bytes: usize,
    insert_clock: u64,
}

impl Default for VideoStore {
    fn default() -> Self {
        Self {
            streams: HashMap::new(),
            budget_bytes: DEFAULT_VIDEO_FRAME_CACHE_BYTES,
            insert_clock: 0,
        }
    }
}

impl VideoStore {
    fn insert_frame(&mut self, stream_id: String, frame: u32, image: CpuMediaFrame) {
        self.insert_clock += 1;
        let entry = self.streams.entry(stream_id.clone()).or_default();
        entry.metadata.width = image.width;
        entry.metadata.height = image.height;
        entry.metadata.frame_count = entry.metadata.frame_count.max(frame.saturating_add(1));
        entry
            .frames
            .insert(frame, Arc::new(image), self.insert_clock);
        self.evict_to_budget(Some((&stream_id, frame)));
    }

    /// Evicts unpinned frames until the cache fits its byte budget. Pinned frames and the
    /// `keep` frame are never evicted, so the budget can be exceeded by frames in use.
    fn evict_to_budget(&mut self, keep: Option<(&str, u32)>) {
        let mut total: usize = self.streams.values().map(|video| video.frames.bytes).sum();
        while total > self.budget_bytes {
            let victim = self
                .streams
                .iter()
                .filter_map(|(stream_id, video)| {
                    let keep = keep
                        .filter(|(keep_id, _)| keep_id == stream_id)
                        .map(|(_, frame)| frame);
                    video
                        .frames
                        .eviction_candidate(keep)
                        .map(|(rank, frame)| (rank, stream_id, frame))
                })
                .max()
                .map(|(_, stream_id, frame)| (stream_id.clone(), frame));
            let Some((stream_id, frame)) = victim else {
                break;
            };
            let Some(video) = self.streams.get_mut(&stream_id) else {
                break;
            };
            let before = video.frames.bytes;
            video.frames.remove(frame);
            total -= before - video.frames.bytes;
        }
    }
}

// ── Stored entry types ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct StoredImage {
    metadata: ImageMetadata,
    frame: Arc<CpuMediaFrame>,
}

#[derive(Debug, Clone, Default)]
struct StoredVideo {
    metadata: VideoMetadata,
    frames: VideoFrameCache,
}

#[derive(Debug, Clone)]
struct StoredFont {
    data: Arc<Vec<u8>>,
    version: u64,
}

// ── WasmMediaStore ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub(crate) struct WasmMediaStore {
    inner: Arc<WasmMediaStoreInner>,
}

#[derive(Debug, Default)]
struct WasmMediaStoreInner {
    fonts: RwLock<HashMap<String, StoredFont>>,
    images: RwLock<HashMap<String, StoredImage>>,
    videos: RwLock<VideoStore>,
}

impl WasmMediaStore {
    pub fn clear(&self) -> Result<(), &'static str> {
        self.inner
            .images
            .write()
            .map_err(|_| "media store lock poisoned")?
            .clear();
        self.inner
            .fonts
            .write()
            .map_err(|_| "media store lock poisoned")?
            .clear();
        self.inner
            .videos
            .write()
            .map_err(|_| "media store lock poisoned")?
            .streams
            .clear();
        Ok(())
    }

    pub fn clear_video_frames(&self) -> Result<(), &'static str> {
        let mut videos = self
            .inner
            .videos
            .write()
            .map_err(|_| "media store lock poisoned")?;
        for video in videos.streams.values_mut() {
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
        if let Some(video) = videos.streams.get_mut(stream_id) {
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
            .streams
            .remove(stream_id);
        Ok(())
    }

    pub fn remove_font(&self, font_family: &str) -> Result<(), &'static str> {
        self.inner
            .fonts
            .write()
            .map_err(|_| "media store lock poisoned")?
            .remove(font_family);
        Ok(())
    }

    pub fn has_image(&self, image_id: &str) -> bool {
        self.inner
            .images
            .read()
            .ok()
            .is_some_and(|images| images.contains_key(image_id))
    }

    pub fn has_font(&self, font_family: &str) -> bool {
        self.inner
            .fonts
            .read()
            .ok()
            .is_some_and(|fonts| fonts.contains_key(font_family))
    }

    pub fn has_video_frame(&self, stream_id: &str, frame: u32) -> bool {
        let Ok(videos) = self.inner.videos.read() else {
            return false;
        };
        videos
            .streams
            .get(stream_id)
            .is_some_and(|video| video.frames.contains(frame))
    }

    pub fn set_video_frame_cache_bytes(&self, bytes: usize) -> Result<(), &'static str> {
        let mut videos = self
            .inner
            .videos
            .write()
            .map_err(|_| "media store lock poisoned")?;
        videos.budget_bytes = bytes;
        videos.evict_to_budget(None);
        Ok(())
    }

    /// Protects the frames each stream needs at the playhead from eviction.
    pub fn pin_video_frames(
        &self,
        requirements: &[VideoFrameRequirement],
    ) -> Result<(), &'static str> {
        let mut videos = self
            .inner
            .videos
            .write()
            .map_err(|_| "media store lock poisoned")?;
        for (stream_id, video) in &mut videos.streams {
            let frames = requirements
                .iter()
                .find(|requirement| requirement.stream_id == *stream_id)
                .map_or(&[][..], |requirement| requirement.frames.as_slice());
            video.frames.pin(frames);
        }
        videos.evict_to_budget(None);
        Ok(())
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

    pub fn set_font(&self, font_family: String, data: Vec<u8>) -> Result<(), &'static str> {
        let font_family = font_family.trim();
        if font_family.is_empty() {
            return Err("font family must not be empty");
        }
        if data.is_empty() {
            return Err("font data must not be empty");
        }

        let mut fonts = self
            .inner
            .fonts
            .write()
            .map_err(|_| "media store lock poisoned")?;
        let version = fonts
            .get(font_family)
            .map(|font| font.version.saturating_add(1))
            .unwrap_or(1);
        fonts.insert(
            font_family.to_string(),
            StoredFont {
                data: Arc::new(data),
                version,
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
        let entry = videos.streams.entry(stream_id).or_default();
        entry.metadata = metadata;
        Ok(())
    }

    pub fn set_video_frame(
        &self,
        stream_id: String,
        frame: u32,
        image: CpuMediaFrame,
    ) -> Result<(), &'static str> {
        self.inner
            .videos
            .write()
            .map_err(|_| "media store lock poisoned")?
            .insert_frame(stream_id, frame, image);
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

    fn get_font_resolver(&self, font_family: &str) -> Option<Box<dyn FontResolver>> {
        let fonts = self.inner.fonts.read().ok()?;
        let font = fonts.get(font_family)?;
        Some(Box::new(WasmFontResolver {
            id: format!("wasm-font:{font_family}:{}", font.version),
            data: Arc::clone(&font.data),
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
            .and_then(|videos| videos.streams.get(&self.id).map(|entry| entry.metadata))
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
            .streams
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
        if let Some(entry) = videos.streams.get_mut(&self.id) {
            entry.frames.pin(frames);
        }
        videos.evict_to_budget(None);
    }
}

#[derive(Clone)]
struct WasmFontResolver {
    id: String,
    data: Arc<Vec<u8>>,
}

impl FontResolver for WasmFontResolver {
    fn id(&self) -> &str {
        &self.id
    }

    fn data(&self) -> Result<Vec<Vec<u8>>, MediaError> {
        Ok(vec![self.data.as_ref().clone()])
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

    #[wasm_bindgen(js_name = "removeFontFamily")]
    pub fn remove_font_family(&self, font_family: &str) -> Result<(), JsValue> {
        self.store
            .remove_font(font_family)
            .map_err(JsValue::from_str)
    }

    #[wasm_bindgen(js_name = "hasImage")]
    pub fn has_image(&self, image_id: &str) -> bool {
        self.store.has_image(image_id)
    }

    /// Sets the byte budget shared by all cached video frames; frames needed at the playhead are
    /// kept even when they exceed it.
    #[wasm_bindgen(js_name = "setVideoFrameCacheBytes")]
    pub fn set_video_frame_cache_bytes(&self, bytes: u32) -> Result<(), JsValue> {
        self.store
            .set_video_frame_cache_bytes(bytes as usize)
            .map_err(JsValue::from_str)
    }

    #[wasm_bindgen(js_name = "hasVideoFrame")]
    pub fn has_video_frame(&self, stream_id: &str, frame: u32) -> bool {
        self.store.has_video_frame(stream_id, frame)
    }

    #[wasm_bindgen(js_name = "hasFont")]
    pub fn has_font(&self, font_family: &str) -> bool {
        self.store.has_font(font_family)
    }

    #[wasm_bindgen(js_name = "setImage")]
    pub fn set_image(
        &self,
        image_id: &str,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    ) -> Result<(), JsValue> {
        if width == 0 || height == 0 {
            return Err(JsValue::from_str("image dimensions must be > 0"));
        }
        if !validate_rgba_len(width, height, rgba.len()) {
            return Err(JsValue::from_str("invalid image rgba buffer length"));
        }
        let frame = image_frame_from_rgba(width, height, rgba)
            .map_err(|error| JsValue::from_str(&error))?;
        self.store
            .set_image(image_id.to_string(), frame)
            .map_err(JsValue::from_str)
    }

    #[wasm_bindgen(js_name = "setFont")]
    pub fn set_font(&self, font_family: &str, data: Vec<u8>) -> Result<(), JsValue> {
        self.store
            .set_font(font_family.to_string(), data)
            .map_err(JsValue::from_str)
    }

    #[wasm_bindgen(js_name = "setVideoFrame")]
    pub fn set_video_frame(
        &self,
        stream_id: &str,
        frame: u32,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    ) -> Result<(), JsValue> {
        if width == 0 || height == 0 {
            return Err(JsValue::from_str("video frame dimensions must be > 0"));
        }
        if !validate_rgba_len(width, height, rgba.len()) {
            return Err(JsValue::from_str("invalid video rgba buffer length"));
        }
        let image = image_frame_from_rgba(width, height, rgba)
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

impl Default for LumenMediaStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_cache_evicts_oldest_frames_across_streams_by_bytes() {
        let store = WasmMediaStore::default();
        store
            .set_video_frame_cache_bytes(3 * FRAME_BYTES)
            .expect("set budget");
        insert_frames(&store, "a", 0..2);
        insert_frames(&store, "b", 0..2);

        assert!(!store.has_video_frame("a", 0), "oldest frame is evicted");
        assert!(store.has_video_frame("a", 1));
        assert!(store.has_video_frame("b", 0));
        assert!(store.has_video_frame("b", 1));
    }

    #[test]
    fn video_cache_evicts_farthest_from_pinned_playhead_first() {
        let store = WasmMediaStore::default();
        insert_frames(&store, "a", 0..6);
        store
            .pin_video_frames(&[VideoFrameRequirement {
                stream_id: "a".to_string(),
                frames: vec![2],
            }])
            .expect("pin frames");
        store
            .set_video_frame_cache_bytes(3 * FRAME_BYTES)
            .expect("set budget");

        assert!(
            !store.has_video_frame("a", 0),
            "frames behind the playhead go first"
        );
        assert!(!store.has_video_frame("a", 1));
        assert!(
            !store.has_video_frame("a", 5),
            "then the farthest frame ahead"
        );
        assert!(store.has_video_frame("a", 2), "pinned frame is kept");
        assert!(store.has_video_frame("a", 3));
        assert!(store.has_video_frame("a", 4));
    }

    #[test]
    fn moving_pins_lets_next_frames_of_every_stream_coexist() {
        let store = WasmMediaStore::default();
        store
            .set_video_frame_cache_bytes(2 * FRAME_BYTES)
            .expect("set budget");
        let pin = |frame: u32| {
            store
                .pin_video_frames(&["a", "b"].map(|stream_id| VideoFrameRequirement {
                    stream_id: stream_id.to_string(),
                    frames: vec![frame],
                }))
                .expect("pin frames");
        };
        pin(0);
        insert_frames(&store, "a", 0..1);
        insert_frames(&store, "b", 0..1);

        pin(1);
        insert_frames(&store, "a", 1..2);
        insert_frames(&store, "b", 1..2);

        assert!(store.has_video_frame("a", 1));
        assert!(store.has_video_frame("b", 1));
        assert!(!store.has_video_frame("a", 0));
        assert!(!store.has_video_frame("b", 0));
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

    #[test]
    fn font_resolver_returns_registered_bytes_and_versions_updates() {
        let store = WasmMediaStore::default();
        store
            .set_font("Inter".to_string(), vec![1, 2, 3])
            .expect("set font");

        let first = store
            .get_font_resolver("Inter")
            .expect("font resolver should exist");
        assert_eq!(first.data().expect("font data"), vec![vec![1, 2, 3]]);

        store
            .set_font("Inter".to_string(), vec![4, 5, 6])
            .expect("replace font");
        let second = store
            .get_font_resolver("Inter")
            .expect("font resolver should still exist");
        assert_ne!(first.id(), second.id());
        assert_eq!(second.data().expect("font data"), vec![vec![4, 5, 6]]);
    }

    const FRAME_BYTES: usize = 4;

    fn insert_frames(store: &WasmMediaStore, stream_id: &str, frames: std::ops::Range<u32>) {
        for frame in frames {
            let image =
                crate::utils::image_frame_from_rgba(1, 1, vec![255, 0, 0, 255]).expect("frame");
            store
                .set_video_frame(stream_id.to_string(), frame, image)
                .expect("set frame");
        }
    }
}
