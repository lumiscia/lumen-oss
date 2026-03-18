//! Skia surface pooling primitives for reusable render targets.

use std::{collections::HashMap, fmt::Debug, rc::Rc};

use crate::error::{LumenError, RenderError};

#[cfg(not(feature = "threading"))]
mod single_threaded;

#[cfg(feature = "threading")]
mod multithreaded;

#[cfg(not(feature = "threading"))]
pub use single_threaded::SingleThreadedSurfacePool as DefaultSurfacePool;

#[cfg(feature = "threading")]
pub use multithreaded::MultiThreadedSurfacePool as DefaultSurfacePool;

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

    fn record_acquire(&mut self, width: u32, height: u32) {
        self.total_acquires = self.total_acquires.saturating_add(1);
        let entry = self.acquires_by_size.entry((width, height)).or_default();
        *entry = entry.saturating_add(1);
    }

    fn record_reuse(&mut self) {
        self.reused_acquires = self.reused_acquires.saturating_add(1);
    }

    fn record_fresh_allocation(&mut self, width: u32, height: u32) {
        self.fresh_allocations = self.fresh_allocations.saturating_add(1);
        self.fresh_allocation_bytes = self
            .fresh_allocation_bytes
            .saturating_add(allocation_bytes(width, height));
    }
}

pub trait SurfacePool: Send + Sync + std::fmt::Debug {
    fn acquire(&self, width: u32, height: u32) -> crate::Result<SurfaceLease<'_>>;

    fn acquire_raster(&self, width: u32, height: u32) -> crate::Result<SurfaceLease<'_>>;

    fn stats(&self) -> SurfacePoolStats;

    #[doc(hidden)]
    fn release(&self, kind: SurfaceKind, width: u32, height: u32, surface: skia_safe::Surface);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[doc(hidden)]
pub enum SurfaceKind {
    Render,
    Raster,
}

type SurfaceSize = (u32, u32);

#[derive(Default)]
struct SurfaceBuckets {
    render: HashMap<SurfaceSize, Vec<skia_safe::Surface>>,
    raster: HashMap<SurfaceSize, Vec<skia_safe::Surface>>,
}

impl SurfaceBuckets {
    fn take_render(&mut self, size: SurfaceSize) -> Option<skia_safe::Surface> {
        self.render.get_mut(&size).and_then(Vec::pop)
    }

    fn take_raster(&mut self, size: SurfaceSize) -> Option<skia_safe::Surface> {
        self.raster.get_mut(&size).and_then(Vec::pop)
    }

    fn store_render(&mut self, size: SurfaceSize, surface: skia_safe::Surface) {
        self.render.entry(size).or_default().push(surface);
    }

    fn store_raster(&mut self, size: SurfaceSize, surface: skia_safe::Surface) {
        self.raster.entry(size).or_default().push(surface);
    }
}

fn allocation_bytes(width: u32, height: u32) -> u64 {
    u64::from(width)
        .saturating_mul(u64::from(height))
        .saturating_mul(4)
}

fn allocate_raster_surface(width: u32, height: u32) -> crate::Result<skia_safe::Surface> {
    skia_safe::surfaces::raster_n32_premul((width as i32, height as i32))
        .ok_or_else(|| LumenError::from(RenderError::SurfaceAllocation { width, height }))
}

pub struct SurfaceLease<'pool> {
    surface: Option<skia_safe::Surface>,
    pool: &'pool dyn SurfacePool,
    kind: SurfaceKind,
}

impl<'pool> SurfaceLease<'pool> {
    fn new(surface: skia_safe::Surface, pool: &'pool dyn SurfacePool, kind: SurfaceKind) -> Self {
        Self {
            surface: Option::Some(surface),
            pool,
            kind,
        }
    }

    pub fn width(&self) -> u32 {
        self.surface().width() as u32
    }

    pub fn height(&self) -> u32 {
        self.surface().height() as u32
    }

    pub fn surface(&self) -> &skia_safe::Surface {
        self.surface.as_ref().unwrap()
    }

    pub fn surface_mut(&mut self) -> &mut skia_safe::Surface {
        self.surface.as_mut().unwrap()
    }

    pub fn take(mut self) -> Result<OwnedSurface, LumenError> {
        let Some(surface) = self.surface.take() else {
            return Err(RenderError::SurfaceLeaseReleased.into());
        };
        Ok(OwnedSurface {
            surface,
            kind: self.kind,
        })
    }

    pub fn take_rc(self: Rc<Self>) -> Result<OwnedSurface, LumenError> {
        Rc::try_unwrap(self)
            .map_err(|_| LumenError::from(RenderError::SharedSurfaceLease))?
            .take()
    }
}

impl Drop for SurfaceLease<'_> {
    fn drop(&mut self) {
        if let Some(surface) = self.surface.take() {
            let width = surface.width() as u32;
            let height = surface.height() as u32;
            self.pool.release(self.kind, width, height, surface);
        }
    }
}

impl std::fmt::Debug for SurfaceLease<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PoolLease")
            .field("width", &self.width())
            .field("height", &self.height())
            .finish_non_exhaustive()
    }
}

// Owned surface that we can manage ourselves
#[derive(Debug)]
pub struct OwnedSurface {
    surface: skia_safe::Surface,
    kind: SurfaceKind,
}

impl OwnedSurface {
    pub fn width(&self) -> u32 {
        self.surface.width() as u32
    }

    pub fn height(&self) -> u32 {
        self.surface.height() as u32
    }

    pub fn surface(&self) -> &skia_safe::Surface {
        &self.surface
    }

    pub fn surface_mut(&mut self) -> &mut skia_safe::Surface {
        &mut self.surface
    }
}

impl OwnedSurface {
    pub fn kind(&self) -> SurfaceKind {
        self.kind
    }

    pub fn release_to(self, pool: &dyn SurfacePool) {
        let width = self.surface.width() as u32;
        let height = self.surface.height() as u32;
        pool.release(self.kind, width, height, self.surface)
    }
}
