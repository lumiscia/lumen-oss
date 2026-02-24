//! Media resolver traits and test doubles for image/video sources.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use crate::error::MediaError;

pub trait ImageResolver: Send + Sync {
    fn id(&self) -> &str;
    fn width(&self) -> u32;
    fn height(&self) -> u32;
    fn resolve(&self) -> Result<Vec<u8>, MediaError>;
}

pub trait VideoFrameResolver: Send + Sync {
    fn id(&self) -> &str;
    fn width(&self) -> u32;
    fn height(&self) -> u32;
    fn frame_count(&self) -> u32;
    fn resolve_frame(&self, frame: u32) -> Result<Vec<u8>, MediaError>;
}

pub trait MediaStore: Send + Sync {
    fn get_image_resolver(&self, source: &str) -> Option<Box<dyn ImageResolver>>;
    fn get_video_resolver(&self, source: &str) -> Option<Box<dyn VideoFrameResolver>>;
}

#[derive(Clone)]
pub struct MockImageResolver {
    id: String,
    width: u32,
    height: u32,
    pixels: Arc<Vec<u8>>,
}

impl MockImageResolver {
    pub fn new(id: impl Into<String>, width: u32, height: u32, pixels: Vec<u8>) -> Self {
        Self {
            id: id.into(),
            width,
            height,
            pixels: Arc::new(pixels),
        }
    }
}

impl ImageResolver for MockImageResolver {
    fn id(&self) -> &str {
        &self.id
    }

    fn width(&self) -> u32 {
        self.width
    }

    fn height(&self) -> u32 {
        self.height
    }

    fn resolve(&self) -> Result<Vec<u8>, MediaError> {
        Ok(self.pixels.as_ref().clone())
    }
}

#[derive(Clone)]
pub struct MockVideoResolver {
    id: String,
    width: u32,
    height: u32,
    frame_count: u32,
    pixels: Arc<Vec<u8>>,
    requested_frames: Arc<Mutex<Vec<u32>>>,
}

impl MockVideoResolver {
    pub fn new(
        id: impl Into<String>,
        width: u32,
        height: u32,
        frame_count: u32,
        pixels: Vec<u8>,
    ) -> Self {
        Self {
            id: id.into(),
            width,
            height,
            frame_count,
            pixels: Arc::new(pixels),
            requested_frames: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn requested_frames(&self) -> Arc<Mutex<Vec<u32>>> {
        Arc::clone(&self.requested_frames)
    }
}

impl VideoFrameResolver for MockVideoResolver {
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
        if let Ok(mut requested) = self.requested_frames.lock() {
            requested.push(frame);
        }
        if frame >= self.frame_count {
            return Err(MediaError::FrameOutOfRange {
                media_source: self.id.clone(),
                frame,
                frame_count: self.frame_count,
            });
        }
        Ok(self.pixels.as_ref().clone())
    }
}

#[derive(Default)]
pub struct MockMediaStore {
    images: HashMap<String, MockImageResolver>,
    videos: HashMap<String, MockVideoResolver>,
}

impl MockMediaStore {
    pub fn insert_image(&mut self, resolver: MockImageResolver) {
        self.images.insert(resolver.id().to_string(), resolver);
    }

    pub fn insert_video(&mut self, resolver: MockVideoResolver) {
        self.videos.insert(resolver.id().to_string(), resolver);
    }
}

impl MediaStore for MockMediaStore {
    fn get_image_resolver(&self, source: &str) -> Option<Box<dyn ImageResolver>> {
        self.images
            .get(source)
            .cloned()
            .map(|resolver| Box::new(resolver) as Box<dyn ImageResolver>)
    }

    fn get_video_resolver(&self, source: &str) -> Option<Box<dyn VideoFrameResolver>> {
        self.videos
            .get(source)
            .cloned()
            .map(|resolver| Box::new(resolver) as Box<dyn VideoFrameResolver>)
    }
}
