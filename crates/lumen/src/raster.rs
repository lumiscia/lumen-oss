//! Raster frame representation for bitmap and pooled-surface backed data.

use std::sync::Arc;

use crate::{
	error::LumenError,
	surface_pool::{SurfacePool, SurfaceRef},
};

#[derive(Debug)]
pub enum RasterFrame {
	Bitmap(Arc<Vec<u8>>, u32, u32),
	Surface(SurfaceRef),
}

impl Clone for RasterFrame {
	fn clone(&self) -> Self {
		match self {
			Self::Bitmap(bytes, width, height) => Self::Bitmap(Arc::clone(bytes), *width, *height),
			Self::Surface(surface_ref) => {
				let width = surface_ref.width();
				let height = surface_ref.height();
				Self::Bitmap(Arc::new(vec![0; (width * height * 4) as usize]), width, height)
			},
		}
	}
}

impl RasterFrame {
	pub fn dimensions(&self) -> (u32, u32) {
		match self {
			Self::Bitmap(_, width, height) => (*width, *height),
			Self::Surface(surface) => (surface.width(), surface.height()),
		}
	}

	pub fn as_bitmap_bytes(&self) -> Option<&[u8]> {
		match self {
			Self::Bitmap(bytes, ..) => Some(bytes.as_slice()),
			Self::Surface(_) => None,
		}
	}

	pub fn to_bitmap(self) -> Result<Self, LumenError> {
		match self {
			Self::Bitmap(..) => Ok(self),
			Self::Surface(surface_ref) => {
				let (width, height) = (surface_ref.width(), surface_ref.height());
				let bytes = vec![0; (width * height * 4) as usize];
				Ok(Self::Bitmap(Arc::new(bytes), width, height))
			},
		}
	}

	pub fn promote_to_surface(self, pool: &Arc<SurfacePool>) -> Result<Self, LumenError> {
		match self {
			Self::Surface(..) => Ok(self),
			Self::Bitmap(_, width, height) => {
				let surface_ref = pool.acquire(width, height)?;
				Ok(Self::Surface(surface_ref))
			},
		}
	}
}
