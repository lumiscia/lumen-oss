use std::sync::{Condvar, Mutex, PoisonError};

use lumen_ffmpeg::EncoderFamily;

/// NVIDIA's driver gives a process a fixed number of contexts across all of its Vulkan devices,
/// 63 on the cards measured. Past that `vkCreateDevice` fails, FFmpeg's device can segfault, and a
/// Blackwell card can hang, so renders wait for room and stop short of it.
const CONTEXTS: u32 = 56;
/// A device with only wgpu's render queue.
const RENDERER_CONTEXTS: u32 = 1;
/// A render device that also has the compute and video queues hardware decode needs. NVENC renders
/// count the same, whatever they decode with: running a Blackwell card out of memory with CUDA
/// contexts hung it, so they're held to the number measured to work.
const LARGE_RENDERER_CONTEXTS: u32 = 4;
/// The device FFmpeg opens for an encoder taking uploaded frames, which holds every queue it finds.
const UPLOAD_ENCODER_CONTEXTS: u32 = 5;

static BUDGET: DeviceBudget = DeviceBudget::new(CONTEXTS);

/// Waits until the devices a render is about to open fit in the driver's limit, and holds their
/// room until dropped.
pub(super) fn reserve_devices(
    encoder: EncoderFamily,
    hardware_decode: bool,
) -> DeviceReservation<'static> {
    let renderer = if hardware_decode || encoder == EncoderFamily::Nvenc {
        LARGE_RENDERER_CONTEXTS
    } else {
        RENDERER_CONTEXTS
    };
    let upload_encoder = if encoder == EncoderFamily::Vulkan {
        UPLOAD_ENCODER_CONTEXTS
    } else {
        0
    };
    BUDGET.reserve(renderer + upload_encoder)
}

struct DeviceBudget {
    capacity: u32,
    used: Mutex<u32>,
    freed: Condvar,
}

pub(super) struct DeviceReservation<'a> {
    budget: &'a DeviceBudget,
    contexts: u32,
}

impl DeviceBudget {
    const fn new(capacity: u32) -> Self {
        Self {
            capacity,
            used: Mutex::new(0),
            freed: Condvar::new(),
        }
    }

    fn reserve(&self, contexts: u32) -> DeviceReservation<'_> {
        let contexts = contexts.min(self.capacity);
        let mut used = self.used.lock().unwrap_or_else(PoisonError::into_inner);
        if *used + contexts > self.capacity {
            tracing::info!(
                contexts,
                used = *used,
                "render waiting for room for its GPU devices"
            );
        }
        while *used + contexts > self.capacity {
            used = self
                .freed
                .wait(used)
                .unwrap_or_else(PoisonError::into_inner);
        }
        *used += contexts;
        DeviceReservation {
            budget: self,
            contexts,
        }
    }
}

impl Drop for DeviceReservation<'_> {
    fn drop(&mut self) {
        *self
            .budget
            .used
            .lock()
            .unwrap_or_else(PoisonError::into_inner) -= self.contexts;
        self.budget.freed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::mpsc, time::Duration};

    use super::*;

    #[test]
    fn a_render_past_the_budget_waits_for_one_to_finish() {
        let budget = DeviceBudget::new(10);
        let first = budget.reserve(6);
        std::thread::scope(|scope| {
            let (started, waiting) = mpsc::channel();
            let budget = &budget;
            scope.spawn(move || {
                let _second = budget.reserve(6);
                started.send(()).unwrap();
            });
            assert!(waiting.recv_timeout(Duration::from_millis(100)).is_err());
            drop(first);
            waiting.recv_timeout(Duration::from_secs(10)).unwrap();
        });
        assert_eq!(*budget.used.lock().unwrap(), 0);
    }

    #[test]
    fn a_render_larger_than_the_budget_still_runs_alone() {
        let budget = DeviceBudget::new(10);
        let reservation = budget.reserve(12);
        assert_eq!(*budget.used.lock().unwrap(), 10);
        drop(reservation);
    }
}
