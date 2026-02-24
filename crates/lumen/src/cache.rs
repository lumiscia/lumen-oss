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

        let decoded = Arc::new(resolver.resolve()?);
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
}

#[derive(Debug, Default)]
pub struct NodeOutputCache {
    outputs: HashMap<NodeId, PortValue>,
}

impl NodeOutputCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, node_id: NodeId, value: PortValue) {
        self.outputs.insert(node_id, value);
    }

    pub fn get(&self, node_id: NodeId) -> Option<&PortValue> {
        self.outputs.get(&node_id)
    }

    pub fn clear(&mut self) {
        self.outputs.clear();
    }
}

pub type SharedAssetCache = Arc<RwLock<AssetCache>>;
