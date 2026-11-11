use std::{
    collections::VecDeque,
    sync::{Condvar, Mutex, PoisonError},
    time::Duration,
};

use lumen_ffmpeg::EncoderFamily;

use super::{CancelToken, ServerRenderError};

/// NVIDIA's driver gives a process a fixed number of contexts across all of its Vulkan devices,
/// 63 on the cards measured. Past that `vkCreateDevice` fails, FFmpeg's device can segfault, and a
/// Blackwell card can hang, so renders wait for room and stop short of it.
const CONTEXTS: u32 = 56;
/// A device with only wgpu's render queue.
const RENDERER_CONTEXTS: u32 = 1;
/// A render device that also has the compute and video queues FFmpeg uses it through. NVENC renders
/// count the same, whatever they decode with: running a Blackwell card out of memory with CUDA
/// contexts hung it, so they're held to the number measured to work.
const LARGE_RENDERER_CONTEXTS: u32 = 4;
/// The device FFmpeg opens for an encoder taking uploaded frames, which holds every queue it finds.
const UPLOAD_ENCODER_CONTEXTS: u32 = 5;

/// How often a render waiting for room looks at its cancel token.
const CANCEL_POLL: Duration = Duration::from_millis(50);

static BUDGET: DeviceBudget = DeviceBudget::new(CONTEXTS);

/// Waits until the devices a render is about to open fit in the driver's limit, and holds their
/// room until dropped. Cancelling `cancel` ends the wait with the cancelled error.
pub(super) fn reserve_devices(
    encoder: EncoderFamily,
    hardware_decode: bool,
    cancel: &CancelToken,
) -> Result<DeviceReservation<'static>, ServerRenderError> {
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
    BUDGET.reserve(renderer + upload_encoder, cancel)
}

/// Waits for room for one render device whose video queues the encoder and decoders share.
#[cfg(all(target_os = "linux", feature = "vulkan"))]
pub(super) fn reserve_shared_video_device(
    cancel: &CancelToken,
) -> Result<DeviceReservation<'static>, ServerRenderError> {
    BUDGET.reserve(LARGE_RENDERER_CONTEXTS, cancel)
}

struct DeviceBudget {
    capacity: u32,
    state: Mutex<BudgetState>,
    freed: Condvar,
}

/// Renders start in the order they asked, so a stream of small ones can't hold back a large one.
struct BudgetState {
    used: u32,
    next_ticket: u64,
    /// The tickets still waiting, in the order they were taken.
    waiting: VecDeque<u64>,
}

pub(super) struct DeviceReservation<'a> {
    budget: &'a DeviceBudget,
    contexts: u32,
}

impl DeviceBudget {
    const fn new(capacity: u32) -> Self {
        Self {
            capacity,
            state: Mutex::new(BudgetState {
                used: 0,
                next_ticket: 0,
                waiting: VecDeque::new(),
            }),
            freed: Condvar::new(),
        }
    }

    fn reserve(
        &self,
        contexts: u32,
        cancel: &CancelToken,
    ) -> Result<DeviceReservation<'_>, ServerRenderError> {
        let contexts = contexts.min(self.capacity);
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if !state.waiting.is_empty() || state.used + contexts > self.capacity {
            tracing::info!(
                contexts,
                used = state.used,
                "render waiting for room for its GPU devices"
            );
        }
        // Nothing that can panic runs from here until the ticket is served or given up, as a
        // ticket left in the queue would hold up every render behind it.
        let ticket = state.next_ticket;
        state.next_ticket += 1;
        state.waiting.push_back(ticket);
        loop {
            if cancel.is_cancelled() {
                state.waiting.retain(|waiting| *waiting != ticket);
                self.freed.notify_all();
                return Err(ServerRenderError::cancelled());
            }
            if state.waiting.front() == Some(&ticket) && state.used + contexts <= self.capacity {
                break;
            }
            state = self
                .freed
                .wait_timeout(state, CANCEL_POLL)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        state.used += contexts;
        state.waiting.pop_front();
        self.freed.notify_all();
        Ok(DeviceReservation {
            budget: self,
            contexts,
        })
    }
}

