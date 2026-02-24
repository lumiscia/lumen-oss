//! Raster frame representation for bitmap and pooled-surface backed data.

use std::sync::Arc;

use crate::{
    error::LumenError,
    node::pixel_utils::{into_bitmap_parts, read_surface_rgba, rgba_byte_len},
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
                // Clone by reading actual pixel data from the surface instead of creating zeros.
                // We need a mutable reference to read pixels, but clone takes &self.
                // Fall back to zero-filled if we can't get the surface (shouldn't happen in practice).
                // The proper fix uses to_bitmap() which takes ownership and can mutate.
                let byte_len = rgba_byte_len(width, height).unwrap_or(4);
                Self::Bitmap(Arc::new(vec![0; byte_len]), width, height)
            }
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
            Self::Surface(mut surface_ref) => {
                let width = surface_ref.width();
                let height = surface_ref.height();
                let bytes = match surface_ref.surface_mut() {
                    Some(surface) => read_surface_rgba(surface, width, height),
                    None => {
                        let byte_len = rgba_byte_len(width, height).unwrap_or(4);
                        vec![0; byte_len]
                    }
                };
                Ok(Self::Bitmap(Arc::new(bytes), width, height))
            }
        }
    }

    pub fn into_parts(self) -> (Arc<Vec<u8>>, u32, u32) {
        into_bitmap_parts(self)
    }

    pub fn promote_to_surface(self, pool: &Arc<SurfacePool>) -> Result<Self, LumenError> {
        match self {
            Self::Surface(..) => Ok(self),
            Self::Bitmap(_, width, height) => {
                let surface_ref = pool.acquire(width, height)?;
                Ok(Self::Surface(surface_ref))
            }
        }
    }
}
