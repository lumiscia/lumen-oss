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

const MAX_CACHED_RENDER_BYTES: u64 = 256 * 1024 * 1024;
const MAX_CACHED_RASTER_BYTES: u64 = 256 * 1024 * 1024;
const MAX_SURFACES_PER_SIZE: usize = 2;

type SurfaceSize = (u32, u32);

#[derive(Default)]
struct SurfaceBuckets {
    render: HashMap<SurfaceSize, Vec<skia_safe::Surface>>,
    raster: HashMap<SurfaceSize, Vec<skia_safe::Surface>>,
    render_cached_bytes: u64,
    raster_cached_bytes: u64,
}

impl SurfaceBuckets {
    fn take_render(&mut self, size: SurfaceSize) -> Option<skia_safe::Surface> {
        take_surface_from_bucket(&mut self.render, &mut self.render_cached_bytes, size)
    }

    fn take_raster(&mut self, size: SurfaceSize) -> Option<skia_safe::Surface> {
        take_surface_from_bucket(&mut self.raster, &mut self.raster_cached_bytes, size)
    }

    fn store_render(&mut self, size: SurfaceSize, surface: skia_safe::Surface) {
        store_surface_with_budget(
            &mut self.render,
            &mut self.render_cached_bytes,
            size,
            surface,
            MAX_CACHED_RENDER_BYTES,
        )
    }

    fn store_raster(&mut self, size: SurfaceSize, surface: skia_safe::Surface) {
        store_surface_with_budget(
            &mut self.raster,
            &mut self.raster_cached_bytes,
            size,
            surface,
            MAX_CACHED_RASTER_BYTES,
        )
    }
}

fn take_surface_from_bucket(
    buckets: &mut HashMap<SurfaceSize, Vec<skia_safe::Surface>>,
    cached_bytes: &mut u64,
    size: SurfaceSize,
) -> Option<skia_safe::Surface> {
    let (surface, should_remove_entry) = {
        let bucket = buckets.get_mut(&size)?;
        let surface = bucket.pop()?;
        let should_remove_entry = bucket.is_empty();
        (surface, should_remove_entry)
    };

    if should_remove_entry {
        buckets.remove(&size);
    }

    *cached_bytes = cached_bytes.saturating_sub(allocation_bytes(size.0, size.1));
    Some(surface)
}

fn store_surface_with_budget(
    buckets: &mut HashMap<SurfaceSize, Vec<skia_safe::Surface>>,
    cached_bytes: &mut u64,
    size: SurfaceSize,
    surface: skia_safe::Surface,
    max_bytes: u64,
) {
    let surface_bytes = allocation_bytes(size.0, size.1);
    if surface_bytes > max_bytes {
        return;
    }

    if buckets
        .get(&size)
        .is_some_and(|existing| existing.len() >= MAX_SURFACES_PER_SIZE)
    {
        return;
    }

    while cached_bytes.saturating_add(surface_bytes) > max_bytes {
        if !evict_largest_surface(buckets, cached_bytes) {
            return;
        }
    }

    buckets.entry(size).or_default().push(surface);
    *cached_bytes = cached_bytes.saturating_add(surface_bytes);
}

fn evict_largest_surface(
    buckets: &mut HashMap<SurfaceSize, Vec<skia_safe::Surface>>,
    cached_bytes: &mut u64,
) -> bool {
    let candidate = buckets
        .iter()
        .filter_map(|(&size, surfaces)| (!surfaces.is_empty()).then_some(size))
        .max_by_key(|(width, height)| allocation_bytes(*width, *height));

    let Some(size) = candidate else {
        return false;
    };

    let should_remove_entry = {
        let Some(bucket) = buckets.get_mut(&size) else {
            return false;
        };
        if bucket.pop().is_none() {
            return false;
        }
        bucket.is_empty()
    };

    if should_remove_entry {
        buckets.remove(&size);
    }

    *cached_bytes = cached_bytes.saturating_sub(allocation_bytes(size.0, size.1));
    true
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
