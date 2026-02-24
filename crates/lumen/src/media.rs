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
    fn resolve(&self) -> Result<Arc<Vec<u8>>, MediaError>;
}

pub trait VideoFrameResolver: Send + Sync {
    fn id(&self) -> &str;
    fn width(&self) -> u32;
    fn height(&self) -> u32;
    fn frame_count(&self) -> u32;
    fn resolve_frame(&self, frame: u32) -> Result<Arc<Vec<u8>>, MediaError>;
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
        let mut pixels = pixels;
        premultiply_rgba_in_place_if_needed(&mut pixels);
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

    fn resolve(&self) -> Result<Arc<Vec<u8>>, MediaError> {
        Ok(Arc::clone(&self.pixels))
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
        let mut pixels = pixels;
        premultiply_rgba_in_place_if_needed(&mut pixels);
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

    fn resolve_frame(&self, frame: u32) -> Result<Arc<Vec<u8>>, MediaError> {
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
        Ok(Arc::clone(&self.pixels))
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

pub fn premultiply_rgba_in_place_if_needed(bytes: &mut [u8]) {
    if bytes.chunks_exact(4).all(|pixel| pixel[3] == 255) {
        return;
    }

    for pixel in bytes.chunks_exact_mut(4) {
        let alpha = pixel[3];
        if alpha == 255 {
            continue;
        }
        if alpha == 0 {
            pixel[0] = 0;
            pixel[1] = 0;
            pixel[2] = 0;
            continue;
        }

        let alpha = f32::from(alpha) / 255.0;
        pixel[0] = (f32::from(pixel[0]) * alpha).round().clamp(0.0, 255.0) as u8;
        pixel[1] = (f32::from(pixel[1]) * alpha).round().clamp(0.0, 255.0) as u8;
        pixel[2] = (f32::from(pixel[2]) * alpha).round().clamp(0.0, 255.0) as u8;
    }
}
