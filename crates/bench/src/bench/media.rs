use std::sync::Arc;

use lumen_engine::{
    error::MediaError,
    media::{
        CpuMediaFrame, ImageMetadata, ImageResolver, MediaFrame, MediaStore, VideoFrameResolver,
    },
};

#[derive(Debug, Default)]
pub enum BenchmarkMediaStore {
    #[default]
    Empty,
    Image(InMemoryImage),
}

impl MediaStore for BenchmarkMediaStore {
    fn get_image_resolver(&self, source: &str) -> Option<Box<dyn ImageResolver>> {
        match self {
            Self::Image(image) if image.id() == source => Some(Box::new(image.clone())),
            Self::Empty | Self::Image(_) => None,
        }
    }

    fn get_video_resolver(&self, _stream_id: &str) -> Option<Box<dyn VideoFrameResolver>> {
        None
    }
}

#[derive(Debug, Clone)]
pub struct InMemoryImage {
    id: String,
    frame: Arc<CpuMediaFrame>,
}

impl InMemoryImage {
    pub fn checkerboard(id: impl Into<String>, width: u32, height: u32) -> Self {
        let width = width.max(1);
        let height = height.max(1);
        let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
        for y in 0..height {
            for x in 0..width {
                let light = ((x / 16) + (y / 16)) % 2 == 0;
                let value = if light { 224 } else { 32 };
                rgba.extend_from_slice(&[value, 96, 255 - value, 255]);
            }
        }
        Self {
            id: id.into(),
            frame: Arc::new(CpuMediaFrame {
                rgba: Arc::new(rgba),
                width,
                height,
                row_bytes: width as usize * 4,
            }),
        }
    }
}

impl ImageResolver for InMemoryImage {
    fn id(&self) -> &str {
        &self.id
    }

    fn metadata(&self) -> ImageMetadata {
        ImageMetadata {
            width: self.frame.width,
            height: self.frame.height,
        }
    }

    fn frame(&self) -> Result<MediaFrame, MediaError> {
        Ok(MediaFrame::CpuRgba(Arc::clone(&self.frame)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkerboard_is_repeatable_and_tightly_packed() {
        let first = InMemoryImage::checkerboard("small", 32, 16);
        let second = InMemoryImage::checkerboard("small", 32, 16);
        let MediaFrame::CpuRgba(first_frame) = first.frame().unwrap() else {
            panic!("expected CPU RGBA image")
        };
        let MediaFrame::CpuRgba(second_frame) = second.frame().unwrap() else {
            panic!("expected CPU RGBA image")
        };

        assert_eq!(first.metadata().width, 32);
        assert_eq!(first.metadata().height, 16);
        assert_eq!(first_frame.row_bytes, 32 * 4);
        assert_eq!(first_frame.rgba.len(), 32 * 16 * 4);
        assert_eq!(first_frame.rgba, second_frame.rgba);
    }

    #[test]
    fn store_only_resolves_its_registered_image() {
        let store = BenchmarkMediaStore::Image(InMemoryImage::checkerboard("small", 2, 2));
        assert!(store.get_image_resolver("small").is_some());
        assert!(store.get_image_resolver("missing").is_none());
        assert!(store.get_video_resolver("small").is_none());
    }
}
