use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, RwLock},
};

use lumen_engine::{
    error::{MediaError, MediaFailure},
    media::{
        AssetKind, CpuMediaFrame, FontResolver, ImageMetadata, ImageResolver, MediaFrame,
        MediaIdentity, MediaResolver, MediaStore, ModelResolver, ResolvedMedia, TextSession,
        VideoFrameRequirement, VideoFrameResolver, VideoMetadata, YuvFrame,
    },
};
/// Total bytes of decoded video frames kept across all streams of one media store.
pub(crate) const DEFAULT_VIDEO_FRAME_CACHE_BYTES: usize = 512 * 1024 * 1024;

// ── VideoFrameCache ───────────────────────────────────────────────────────────

/// A decoded video frame, as RGBA pixels or as YUV planes the renderer converts.
#[derive(Debug, Clone)]
pub(crate) enum VideoFrameData {
    /// Pixels as decoded, which the renderer premultiplies unless the node says they are.
    RawRgba(Arc<CpuMediaFrame>),
    /// Planes, and the size the frame shows at, which carries the source's pixel aspect ratio.
    Yuv(Arc<YuvFrame>, (u32, u32)),
}

impl VideoFrameData {
    fn bytes(&self) -> usize {
        match self {
            Self::RawRgba(frame) => frame.rgba.len(),
            Self::Yuv(frame, _) => frame.data.len(),
        }
    }

    /// The size the frame shows at.
    fn display_size(&self) -> (u32, u32) {
        match self {
            Self::RawRgba(frame) => (frame.width, frame.height),
            Self::Yuv(_, size) => *size,
        }
    }

    fn into_media_frame(self) -> MediaFrame {
        match self {
            Self::RawRgba(frame) => MediaFrame::RawRgba(frame),
            Self::Yuv(frame, _) => MediaFrame::Yuv(frame),
        }
    }
}

#[derive(Debug, Clone)]
struct CachedFrame {
    image: VideoFrameData,
    inserted: u64,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct VideoFrameCache {
    entries: HashMap<u32, CachedFrame>,
    bytes: usize,
}

impl VideoFrameCache {
    pub fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }

    pub fn contains(&self, frame: u32) -> bool {
        self.entries.contains_key(&frame)
    }

    pub fn get(&self, frame: u32) -> Option<VideoFrameData> {
        self.entries.get(&frame).map(|entry| entry.image.clone())
    }

    fn insert(&mut self, frame: u32, image: VideoFrameData, inserted: u64) {
        self.bytes += image.bytes();
        if let Some(previous) = self.entries.insert(frame, CachedFrame { image, inserted }) {
            self.bytes -= previous.image.bytes();
        }
    }

    fn remove(&mut self, frame: u32) {
        if let Some(previous) = self.entries.remove(&frame) {
            self.bytes -= previous.image.bytes();
        }
    }

    /// The unprotected frame to evict first: frames behind the playhead (the earliest protected
    /// frame) go before frames ahead of it, farthest first. Without a playhead the oldest insert
    /// goes.
    fn eviction_candidate(
        &self,
        protected: &HashSet<u32>,
        keep: Option<u32>,
    ) -> Option<(EvictionRank, u32)> {
        let playhead = protected.iter().min().copied();
        self.entries
            .iter()
            .filter(|(frame, _)| Some(**frame) != keep && !protected.contains(frame))
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
    /// Frames eviction must keep, by stream id. Replaced wholesale by whoever owns the playhead
    /// and independent of stream registration, so it cannot go stale when streams change.
    protected: HashMap<String, HashSet<u32>>,
    budget_bytes: usize,
    insert_clock: u64,
}

impl Default for VideoStore {
    fn default() -> Self {
        Self {
            streams: HashMap::new(),
            protected: HashMap::new(),
            budget_bytes: DEFAULT_VIDEO_FRAME_CACHE_BYTES,
            insert_clock: 0,
        }
    }
}

impl VideoStore {
    fn insert_frame(&mut self, stream_id: String, frame: u32, image: VideoFrameData) {
        self.insert_clock += 1;
        let entry = self.streams.entry(stream_id.clone()).or_default();
        (entry.metadata.width, entry.metadata.height) = image.display_size();
        entry.metadata.frame_count = entry.metadata.frame_count.max(frame.saturating_add(1));
        entry.frames.insert(frame, image, self.insert_clock);
        self.evict_to_budget(Some((&stream_id, frame)));
    }

