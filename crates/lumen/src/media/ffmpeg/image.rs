use std::{collections::HashMap, sync::Arc};

use crate::{
    error::MediaError,
    media::{CpuMediaFrame, premultiply_rgba_in_place_if_needed},
};

pub(super) struct FrameImage {
    pub source: String,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub premultiply: bool,
}

impl FrameImage {
    pub fn into_media_frame(mut self) -> Result<Arc<CpuMediaFrame>, MediaError> {
        if self.premultiply {
            premultiply_rgba_in_place_if_needed(&mut self.rgba);
        }
        let expected = self.width as usize * self.height as usize * 4;
        if self.rgba.len() < expected {
            return Err(MediaError::Decode {
                media_source: self.source,
                details: "decoded frame did not contain enough RGBA data".to_string(),
            });
        }
        Ok(Arc::new(CpuMediaFrame {
            rgba: Arc::new(self.rgba),
            width: self.width,
            height: self.height,
            row_bytes: self.width as usize * 4,
        }))
    }
}

#[derive(Default)]
pub(super) struct FrameLruCache {
    entries: HashMap<u32, Arc<CpuMediaFrame>>,
}

impl FrameLruCache {
    pub fn get(&mut self, frame: u32) -> Option<Arc<CpuMediaFrame>> {
        self.entries.get(&frame).cloned()
    }

    pub fn insert(&mut self, frame: u32, data: Arc<CpuMediaFrame>) {
        self.entries.insert(frame, data);
    }

    pub fn retain(&mut self, frames: &[u32]) {
        let keep: std::collections::HashSet<_> = frames.iter().copied().collect();
        self.entries.retain(|frame, _| keep.contains(frame));
    }
}
