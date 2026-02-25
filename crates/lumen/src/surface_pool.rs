//! Skia surface pooling primitives for reusable render targets.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use crate::{
    backend::SurfaceFactory,
    error::{LumenError, RenderError},
};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SurfacePoolStats {
    pub total_acquires: u64,
    pub reused_acquires: u64,
    pub fresh_allocations: u64,
    pub fresh_allocation_bytes: u64,
    pub acquires_by_size: HashMap<(u32, u32), u64>,
}

impl SurfacePoolStats {
    pub fn delta_from(&self, baseline: &Self) -> Self {
        let mut acquires_by_size = HashMap::new();
        for (&key, &count) in &self.acquires_by_size {
            let baseline_count = baseline.acquires_by_size.get(&key).copied().unwrap_or(0);
            let delta = count.saturating_sub(baseline_count);
            if delta > 0 {
                acquires_by_size.insert(key, delta);
            }
        }

        Self {
            total_acquires: self.total_acquires.saturating_sub(baseline.total_acquires),
            reused_acquires: self
                .reused_acquires
                .saturating_sub(baseline.reused_acquires),
            fresh_allocations: self
                .fresh_allocations
                .saturating_sub(baseline.fresh_allocations),
            fresh_allocation_bytes: self
                .fresh_allocation_bytes
                .saturating_sub(baseline.fresh_allocation_bytes),
            acquires_by_size,
        }
    }
}

#[derive(Default)]
pub struct SurfacePool {
    available: Mutex<HashMap<(u32, u32), Vec<skia_safe::Surface>>>,
    /// CPU-only raster surfaces for pixel read-back operations.
    raster_available: Mutex<HashMap<(u32, u32), Vec<skia_safe::Surface>>>,
    stats: Mutex<SurfacePoolStats>,
    backend: Mutex<SurfaceFactory>,
}

impl SurfacePool {
    pub fn new() -> Self {
        Self {
            available: Mutex::new(HashMap::new()),
            raster_available: Mutex::new(HashMap::new()),
            stats: Mutex::new(SurfacePoolStats::default()),
            backend: Mutex::new(SurfaceFactory::new()),
        }
    }

    pub fn acquire(self: &Arc<Self>, width: u32, height: u32) -> Result<SurfaceRef, LumenError> {
        if let Ok(mut stats) = self.stats.lock() {
            stats.total_acquires = stats.total_acquires.saturating_add(1);
            let entry = stats.acquires_by_size.entry((width, height)).or_default();
            *entry = entry.saturating_add(1);
        }

        if let Ok(mut pool) = self.available.lock()
            && let Some(surface) = pool.get_mut(&(width, height)).and_then(std::vec::Vec::pop)
        {
            if let Ok(mut stats) = self.stats.lock() {
                stats.reused_acquires = stats.reused_acquires.saturating_add(1);
            }
            return Ok(SurfaceRef {
                surface: Some(surface),
                pool: Arc::clone(self),
                width,
                height,
            });
        }

        let surface = self.allocate_surface(width, height)?;
        if let Ok(mut stats) = self.stats.lock() {
            stats.fresh_allocations = stats.fresh_allocations.saturating_add(1);
            let bytes = u64::from(width)
                .saturating_mul(u64::from(height))
                .saturating_mul(4);
            stats.fresh_allocation_bytes = stats.fresh_allocation_bytes.saturating_add(bytes);
        }

        Ok(SurfaceRef {
            surface: Some(surface),
            pool: Arc::clone(self),
            width,
            height,
        })
    }

    /// Acquire a CPU-only raster surface suitable for pixel read-back.
    /// Unlike `acquire`, this always creates software-rasterized surfaces.
    pub fn acquire_raster(
        self: &Arc<Self>,
        width: u32,
        height: u32,
    ) -> Result<RasterSurfaceRef, LumenError> {
        if let Ok(mut stats) = self.stats.lock() {
            stats.total_acquires = stats.total_acquires.saturating_add(1);
            let entry = stats.acquires_by_size.entry((width, height)).or_default();
            *entry = entry.saturating_add(1);
        }

        if let Ok(mut pool) = self.raster_available.lock()
            && let Some(surface) = pool.get_mut(&(width, height)).and_then(std::vec::Vec::pop)
        {
            if let Ok(mut stats) = self.stats.lock() {
                stats.reused_acquires = stats.reused_acquires.saturating_add(1);
            }
            return Ok(RasterSurfaceRef {
                surface: Some(surface),
                pool: Arc::clone(self),
                width,
                height,
            });
        }

        let surface = skia_safe::surfaces::raster_n32_premul((width as i32, height as i32)).ok_or(
            LumenError::from(RenderError::SurfaceAllocation { width, height }),
        )?;
        if let Ok(mut stats) = self.stats.lock() {
            stats.fresh_allocations = stats.fresh_allocations.saturating_add(1);
            let bytes = u64::from(width)
                .saturating_mul(u64::from(height))
                .saturating_mul(4);
            stats.fresh_allocation_bytes = stats.fresh_allocation_bytes.saturating_add(bytes);
        }

        Ok(RasterSurfaceRef {
            surface: Some(surface),
            pool: Arc::clone(self),
            width,
            height,
        })
    }

    fn release_raster(&self, width: u32, height: u32, surface: skia_safe::Surface) {
        if let Ok(mut pool) = self.raster_available.lock() {
            pool.entry((width, height)).or_default().push(surface);
        }
    }

    fn allocate_surface(&self, width: u32, height: u32) -> Result<skia_safe::Surface, LumenError> {
        self.backend
            .lock()
            .ok()
            .and_then(|mut backend| backend.create_surface(width, height).ok())
            .ok_or(RenderError::SurfaceAllocation { width, height }.into())
    }

    pub fn stats(&self) -> SurfacePoolStats {
        self.stats
            .lock()
            .map_or_else(|_| SurfacePoolStats::default(), |stats| stats.clone())
    }

    fn release(&self, width: u32, height: u32, surface: skia_safe::Surface) {
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

/// A CPU-only raster surface reference for pixel read-back operations.
pub struct RasterSurfaceRef {
    surface: Option<skia_safe::Surface>,
    pool: Arc<SurfacePool>,
    width: u32,
    height: u32,
}

impl RasterSurfaceRef {
    pub fn surface_mut(&mut self) -> Option<&mut skia_safe::Surface> {
        self.surface.as_mut()
    }
}

impl Drop for RasterSurfaceRef {
    fn drop(&mut self) {
        if let Some(surface) = self.surface.take() {
            self.pool.release_raster(self.width, self.height, surface);
        }
    }
}
