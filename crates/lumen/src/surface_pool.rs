//! Skia surface pooling primitives for reusable render targets.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use crate::error::{LumenError, RenderError};

#[derive(Default)]
pub struct SurfacePool {
    available: Mutex<HashMap<(u32, u32), Vec<skia_safe::Surface>>>,
}

impl SurfacePool {
    pub fn new() -> Self {
        Self {
            available: Mutex::new(HashMap::new()),
        }
    }

    pub fn acquire(self: &Arc<Self>, width: u32, height: u32) -> Result<SurfaceRef, LumenError> {
        if let Ok(mut pool) = self.available.lock()
            && let Some(surface) = pool.get_mut(&(width, height)).and_then(std::vec::Vec::pop)
        {
            return Ok(SurfaceRef {
                surface: Some(surface),
                pool: Arc::clone(self),
                width,
                height,
            });
        }

        let surface = skia_safe::surfaces::raster_n32_premul((width as i32, height as i32))
            .ok_or(RenderError::SurfaceAllocation { width, height })?;

        Ok(SurfaceRef {
            surface: Some(surface),
            pool: Arc::clone(self),
            width,
            height,
        })
    }

    fn release(&self, width: u32, height: u32, mut surface: skia_safe::Surface) {
        surface.canvas().clear(skia_safe::Color::TRANSPARENT);
        if let Ok(mut pool) = self.available.lock() {
            pool.entry((width, height)).or_default().push(surface);
        }
    }
}

pub struct SurfaceRef {
    surface: Option<skia_safe::Surface>,
    pool: Arc<SurfacePool>,
    width: u32,
    height: u32,
}

impl SurfaceRef {
    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn surface(&self) -> Option<&skia_safe::Surface> {
        self.surface.as_ref()
    }

    pub fn surface_mut(&mut self) -> Option<&mut skia_safe::Surface> {
        self.surface.as_mut()
    }
}

impl std::fmt::Debug for SurfaceRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SurfaceRef")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

impl Drop for SurfaceRef {
    fn drop(&mut self) {
        if let Some(surface) = self.surface.take() {
            self.pool.release(self.width, self.height, surface);
        }
    }
}
