//! Media resolver traits and shared metadata for image/video sources.

use std::{fmt::Debug, sync::Arc};

use crate::{error::MediaError, raster::ImageFrame};

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

    fn resolve_image(&self) -> Result<Arc<ImageFrame>, MediaError>;
}

pub trait VideoFrameResolver: Send + Sync {
    fn id(&self) -> &str;

    fn metadata(&self) -> VideoMetadata;

    fn resolve_frame_image(&self, frame: u32) -> Result<Arc<ImageFrame>, MediaError>;
}

pub trait MediaStore: Send + Sync + Debug {
    fn get_image_resolver(&self, source: &str) -> Option<Box<dyn ImageResolver>>;

    fn get_video_resolver(&self, stream: &str) -> Option<Box<dyn VideoFrameResolver>>;
}

pub fn premultiply_rgba_in_place_if_needed(pixels: &mut [u8]) {
    for pixel in pixels.chunks_exact_mut(4) {
        let alpha = u16::from(pixel[3]);
        if alpha == u16::from(u8::MAX) {
            continue;
        }
        if alpha == 0 {
            pixel[0] = 0;
            pixel[1] = 0;
            pixel[2] = 0;
            continue;
        }
        for channel in &mut pixel[..3] {
            *channel = ((u16::from(*channel) * alpha) + 127)
                .checked_div(u16::from(u8::MAX))
                .unwrap_or(0) as u8;
        }
    }
}