    /// Evicts unprotected frames until the cache fits its byte budget. Protected frames and the
    /// `keep` frame are never evicted, so the budget can be exceeded by frames in use.
    fn evict_to_budget(&mut self, keep: Option<(&str, u32)>) {
        let unprotected = HashSet::new();
        let mut total: usize = self.streams.values().map(|video| video.frames.bytes).sum();
        while total > self.budget_bytes {
            let victim = self
                .streams
                .iter()
                .filter_map(|(stream_id, video)| {
                    let keep = keep
                        .filter(|(keep_id, _)| keep_id == stream_id)
                        .map(|(_, frame)| frame);
                    let protected = self.protected.get(stream_id).unwrap_or(&unprotected);
                    video
                        .frames
                        .eviction_candidate(protected, keep)
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
    identity: MediaIdentity,
}

/// A stream, which keeps its identity until it is removed.
#[derive(Debug, Clone)]
struct StoredVideo {
    metadata: VideoMetadata,
    frames: VideoFrameCache,
    identity: MediaIdentity,
}

impl Default for StoredVideo {
    fn default() -> Self {
        Self {
            metadata: VideoMetadata::default(),
            frames: VideoFrameCache::default(),
            identity: MediaIdentity::unique(),
        }
    }
}

#[derive(Debug, Clone)]
struct StoredModel {
    data: Arc<Vec<u8>>,
    identity: MediaIdentity,
}

#[derive(Debug, Clone)]
struct StoredFont {
    data: Arc<Vec<u8>>,
    identity: MediaIdentity,
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
    models: RwLock<HashMap<String, StoredModel>>,
    videos: RwLock<VideoStore>,
    text: TextSession,
}

impl WasmMediaStore {
    pub fn clear(&self) -> Result<(), &'static str> {
        self.inner
            .images
            .write()
            .map_err(|_| "media store lock poisoned")?
            .clear();
        self.inner
            .models
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
        self.inner
            .text
            .reset()
            .map_err(|_| "media store lock poisoned")
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

    pub fn remove_model(&self, source: &str) -> Result<(), &'static str> {
        self.inner
            .models
            .write()
            .map_err(|_| "media store lock poisoned")?
            .remove(source);
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
        self.inner
            .text
            .remove_family(font_family)
            .map_err(|_| "media store lock poisoned")
    }

    pub fn has_image(&self, image_id: &str) -> bool {
        self.inner
            .images
            .read()
            .ok()
            .is_some_and(|images| images.contains_key(image_id))
    }

    pub fn has_model(&self, source: &str) -> bool {
        self.inner
            .models
            .read()
            .ok()
            .is_some_and(|models| models.contains_key(source))
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

    /// Replaces the set of frames eviction must keep with `requirements`.
    pub fn protect_video_frames(
        &self,
        requirements: &[VideoFrameRequirement],
    ) -> Result<(), &'static str> {
        let mut videos = self
            .inner
            .videos
            .write()
            .map_err(|_| "media store lock poisoned")?;
        videos.protected.clear();
        for requirement in requirements {
            videos
                .protected
                .entry(requirement.stream_id.clone())
                .or_default()
                .extend(requirement.frames.iter().copied());
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
                    identity: MediaIdentity::unique(),
                },
            );
        Ok(())
    }

    /// Stores the bytes of the model file `source` names, which the engine reads when it draws it.
    pub fn set_model(&self, source: String, data: Vec<u8>) -> Result<(), &'static str> {
        self.inner
            .models
            .write()
            .map_err(|_| "media store lock poisoned")?
            .insert(
                source,
                StoredModel {
                    data: Arc::new(data),
                    identity: MediaIdentity::unique(),
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
        fonts.insert(
            font_family.to_string(),
            StoredFont {
                data: Arc::new(data),
                identity: MediaIdentity::unique(),
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
        self.set_video_frame_data(stream_id, frame, VideoFrameData::RawRgba(Arc::new(image)))
    }

    pub fn set_video_frame_yuv(
        &self,
        stream_id: String,
        frame: u32,
        yuv: YuvFrame,
        display_size: (u32, u32),
    ) -> Result<(), &'static str> {
        self.set_video_frame_data(
            stream_id,
            frame,
            VideoFrameData::Yuv(Arc::new(yuv), display_size),
        )
    }

    fn set_video_frame_data(
        &self,
        stream_id: String,
        frame: u32,
        image: VideoFrameData,
    ) -> Result<(), &'static str> {
        self.inner
            .videos
            .write()
            .map_err(|_| "media store lock poisoned")?
            .insert_frame(stream_id, frame, image);
        Ok(())
    }
}

impl WasmMediaStore {
    /// An image, which resolves before it is set so that a frame can be laid out while it loads.
    fn image(&self, source: &str) -> Result<ResolvedMedia, MediaError> {
        let identity = self
            .inner
            .images
            .read()
            .map_err(|_| MediaError::source_not_found(source, "media store lock poisoned"))?
            .get(source)
            .map(|image| image.identity.clone());
        Ok(ResolvedMedia::new(
            identity.unwrap_or_else(|| unset("image", source)),
            MediaResolver::Image(Box::new(WasmImageResolver {
                source: source.to_string(),
                store: Arc::clone(&self.inner),
            })),
        ))
    }

    /// A stream, which resolves before any of it is set, as its frames arrive one by one.
    fn video(&self, stream_id: &str) -> Result<ResolvedMedia, MediaError> {
        let identity = self
            .inner
            .videos
            .read()
            .map_err(|_| MediaError::source_not_found(stream_id, "media store lock poisoned"))?
            .streams
            .get(stream_id)
            .map(|video| video.identity.clone());
        Ok(ResolvedMedia::new(
            identity.unwrap_or_else(|| unset("video", stream_id)),
            MediaResolver::Video(Box::new(WasmVideoResolver {
                id: stream_id.to_string(),
                store: Arc::clone(&self.inner),
            })),
        ))
    }

    fn model(&self, source: &str) -> Result<ResolvedMedia, MediaError> {
        let models = self
            .inner
            .models
            .read()
            .map_err(|_| MediaError::source_not_found(source, "media store lock poisoned"))?;
        let model = models.get(source).ok_or_else(|| {
            MediaError::source_not_found(source, "the preview has no model by this name in `media`")
        })?;
        Ok(ResolvedMedia::new(
            model.identity.clone(),
            MediaResolver::Model(Box::new(WasmModelResolver {
                data: Arc::clone(&model.data),
            })),
        ))
    }

    fn font(&self, font_family: &str) -> Result<ResolvedMedia, MediaError> {
        let fonts =
            self.inner.fonts.read().map_err(|_| {
                MediaError::source_not_found(font_family, "media store lock poisoned")
            })?;
        let font = fonts.get(font_family).ok_or_else(|| {
            MediaError::source_not_found(font_family, "the preview has no font by this name")
        })?;
        Ok(ResolvedMedia::new(
            font.identity.clone(),
            MediaResolver::Font(Box::new(WasmFontResolver {
                data: Arc::clone(&font.data),
            })),
        ))
    }
}

/// The identity of a source of `kind` that has nothing set yet.
fn unset(kind: &str, source: &str) -> MediaIdentity {
    MediaIdentity::of_key(&format!("wasm-unset-{kind}:{source}"))
}

impl MediaStore for WasmMediaStore {
    fn resolve(&self, source: &str, kind: AssetKind) -> Result<ResolvedMedia, MediaError> {
        match kind {
            AssetKind::Image => self.image(source),
            AssetKind::Video => self.video(source),
            AssetKind::Model => self.model(source),
            AssetKind::Font => self.font(source),
            AssetKind::Environment => Err(MediaFailure::unsupported(
                source,
                "the browser preview has no environment maps; render this composition on a \
                 Lumen server",
            )
            .wanting(&[AssetKind::Environment])
            .into()),
            AssetKind::Audio => Err(MediaError::source_not_found(
                source,
                "the browser preview plays audio itself",
            )),
        }
    }

    fn text_session(&self) -> Option<&TextSession> {
        Some(&self.inner.text)
    }
}

// ── Resolvers ─────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct WasmImageResolver {
    source: String,
    store: Arc<WasmMediaStoreInner>,
}

impl ImageResolver for WasmImageResolver {
    fn metadata(&self) -> ImageMetadata {
        self.store
            .images
            .read()
            .ok()
            .and_then(|images| images.get(&self.source).map(|entry| entry.metadata))
            .unwrap_or_default()
    }

    fn frame(&self) -> Result<MediaFrame, MediaError> {
        self.store
            .images
            .read()
            .map_err(|_| MediaError::source_not_found(&self.source, "media store lock poisoned"))?
            .get(&self.source)
            .map(|entry| Arc::clone(&entry.frame))
            .ok_or_else(|| MediaError::source_not_found(&self.source, "image is not registered"))
            .map(MediaFrame::RawRgba)
    }
}

#[derive(Clone)]
struct WasmVideoResolver {
    id: String,
    store: Arc<WasmMediaStoreInner>,
}

impl VideoFrameResolver for WasmVideoResolver {
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
            .map_err(|_| MediaError::source_not_found(&self.id, "media store lock poisoned"))?;
        let entry = videos
            .streams
            .get(&self.id)
            .ok_or_else(|| MediaError::source_not_found(&self.id, "video is not registered"))?;
        entry
            .frames
            .get(frame)
            .ok_or(MediaError::FrameOutOfRange {
                media_source: self.id.clone(),
                frame,
                frame_count: entry.metadata.frame_count,
            })
            .map(VideoFrameData::into_media_frame)
    }

    fn retain_frames(&self, frames: &[u32]) {
        let Ok(mut videos) = self.store.videos.write() else {
            return;
        };
        videos
            .protected
            .insert(self.id.clone(), frames.iter().copied().collect());
        videos.evict_to_budget(None);
    }
}

struct WasmModelResolver {
    data: Arc<Vec<u8>>,
}

impl ModelResolver for WasmModelResolver {
    fn data(&self) -> Result<Vec<u8>, MediaError> {
        Ok(self.data.as_ref().clone())
    }
}

#[derive(Clone)]
struct WasmFontResolver {
    data: Arc<Vec<u8>>,
}

impl FontResolver for WasmFontResolver {
    fn data(&self) -> Result<Vec<Vec<u8>>, MediaError> {
        Ok(vec![self.data.as_ref().clone()])
    }
}

#[cfg(test)]
mod tests {
    use lumen_engine::media::{ResolverKind, resolve_as};

