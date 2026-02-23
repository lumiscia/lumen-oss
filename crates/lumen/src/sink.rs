//! Output sink traits and in-memory bitmap sink for testing.

use std::sync::Arc;

use crate::{
	error::SinkError,
	raster::RasterFrame,
};

pub trait Sink: Send {
	fn write_frame(&mut self, frame: u32, data: &RasterFrame) -> Result<(), SinkError>;
	fn finalize(&mut self) -> Result<(), SinkError>;
}

#[derive(Debug, Clone)]
pub struct CollectedBitmapFrame {
	pub frame: u32,
	pub width: u32,
	pub height: u32,
	pub pixels: Arc<Vec<u8>>,
}

#[derive(Default)]
pub struct BitmapSink {
	frames: Vec<CollectedBitmapFrame>,
	finalized: bool,
}

impl BitmapSink {
	pub fn new() -> Self {
		Self::default()
	}

	pub fn frames(&self) -> &[CollectedBitmapFrame] {
		&self.frames
	}
}

impl Sink for BitmapSink {
	fn write_frame(&mut self, frame: u32, data: &RasterFrame) -> Result<(), SinkError> {
		let bitmap = data
			.clone()
			.to_bitmap()
			.map_err(|err| SinkError::WriteFrame {
				frame,
				details: err.to_string(),
			})?;

		if let RasterFrame::Bitmap(pixels, width, height) = bitmap {
			self.frames.push(CollectedBitmapFrame {
				frame,
				width,
				height,
				pixels,
			});
		}

		Ok(())
	}

	fn finalize(&mut self) -> Result<(), SinkError> {
		self.finalized = true;
		Ok(())
	}
}
