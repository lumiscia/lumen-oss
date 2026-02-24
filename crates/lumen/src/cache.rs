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
        signature_hash: u64,
    ) -> Option<Arc<Vec<u8>>> {
        self.memo_cache.get(cache_id, width, height, signature_hash)
    }

    pub fn memo_insert(
        &mut self,
        cache_id: impl Into<String>,
        width: u32,
        height: u32,
        signature_hash: u64,
        bitmap: Arc<Vec<u8>>,
    ) {
        self.memo_cache
            .insert(cache_id.into(), width, height, signature_hash, bitmap);
    }
}

#[derive(Debug, Default)]
pub struct MemoCache {
    entries: HashMap<(String, u32, u32, u64), Arc<Vec<u8>>>,
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
        signature_hash: u64,
    ) -> Option<Arc<Vec<u8>>> {
        self.entries
            .get(&(cache_id.to_string(), width, height, signature_hash))
            .cloned()
    }

    pub fn insert(
        &mut self,
        cache_id: String,
        width: u32,
        height: u32,
        signature_hash: u64,
        bitmap: Arc<Vec<u8>>,
    ) {
        self.entries
            .insert((cache_id, width, height, signature_hash), bitmap);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeOutputCacheKey {
    pub node_id: NodeId,
    pub frame: u32,
    pub resolution_key: u32,
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
        resolution_key: u32,
        graph_revision: u64,
        value: PortValue,
    ) {
        self.outputs.insert(
            NodeOutputCacheKey {
                node_id,
                frame,
                resolution_key,
                graph_revision,
            },
            value,
        );
    }

    pub fn get(
        &self,
        node_id: NodeId,
        frame: u32,
        resolution_key: u32,
        graph_revision: u64,
    ) -> Option<&PortValue> {
        self.outputs.get(&NodeOutputCacheKey {
            node_id,
            frame,
            resolution_key,
            graph_revision,
        })
    }

    pub fn clear(&mut self) {
        self.outputs.clear();
    }
}

pub type SharedAssetCache = Arc<RwLock<AssetCache>>;
