//! Media resolver traits and test doubles for image/video sources.

#[cfg(test)]
use std::{collections::HashMap, sync::Arc};

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

#[cfg(test)]
#[derive(Clone)]
pub struct MockImageResolver {
	id: String,
	width: u32,
	height: u32,
	pixels: Arc<Vec<u8>>,
}

#[cfg(test)]
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

#[cfg(test)]
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

#[cfg(test)]
#[derive(Default)]
pub struct MockMediaStore {
	images: HashMap<String, MockImageResolver>,
}

#[cfg(test)]
impl MockMediaStore {
	pub fn insert_image(&mut self, resolver: MockImageResolver) {
		self.images.insert(resolver.id().to_string(), resolver);
	}
}

#[cfg(test)]
impl MediaStore for MockMediaStore {
	fn get_image_resolver(&self, source: &str) -> Option<Box<dyn ImageResolver>> {
		self.images
			.get(source)
			.cloned()
			.map(|resolver| Box::new(resolver) as Box<dyn ImageResolver>)
	}

	fn get_video_resolver(&self, _source: &str) -> Option<Box<dyn VideoFrameResolver>> {
		None
	}
}