    use super::*;

    fn resolver<K: ResolverKind + ?Sized>(store: &WasmMediaStore, source: &str) -> Box<K> {
        match resolve_as::<K>(store, source) {
            Ok((_, resolver)) => resolver,
            Err(error) => panic!("{source}: {error}"),
        }
    }

    fn identity(store: &WasmMediaStore, source: &str, kind: AssetKind) -> MediaIdentity {
        match store.resolve(source, kind) {
            Ok(resolved) => resolved.identity,
            Err(error) => panic!("{source}: {error}"),
        }
    }

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
    fn video_cache_evicts_farthest_from_protected_playhead_first() {
        let store = WasmMediaStore::default();
        insert_frames(&store, "a", 0..6);
        store
            .protect_video_frames(&[VideoFrameRequirement {
                stream_id: "a".to_string(),
                frames: vec![2],
            }])
            .expect("protect frames");
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
        assert!(store.has_video_frame("a", 2), "protected frame is kept");
        assert!(store.has_video_frame("a", 3));
        assert!(store.has_video_frame("a", 4));
    }

    #[test]
    fn moving_protection_lets_next_frames_of_every_stream_coexist() {
        let store = WasmMediaStore::default();
        store
            .set_video_frame_cache_bytes(2 * FRAME_BYTES)
            .expect("set budget");
        let protect = |frame: u32| {
            store
                .protect_video_frames(&["a", "b"].map(|stream_id| VideoFrameRequirement {
                    stream_id: stream_id.to_string(),
                    frames: vec![frame],
                }))
                .expect("protect frames");
        };
        protect(0);
        insert_frames(&store, "a", 0..1);
        insert_frames(&store, "b", 0..1);

        protect(1);
        insert_frames(&store, "a", 1..2);
        insert_frames(&store, "b", 1..2);

        assert!(store.has_video_frame("a", 1));
        assert!(store.has_video_frame("b", 1));
        assert!(!store.has_video_frame("a", 0));
        assert!(!store.has_video_frame("b", 0));
    }

