//! FFmpeg-backed media resolver placeholders.

use crate::{
    error::MediaError,
    media::{ImageResolver, MediaStore, VideoFrameResolver},
};

#[derive(Debug, Clone)]
pub struct FfmpegVideoResolver {
    id: String,
    width: u32,
    height: u32,
    frame_count: u32,
}

impl FfmpegVideoResolver {
    pub fn new(id: impl Into<String>, width: u32, height: u32, frame_count: u32) -> Self {
        Self {
            id: id.into(),
            width,
            height,
            frame_count,
        }
    }
}

impl VideoFrameResolver for FfmpegVideoResolver {
    fn id(&self) -> &str {
        &self.id
    }

    fn width(&self) -> u32 {
        self.width
    }

    fn height(&self) -> u32 {
        self.height
    }

    fn frame_count(&self) -> u32 {
        self.frame_count
    }

    fn resolve_frame(&self, frame: u32) -> Result<Vec<u8>, MediaError> {
        Err(MediaError::FrameOutOfRange {
            media_source: self.id.clone(),
            frame,
            frame_count: self.frame_count,
        })
    }
}

#[derive(Default)]
pub struct FfmpegMediaStore;

impl MediaStore for FfmpegMediaStore {
    fn get_image_resolver(&self, _source: &str) -> Option<Box<dyn ImageResolver>> {
        None
    }

    fn get_video_resolver(&self, _source: &str) -> Option<Box<dyn VideoFrameResolver>> {
        None
    }
}
