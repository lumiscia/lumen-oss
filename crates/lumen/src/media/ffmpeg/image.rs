use std::{collections::HashMap, sync::Arc};

use crate::{
    error::MediaError,
    gpu_image::{AlphaMode, GpuImageFrame, RectI},
    media::premultiply_rgba_in_place_if_needed,
};

pub(super) struct FrameImage {
    pub source: String,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl FrameImage {
    pub fn into_gpu_image(mut self) -> Result<Arc<GpuImageFrame>, MediaError> {
        premultiply_rgba_in_place_if_needed(&mut self.rgba);
        Ok(Arc::new(
            GpuImageFrame::from_cpu_decoded_rgba(
                self.rgba.as_slice(),
                self.width,
                self.height,
                (self.width as usize) * 4,
                AlphaMode::Premultiplied,
                RectI::from_size(self.width, self.height),
                RectI::from_size(self.width, self.height),
            )
            .map_err(|error| MediaError::Decode {
                media_source: self.source,
                details: error.to_string(),
            })?,
        ))
    }
}

#[derive(Default)]
pub(super) struct FrameLruCache {
    entries: HashMap<u32, Arc<GpuImageFrame>>,
}

impl FrameLruCache {
    pub fn get(&mut self, frame: u32) -> Option<Arc<GpuImageFrame>> {
        self.entries.get(&frame).cloned()
    }

    pub fn insert(&mut self, frame: u32, data: Arc<GpuImageFrame>) {
        self.entries.insert(frame, data);
    }

    pub fn retain(&mut self, frames: &[u32]) {
        let keep: std::collections::HashSet<_> = frames.iter().copied().collect();
        self.entries.retain(|frame, _| keep.contains(frame));
    }
}
