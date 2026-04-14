//! Skia render-target allocation for the active backend.

use std::{
    collections::HashMap,
    fmt::Debug,
    sync::{Mutex, PoisonError},
};

use crate::{backend::SurfaceFactory, error::RenderError};

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

pub trait SurfacePool: std::fmt::Debug {
    fn with_surface<T>(
        &self,
        width: u32,
        height: u32,
        f: impl FnOnce(&mut skia_safe::Surface) -> crate::Result<T>,
    ) -> crate::Result<T>;

    fn stats(&self) -> SurfacePoolStats;

    fn flush(&self);
}

#[derive(Default)]
pub struct DefaultSurfacePool {
    stats: Mutex<SurfacePoolStats>,
    slots: Mutex<ScratchSurfaceRing>,
    backend: Mutex<SurfaceFactory>,
}

impl DefaultSurfacePool {
    pub fn new() -> Self {
        Self::default()
    }
}

fn allocation_bytes(width: u32, height: u32) -> u64 {
    u64::from(width)
        .saturating_mul(u64::from(height))
        .saturating_mul(4)
}

#[derive(Default)]
struct ScratchSurfaceRing {
    next_slot: usize,
    slots: [ScratchSurfaceSlot; 2],
}

#[derive(Default)]
struct ScratchSurfaceSlot {
    in_use: bool,
    surface: Option<skia_safe::Surface>,
}

impl SurfacePool for DefaultSurfacePool {
    fn with_surface<T>(
        &self,
        width: u32,
        height: u32,
        f: impl FnOnce(&mut skia_safe::Surface) -> crate::Result<T>,
    ) -> crate::Result<T> {
        let selection = {
            let mut stats = lock(&self.stats);
            stats.record_acquire(width, height);

            let mut ring = lock(&self.slots);
            let start = ring.next_slot;
            let slot_count = ring.slots.len();
            let mut selection = None;

            for offset in 0..slot_count {
                let index = (start + offset) % slot_count;
                if ring.slots[index].in_use {
                    continue;
                }

                ring.next_slot = (index + 1) % slot_count;
                let taken_surface = {
                    let slot = &mut ring.slots[index];
                    slot.in_use = true;
                    slot.surface.take()
                };

                let reusable_surface = if let Some(surface) = taken_surface {
                    if surface.width() as u32 == width && surface.height() as u32 == height {
                        stats.record_reuse();
                        Some(surface)
                    } else {
                        stats.record_fresh_allocation(width, height);
                        None
                    }
                } else {
                    stats.record_fresh_allocation(width, height);
                    None
                };

                selection = Some((index, reusable_surface));
                break;
            }

            selection
        };

        let Some((index, maybe_surface)) = selection else {
            return Err(RenderError::ScratchSurfaceUnavailable.into());
        };

        let mut surface = match maybe_surface {
            Some(surface) => surface,
            None => lock(&self.backend).create_surface(width, height)?,
        };

        let result = f(&mut surface);
        let mut ring = lock(&self.slots);
        let slot = &mut ring.slots[index];
        slot.in_use = false;
        slot.surface = Some(surface);
        result
    }

    fn stats(&self) -> SurfacePoolStats {
        lock(&self.stats).clone()
    }

    fn flush(&self) {
        lock(&self.backend).flush();
    }
}

impl std::fmt::Debug for DefaultSurfacePool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DefaultSurfacePool")
            .field("stats", &lock(&self.stats))
            .finish_non_exhaustive()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