    #[test]
    fn yuv_frames_count_their_bytes_and_resolve_as_yuv() {
        use lumen_engine::media::{
            ChromaSubsampling, Rotation, YuvChroma, YuvColor, YuvFormat, YuvLayout, YuvMatrix,
        };
        let format = YuvFormat {
            width: 4,
            height: 2,
            subsampling: ChromaSubsampling::Yuv420,
            depth: 8,
            msb_aligned: false,
            color: YuvColor {
                matrix: YuvMatrix::Bt709,
                full_range: false,
            },
            rotation: Rotation::Cw90,
        };
        let (layout, bytes) = YuvLayout::packed(&format, true, false);
        let yuv = || YuvFrame {
            format,
            layout,
            data: Arc::new(vec![0; bytes]),
        };
        assert!(matches!(layout.chroma, YuvChroma::Interleaved(_)));

        let store = WasmMediaStore::default();
        store
            .set_video_frame_cache_bytes(2 * bytes)
            .expect("set budget");
        for frame in 0..3 {
            store
                .set_video_frame_yuv("a".to_string(), frame, yuv(), (3, 4))
                .expect("set frame");
        }

        assert!(!store.has_video_frame("a", 0), "two frames fit the budget");
        assert!(store.has_video_frame("a", 1));
        assert!(store.has_video_frame("a", 2));
        let resolver = resolver::<dyn VideoFrameResolver>(&store, "a");
        assert!(matches!(resolver.frame(2), Ok(MediaFrame::Yuv(_))));
        assert_eq!(
            (resolver.metadata().width, resolver.metadata().height),
            (3, 4)
        );
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
        let frame = crate::utils::raw_frame_from_rgba(1, 1, vec![255, 0, 0, 255]).expect("frame");
        store
            .set_video_frame("intro".to_string(), 0, frame)
            .expect("set frame");

        store.clear_video_frames().expect("clear video frames");

        let resolver = resolver::<dyn VideoFrameResolver>(&store, "intro");
        assert_eq!(resolver.metadata().frame_count, 240);
        assert!(matches!(
            resolver.frame(0),
            Err(MediaError::FrameOutOfRange { .. })
        ));
    }

