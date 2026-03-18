use std::sync::{Mutex, PoisonError};

use crate::{backend::SurfaceFactory, render::surface::SurfaceLease};

use super::{SurfaceBuckets, SurfaceKind, SurfacePool, SurfacePoolStats, allocate_raster_surface};

#[derive(Default)]
pub struct SingleThreadedSurfacePool {
    buckets: Mutex<SurfaceBuckets>,
    stats: Mutex<SurfacePoolStats>,
    backend: Mutex<SurfaceFactory>,
}

impl SingleThreadedSurfacePool {
    pub fn new() -> Self {
        Self::default()
    }
}

unsafe impl Send for SingleThreadedSurfacePool {}
unsafe impl Sync for SingleThreadedSurfacePool {}

impl SurfacePool for SingleThreadedSurfacePool {
    fn acquire(&self, width: u32, height: u32) -> crate::Result<SurfaceLease<'_>> {
        lock(&self.stats).record_acquire(width, height);

        let surface = if let Some(surface) = lock(&self.buckets).take_render((width, height)) {
            lock(&self.stats).record_reuse();
            surface
        } else {
            let surface = lock(&self.backend).create_surface(width, height)?;
            lock(&self.stats).record_fresh_allocation(width, height);
            surface
        };

        Ok(SurfaceLease::new(surface, self, SurfaceKind::Render))
    }

    fn acquire_raster(&self, width: u32, height: u32) -> crate::Result<SurfaceLease<'_>> {
        lock(&self.stats).record_acquire(width, height);

        let surface = if let Some(surface) = lock(&self.buckets).take_raster((width, height)) {
            lock(&self.stats).record_reuse();
            surface
        } else {
            let surface = allocate_raster_surface(width, height)?;
            lock(&self.stats).record_fresh_allocation(width, height);
            surface
        };

        Ok(SurfaceLease::new(surface, self, SurfaceKind::Raster))
    }

    fn stats(&self) -> SurfacePoolStats {
        lock(&self.stats).clone()
    }

    fn release(&self, kind: SurfaceKind, width: u32, height: u32, surface: skia_safe::Surface) {
        let mut buckets = lock(&self.buckets);
        match kind {
            SurfaceKind::Render => buckets.store_render((width, height), surface),
            SurfaceKind::Raster => buckets.store_raster((width, height), surface),
        }
    }
}

impl std::fmt::Debug for SingleThreadedSurfacePool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SingleThreadedSurfacePool")
            .field("stats", &lock(&self.stats))
            .finish_non_exhaustive()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