impl Drop for DeviceReservation<'_> {
    fn drop(&mut self) {
        self.budget
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .used -= self.contexts;
        self.budget.freed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::mpsc, time::Duration};

    use super::*;
    use crate::render::ErrorCode;

    fn reserve(budget: &DeviceBudget, contexts: u32) -> DeviceReservation<'_> {
        budget.reserve(contexts, &CancelToken::new()).unwrap()
    }

    /// Returns once `waiting` renders are queued behind the ones holding room.
    fn wait_for_waiting(budget: &DeviceBudget, waiting: usize) {
        while budget.state.lock().unwrap().waiting.len() < waiting {
            std::thread::yield_now();
        }
    }

    #[test]
    fn a_render_past_the_budget_waits_for_one_to_finish() {
        let budget = DeviceBudget::new(10);
        let first = reserve(&budget, 6);
        std::thread::scope(|scope| {
            let (started, waiting) = mpsc::channel();
            let budget = &budget;
            scope.spawn(move || {
                let _second = reserve(budget, 6);
                started.send(()).unwrap();
            });
            assert!(waiting.recv_timeout(Duration::from_millis(100)).is_err());
            drop(first);
            waiting.recv_timeout(Duration::from_secs(10)).unwrap();
        });
        assert_eq!(budget.state.lock().unwrap().used, 0);
    }

    #[test]
    fn a_render_larger_than_the_budget_still_runs_alone() {
        let budget = DeviceBudget::new(10);
        let reservation = reserve(&budget, 12);
        assert_eq!(budget.state.lock().unwrap().used, 10);
        drop(reservation);
    }

    #[test]
    fn a_small_render_does_not_pass_a_large_one_waiting() {
        let budget = DeviceBudget::new(10);
        let first = reserve(&budget, 6);
        std::thread::scope(|scope| {
            let (started, order) = mpsc::channel();
            let budget = &budget;
            let large_started = started.clone();
            scope.spawn(move || {
                let _large = reserve(budget, 8);
                large_started.send("large").unwrap();
            });
            wait_for_waiting(budget, 1);
            scope.spawn(move || {
                let _small = reserve(budget, 4);
                started.send("small").unwrap();
            });
            assert!(order.recv_timeout(Duration::from_millis(100)).is_err());
            drop(first);
            assert_eq!(order.recv_timeout(Duration::from_secs(10)), Ok("large"));
            assert_eq!(order.recv_timeout(Duration::from_secs(10)), Ok("small"));
        });
    }

    #[test]
    fn a_cancelled_wait_ends_and_lets_the_renders_behind_it_start() {
        let budget = DeviceBudget::new(10);
        let first = reserve(&budget, 6);
        let cancel = CancelToken::new();
        std::thread::scope(|scope| {
            let (done, results) = mpsc::channel();
            let budget = &budget;
            let large_done = done.clone();
            let large_cancel = cancel.clone();
            scope.spawn(move || {
                let result = budget.reserve(8, &large_cancel).map(|_| ());
                large_done.send(("large", result)).unwrap();
            });
            wait_for_waiting(budget, 1);
            scope.spawn(move || {
                let result = budget.reserve(4, &CancelToken::new()).map(|reserved| {
                    assert_eq!(budget.state.lock().unwrap().used, 10);
                    drop(reserved);
                });
                done.send(("small", result)).unwrap();
            });
            wait_for_waiting(budget, 2);

            cancel.cancel();

            // The two end in either order: the cancelled wait wakes the other before it reports.
            for _ in 0..2 {
                match results.recv_timeout(Duration::from_secs(10)).unwrap() {
                    ("large", result) => {
                        assert_eq!(result.unwrap_err().code(), ErrorCode::Cancelled);
                    }
                    (_, result) => result.unwrap(),
                }
            }
        });
        drop(first);
        let state = budget.state.lock().unwrap();
        assert_eq!(state.used, 0);
        assert!(state.waiting.is_empty());
    }

    #[test]
    fn a_cancelled_render_takes_no_room_that_is_free() {
        let budget = DeviceBudget::new(10);
        let cancel = CancelToken::new();
        cancel.cancel();
        let error = budget.reserve(4, &cancel).map(|_| ()).unwrap_err();
        assert_eq!(error.code(), ErrorCode::Cancelled);
        let state = budget.state.lock().unwrap();
        assert_eq!(state.used, 0);
        assert!(state.waiting.is_empty());
    }

    #[test]
    fn a_cancelled_wait_in_the_middle_of_the_queue_leaves_the_others_in_order() {
        let budget = DeviceBudget::new(10);
        let first = reserve(&budget, 10);
        let cancel = CancelToken::new();
        std::thread::scope(|scope| {
            let (done, order) = mpsc::channel();
            let budget = &budget;
            let spawn = |name: &'static str, cancel: CancelToken, waiting: usize| {
                let done = done.clone();
                scope.spawn(move || {
                    // Its room is held until it has reported, so the next can't report first.
                    let result = budget.reserve(6, &cancel);
                    done.send((name, result.is_ok())).unwrap();
                });
                wait_for_waiting(budget, waiting);
            };
            spawn("a", CancelToken::new(), 1);
            spawn("b", cancel.clone(), 2);
            spawn("c", CancelToken::new(), 3);

            cancel.cancel();
            assert_eq!(
                order.recv_timeout(Duration::from_secs(10)).unwrap(),
                ("b", false)
            );
            drop(first);
            assert_eq!(
                order.recv_timeout(Duration::from_secs(10)).unwrap(),
                ("a", true)
            );
            assert_eq!(
                order.recv_timeout(Duration::from_secs(10)).unwrap(),
                ("c", true)
            );
        });
        assert_eq!(budget.state.lock().unwrap().used, 0);
    }
}