    const INTER: &[u8] = include_bytes!("../../text/assets/inter/InterVariable-Latin.ttf");

    /// Roboto under its own name, drawn at half its size by a larger em square.
    fn half_size_roboto() -> Vec<u8> {
        let mut font = include_bytes!("../../text/assets/roboto/Roboto-Regular.ttf").to_vec();
        let tables = u16::from_be_bytes([font[4], font[5]]) as usize;
        let head = font[12..12 + tables * 16]
            .chunks(16)
            .find(|table| &table[..4] == b"head")
            .unwrap();
        let units = u32::from_be_bytes(head[8..12].try_into().unwrap()) as usize + 18;
        let em = u16::from_be_bytes([font[units], font[units + 1]]);
        font[units..units + 2].copy_from_slice(&(em * 2).to_be_bytes());
        font
    }

    fn measured_width(store: &WasmMediaStore, family: &str) -> f64 {
        lumen_engine::expr::Expression::parse(&format!(
            "text_width(\"Variable weight\", 16, 0, \"{family}\")"
        ))
        .unwrap()
        .evaluate(&lumen_engine::expr::ExpressionContext {
            media: Some(store),
            ..Default::default()
        })
        .unwrap()
        .as_f64()
        .unwrap()
    }

    #[test]
    fn removing_a_font_stops_text_measuring_with_it() {
        let store = WasmMediaStore::default();
        let fallback = measured_width(&store, "Inter");
        store.set_font("Inter".to_string(), INTER.to_vec()).unwrap();
        let loaded = measured_width(&store, "Inter");
        assert_ne!(loaded, fallback);

        store.remove_font("Inter").unwrap();
        assert_eq!(measured_width(&store, "Inter"), fallback);

        store.set_font("Inter".to_string(), INTER.to_vec()).unwrap();
        assert_eq!(measured_width(&store, "Inter"), loaded);
    }

