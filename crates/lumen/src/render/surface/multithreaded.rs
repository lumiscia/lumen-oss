use parking_lot::Mutex;

use crate::{backend::SurfaceFactory, render::surface::SurfaceLease};

use super::{SurfaceBuckets, SurfaceKind, SurfacePool, SurfacePoolStats, allocate_raster_surface};

#[derive(Default)]
pub struct MultiThreadedSurfacePool {
    buckets: Mutex<SurfaceBuckets>,
    stats: Mutex<SurfacePoolStats>,
    backend: Mutex<SurfaceFactory>,
}

impl MultiThreadedSurfacePool {
    pub fn new() -> Self {
        Self::default()
    }
}

unsafe impl Send for MultiThreadedSurfacePool {}
unsafe impl Sync for MultiThreadedSurfacePool {}

impl SurfacePool for MultiThreadedSurfacePool {
    fn acquire(&self, width: u32, height: u32) -> crate::Result<SurfaceLease<'_>> {
        self.stats.lock().record_acquire(width, height);

        let surface = if let Some(surface) = self.buckets.lock().take_render((width, height)) {
            self.stats.lock().record_reuse();
            surface
        } else {
            let surface = self.backend.lock().create_surface(width, height)?;
            self.stats.lock().record_fresh_allocation(width, height);
            surface
        };

        Ok(SurfaceLease::new(surface, self, SurfaceKind::Render))
    }

    fn acquire_raster(&self, width: u32, height: u32) -> crate::Result<SurfaceLease<'_>> {
        self.stats.lock().record_acquire(width, height);

        let surface = if let Some(surface) = self.buckets.lock().take_raster((width, height)) {
            self.stats.lock().record_reuse();
            surface
        } else {
            let surface = allocate_raster_surface(width, height)?;
            self.stats.lock().record_fresh_allocation(width, height);
            surface
        };

        Ok(SurfaceLease::new(surface, self, SurfaceKind::Raster))
    }

    fn stats(&self) -> SurfacePoolStats {
        self.stats.lock().clone()
    }

    fn release(&self, kind: SurfaceKind, width: u32, height: u32, surface: skia_safe::Surface) {
        let mut buckets = self.buckets.lock();
        match kind {
            SurfaceKind::Render => buckets.store_render((width, height), surface),
            SurfaceKind::Raster => buckets.store_raster((width, height), surface),
        }
    }
}

impl std::fmt::Debug for MultiThreadedSurfacePool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MultiThreadedSurfacePool")
            .field("stats", &self.stats.lock())
            .finish_non_exhaustive()
    }
}
