//! Shared and per-session cache primitives used by render contexts.

use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};

use crate::{
    error::MediaError,
    media::ImageResolver,
    node::{NodeId, PortValue},
};

#[derive(Debug, Default)]
pub struct AssetCache {
    decoded_images: HashMap<String, Arc<Vec<u8>>>,
    video_metadata: HashMap<String, VideoMetadata>,
    memo_cache: MemoCache,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct VideoMetadata {
    pub width: u32,
    pub height: u32,
    pub frame_count: u32,
}

#[derive(Debug, Clone)]
pub struct CachedBitmap {
    pub pixels: Arc<Vec<u8>>,
    pub width: u32,
    pub height: u32,
}

impl AssetCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get_image(&self, source: &str) -> Option<Arc<Vec<u8>>> {
        self.decoded_images.get(source).cloned()
    }

    pub fn get_or_insert_image(
        &mut self,
        source: &str,
        resolver: &dyn ImageResolver,
    ) -> Result<Arc<Vec<u8>>, MediaError> {
        if let Some(pixels) = self.get_image(source) {
            return Ok(pixels);
        }

        let decoded = resolver.resolve()?;
        self.insert_image(source.to_string(), Arc::clone(&decoded));
        Ok(decoded)
    }
    pub fn insert_image(&mut self, source: impl Into<String>, pixels: Arc<Vec<u8>>) {
        self.decoded_images.insert(source.into(), pixels);
    }

    pub fn set_video_metadata(&mut self, source: impl Into<String>, metadata: VideoMetadata) {
        self.video_metadata.insert(source.into(), metadata);
    }

    pub fn video_metadata(&self, source: &str) -> Option<VideoMetadata> {
        self.video_metadata.get(source).copied()
    }

    pub fn memo_get(
        &self,
        cache_id: &str,
        width: u32,
        height: u32,
        request_hash: u64,
        signature_hash: u64,
    ) -> Option<CachedBitmap> {
        self.memo_cache
            .get(cache_id, width, height, request_hash, signature_hash)
    }

    pub fn memo_insert(
        &mut self,
        cache_id: impl Into<String>,
        width: u32,
        height: u32,
        request_hash: u64,
        signature_hash: u64,
        bitmap: CachedBitmap,
    ) {
        self.memo_cache.insert(
            cache_id.into(),
            width,
            height,
            request_hash,
            signature_hash,
            bitmap,
        );
    }
}

#[derive(Debug, Default)]
pub struct MemoCache {
    /// Two-level map: cache_id -> (width, height, request_hash, signature_hash) -> bitmap.
    /// This avoids allocating a String on every `get` lookup.
    entries: HashMap<String, HashMap<(u32, u32, u64, u64), CachedBitmap>>,
}

impl MemoCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(
        &self,
        cache_id: &str,
        width: u32,
        height: u32,
        request_hash: u64,
        signature_hash: u64,
    ) -> Option<CachedBitmap> {
        self.entries
            .get(cache_id)?
            .get(&(width, height, request_hash, signature_hash))
            .cloned()
    }

    pub fn insert(
        &mut self,
        cache_id: String,
        width: u32,
        height: u32,
        request_hash: u64,
        signature_hash: u64,
        bitmap: CachedBitmap,
    ) {
        self.entries
            .entry(cache_id)
            .or_default()
            .insert((width, height, request_hash, signature_hash), bitmap);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeOutputCacheKey {
    pub node_id: NodeId,
    pub frame: u32,
    pub request_key: u64,
    pub graph_revision: u64,
}

#[derive(Debug, Default)]
pub struct NodeOutputCache {
    outputs: HashMap<NodeOutputCacheKey, PortValue>,
}

impl NodeOutputCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(
        &mut self,
        node_id: NodeId,
        frame: u32,
        request_key: u64,
        graph_revision: u64,
        value: PortValue,
    ) {
        self.outputs.insert(
            NodeOutputCacheKey {
                node_id,
                frame,
                request_key,
                graph_revision,
            },
            value,
        );
    }

    pub fn get(
        &self,
        node_id: NodeId,
        frame: u32,
        request_key: u64,
        graph_revision: u64,
    ) -> Option<&PortValue> {
        self.outputs.get(&NodeOutputCacheKey {
            node_id,
            frame,
            request_key,
            graph_revision,
        })
    }

    pub fn clear(&mut self) {
        self.outputs.clear();
    }
}

pub type SharedAssetCache = Arc<RwLock<AssetCache>>;