    #[test]
    fn clearing_the_store_returns_text_to_the_bundled_fonts() {
        let store = WasmMediaStore::default();
        let fallback = measured_width(&store, "Inter");
        let roboto = measured_width(&store, "Roboto");
        store.set_font("Inter".to_string(), INTER.to_vec()).unwrap();
        store
            .set_font("Roboto".to_string(), half_size_roboto())
            .unwrap();
        assert_ne!(measured_width(&store, "Inter"), fallback);
        assert_ne!(measured_width(&store, "Roboto"), roboto);

        store.clear().unwrap();
        assert_eq!(measured_width(&store, "Inter"), fallback);
        assert_eq!(measured_width(&store, "Roboto"), roboto);
    }

    #[test]
    fn removing_a_font_named_like_the_bundled_one_restores_it() {
        let store = WasmMediaStore::default();
        let bundled = measured_width(&store, "Roboto");
        store
            .set_font("Roboto".to_string(), half_size_roboto())
            .unwrap();
        assert_ne!(measured_width(&store, "Roboto"), bundled);

        store.remove_font("Roboto").unwrap();
        assert_eq!(measured_width(&store, "Roboto"), bundled);
    }

    #[test]
    fn a_font_serves_its_registered_bytes_with_an_identity_each_set_changes() {
        let store = WasmMediaStore::default();
        store
            .set_font("Inter".to_string(), vec![1, 2, 3])
            .expect("set font");

        let first = identity(&store, "Inter", AssetKind::Font);
        let font = resolver::<dyn FontResolver>(&store, "Inter");
        assert_eq!(font.data().expect("font data"), vec![vec![1, 2, 3]]);
        assert_eq!(identity(&store, "Inter", AssetKind::Font), first);

        store
            .set_font("Inter".to_string(), vec![4, 5, 6])
            .expect("replace font");
        assert_ne!(identity(&store, "Inter", AssetKind::Font), first);
        let font = resolver::<dyn FontResolver>(&store, "Inter");
        assert_eq!(font.data().expect("font data"), vec![vec![4, 5, 6]]);
    }

    #[test]
    fn an_image_has_a_new_identity_when_it_is_replaced_or_set_again() {
        let image = || crate::utils::raw_frame_from_rgba(1, 1, vec![255, 0, 0, 255]).unwrap();
        let id = |store: &WasmMediaStore| identity(store, "plate", AssetKind::Image);
        let store = WasmMediaStore::default();
        store.set_image("plate".to_string(), image()).unwrap();
        let first = id(&store);
        assert_eq!(id(&store), first);

        store.set_image("plate".to_string(), image()).unwrap();
        let second = id(&store);
        assert_ne!(second, first);

        store.remove_image("plate").unwrap();
        store.set_image("plate".to_string(), image()).unwrap();
        assert_ne!(id(&store), second);
    }

    #[test]
    fn a_model_serves_the_set_bytes_with_an_identity_each_set_changes() {
        let store = WasmMediaStore::default();
        let resolve = |store: &WasmMediaStore| {
            let model = resolver::<dyn ModelResolver>(store, "fox");
            (
                identity(store, "fox", AssetKind::Model),
                model.data().unwrap(),
            )
        };
        assert!(store.resolve("fox", AssetKind::Model).is_err());

        store.set_model("fox".to_string(), vec![1, 2]).unwrap();
        let (first, data) = resolve(&store);
        assert_eq!(data, [1, 2]);
        assert_eq!(resolve(&store).0, first);

        store.set_model("fox".to_string(), vec![3]).unwrap();
        let (second, data) = resolve(&store);
        assert_eq!(data, [3]);
        assert_ne!(second, first);

        store.remove_model("fox").unwrap();
        assert!(!store.has_model("fox"));
        assert!(store.resolve("fox", AssetKind::Model).is_err());
    }

    const FRAME_BYTES: usize = 4;

    fn insert_frames(store: &WasmMediaStore, stream_id: &str, frames: std::ops::Range<u32>) {
        for frame in frames {
            let image =
                crate::utils::raw_frame_from_rgba(1, 1, vec![255, 0, 0, 255]).expect("frame");
            store
                .set_video_frame(stream_id.to_string(), frame, image)
                .expect("set frame");
        }
    }
}
