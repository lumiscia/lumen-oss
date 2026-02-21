use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};

#[cfg(feature = "renderer-skia")]
use crate::backend::skia::SkiaRenderer;
use crate::backend::{FrameProvider, RenderError, Renderer};
use crate::compile::CompiledTimeline;

const MAX_RENDER_WORKERS: usize = 32;
#[derive(Debug, Clone, Copy)]
pub struct RenderOrchestrator {
    thread_count: usize,
}

impl RenderOrchestrator {
    pub fn new(thread_count: usize) -> Self {
        Self {
            thread_count: thread_count.max(1).min(MAX_RENDER_WORKERS),
        }
    }

    pub fn thread_count(&self) -> usize {
        self.thread_count
    }

    #[cfg(feature = "renderer-skia")]
    pub fn render_range<P, F>(
        &self,
        timeline: Arc<CompiledTimeline>,
        frame_range: Range<u64>,
        make_provider: impl Fn() -> P + Send + Sync + 'static,
        on_frame: F,
    ) -> Result<(), RenderError>
    where
        P: FrameProvider + 'static,
        F: FnMut(u64, Vec<u8>) -> Result<(), RenderError> + Send,
    {
        self.render_range_with(
            timeline,
            frame_range,
            make_provider,
            |width, height| Ok::<_, RenderError>(Box::new(SkiaRenderer::new(width, height)?)),
            on_frame,
        )
    }

    pub fn render_range_with<P, F>(
        &self,
        timeline: Arc<CompiledTimeline>,
        frame_range: Range<u64>,
        make_provider: impl Fn() -> P + Send + Sync + 'static,
        make_renderer: impl Fn(u32, u32) -> Result<Box<dyn Renderer>, RenderError>
        + Send
        + Sync
        + 'static,
        mut on_frame: F,
    ) -> Result<(), RenderError>
    where
        P: FrameProvider + 'static,
        F: FnMut(u64, Vec<u8>) -> Result<(), RenderError> + Send,
    {
        if frame_range.start >= frame_range.end {
            return Ok(());
        }

        let total_frames = frame_range.end - frame_range.start;
        let worker_count = self.thread_count.min(total_frames as usize).max(1);
        let queue_depth = worker_count.saturating_mul(2).max(2);

        let next_frame = Arc::new(AtomicU64::new(frame_range.start));
        let stop = Arc::new(AtomicBool::new(false));
        let provider_factory = Arc::new(make_provider);
        let renderer_factory = Arc::new(make_renderer);
        let in_flight_budget = Arc::new(OutstandingBudget::new(queue_depth));

        enum WorkerMsg {
            Frame { frame: u64, pixels: Vec<u8> },
            Err(RenderError),
        }

        let (tx, rx) = mpsc::sync_channel::<WorkerMsg>(queue_depth);
        let mut handles = Vec::with_capacity(worker_count);

        for _ in 0..worker_count {
            let tx = tx.clone();
            let next_frame = Arc::clone(&next_frame);
            let timeline = Arc::clone(&timeline);
            let stop = Arc::clone(&stop);
            let provider_factory = Arc::clone(&provider_factory);
            let renderer_factory = Arc::clone(&renderer_factory);
            let in_flight_budget = Arc::clone(&in_flight_budget);

            handles.push(std::thread::spawn(move || {
                let mut provider = provider_factory();
                let mut renderer =
                    match renderer_factory(timeline.canvas.width, timeline.canvas.height) {
                        Ok(renderer) => renderer,
                        Err(err) => {
                            if tx.send(WorkerMsg::Err(err)).is_err() {}
                            return;
                        }
                    };

                loop {
                    if stop.load(Ordering::Acquire) {
                        break;
                    }

                    if !in_flight_budget.acquire(&stop) {
                        break;
                    }

                    let frame = next_frame.fetch_add(1, Ordering::AcqRel);
                    if frame >= frame_range.end {
                        in_flight_budget.release();
                        break;
                    }

                    match renderer.render_frame(&timeline, frame, &mut provider) {
                        Ok(pixels) => {
                            if tx.send(WorkerMsg::Frame { frame, pixels }).is_err() {
                                in_flight_budget.release();
                                break;
                            }
                        }
                        Err(err) => {
                            stop.store(true, Ordering::Release);
                            in_flight_budget.release();
                            if tx.send(WorkerMsg::Err(err)).is_err() {}
                            break;
                        }
                    }
                }
            }));
        }
        drop(tx);

        let mut expected = frame_range.start;
        let mut pending = BTreeMap::<u64, Vec<u8>>::new();
        let mut error = None;

        while expected < frame_range.end {
            match rx.recv() {
                Ok(WorkerMsg::Frame { frame, pixels }) => {
                    pending.insert(frame, pixels);
                    while let Some(pixels) = pending.remove(&expected) {
                        let emit_result = on_frame(expected, pixels);
                        in_flight_budget.release();
                        if let Err(err) = emit_result {
                            error = Some(err);
                            stop.store(true, Ordering::Release);
                            in_flight_budget.notify_all();
                            break;
                        }
                        expected += 1;
                    }
                    if error.is_some() {
                        break;
                    }
                }
                Ok(WorkerMsg::Err(err)) => {
                    error = Some(err);
                    stop.store(true, Ordering::Release);
                    in_flight_budget.notify_all();
                    break;
                }
                Err(_) => {
                    if expected < frame_range.end {
                        error = Some(RenderError::Failed(
                            "render workers exited before completing frame range".to_string(),
                        ));
                        stop.store(true, Ordering::Release);
                        in_flight_budget.notify_all();
                    }
                    break;
                }
            }
        }

        // Ensure all workers unblock for joining, even on normal completion.
        stop.store(true, Ordering::Release);
        in_flight_budget.notify_all();

        let mut worker_panicked = false;
        for handle in handles {
            if handle.join().is_err() {
                worker_panicked = true;
            }
        }

        if let Some(err) = error {
            return Err(err);
        }

        if worker_panicked {
            return Err(RenderError::WorkerPanicked);
        }

        Ok(())
    }
}

impl Default for RenderOrchestrator {
    fn default() -> Self {
        Self::new(
            std::thread::available_parallelism()
                .map(|value| value.get())
                .unwrap_or(1),
        )
    }
}

#[derive(Debug)]
struct OutstandingBudget {
    available: Mutex<usize>,
    cv: Condvar,
}

impl OutstandingBudget {
    fn new(slots: usize) -> Self {
        Self {
            available: Mutex::new(slots.max(1)),
            cv: Condvar::new(),
        }
    }

    fn acquire(&self, stop: &AtomicBool) -> bool {
        let mut available = match self.available.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        while *available == 0 {
            if stop.load(Ordering::Acquire) {
                return false;
            }
            available = match self.cv.wait(available) {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
        }
        *available -= 1;
        true
    }

    fn release(&self) {
        let mut available = match self.available.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        *available = available.saturating_add(1);
        self.cv.notify_one();
    }

    fn notify_all(&self) {
        self.cv.notify_all();
    }
}
