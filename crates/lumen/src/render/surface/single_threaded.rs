use std::cell::RefCell;

use crate::{backend::SurfaceFactory, error::LumenError};

use super::{
    RasterSurfaceRef, SharedPointer, SurfaceBuckets, SurfaceKind, SurfacePool, SurfacePoolStats,
    SurfaceRef, allocate_raster_surface, allocate_surface,
};

#[derive(Default)]
pub struct SingleThreadedSurfacePool {
    buckets: RefCell<SurfaceBuckets>,
    stats: RefCell<SurfacePoolStats>,
    backend: RefCell<SurfaceFactory>,
}

impl SingleThreadedSurfacePool {
    pub fn new() -> Self {
        Self::default()
    }
}

impl SurfacePool for SingleThreadedSurfacePool {
    fn acquire(self: SharedPointer<Self>, width: u32, height: u32) -> crate::Result<SurfaceRef> {
        self.stats.borrow_mut().record_acquire(width, height);

        let surface = if let Some(surface) = self.buckets.borrow_mut().take_render((width, height))
        {
            self.stats.borrow_mut().record_reuse();
            surface
        } else {
            let surface = allocate_surface(&mut self.backend.borrow_mut(), width, height)?;
            self.stats
                .borrow_mut()
                .record_fresh_allocation(width, height);
            surface
        };

        Ok(SurfaceRef::new(surface, self, width, height))
    }

    fn acquire_raster(
        self: SharedPointer<Self>,
        width: u32,
        height: u32,
    ) -> crate::Result<RasterSurfaceRef> {
        self.stats.borrow_mut().record_acquire(width, height);

        let surface = if let Some(surface) = self.buckets.borrow_mut().take_raster((width, height))
        {
            self.stats.borrow_mut().record_reuse();
            surface
        } else {
            let surface = allocate_raster_surface(width, height)?;
            self.stats
                .borrow_mut()
                .record_fresh_allocation(width, height);
            surface
        };

        Ok(RasterSurfaceRef::new(surface, self))
    }

    fn stats(&self) -> SurfacePoolStats {
        self.stats.borrow().clone()
    }

    fn release(&self, kind: SurfaceKind, width: u32, height: u32, surface: skia_safe::Surface) {
        let mut buckets = self.buckets.borrow_mut();
        match kind {
            SurfaceKind::Render => buckets.store_render((width, height), surface),
            SurfaceKind::Raster => buckets.store_raster((width, height), surface),
        }
    }
}

impl std::fmt::Debug for SingleThreadedSurfacePool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SingleThreadedSurfacePool")
            .field("stats", &self.stats.borrow())
            .finish_non_exhaustive()
    }
}
