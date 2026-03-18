//! Media resolver traits and test doubles for image/video sources.

use std::{fmt::Debug, sync::Arc};

use crate::error::MediaError;

#[derive(Debug, Clone, Copy, Default)]
pub struct ImageMetadata {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct VideoMetadata {
    pub width: u32,
    pub height: u32,
    pub frame_count: u32,
}

pub trait ImageResolver: Send + Sync {
    fn id(&self) -> &str;

    fn metadata(&self) -> ImageMetadata;

    fn resolve(&self) -> Result<Arc<Vec<u8>>, MediaError>;
}

pub trait VideoFrameResolver: Send + Sync {
    fn id(&self) -> &str;

    fn metadata(&self) -> VideoMetadata;

    fn resolve_frame(&self, frame: u32) -> Result<Arc<Vec<u8>>, MediaError>;
}

pub trait MediaStore: Send + Sync + Debug {
    fn get_image_resolver(&self, source: &str) -> Option<Box<dyn ImageResolver>>;

    fn get_video_resolver(&self, stream: &str) -> Option<Box<dyn VideoFrameResolver>>;
}
