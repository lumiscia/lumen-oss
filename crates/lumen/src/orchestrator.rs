use std::ops::Range;
use std::sync::Arc;

use crate::backend::{FrameProvider, RenderError, Renderer};
use crate::compile::CompiledTimeline;

#[derive(Debug, Clone, Copy)]
pub struct RenderOrchestrator {
    thread_count: usize,
}

impl RenderOrchestrator {
    pub fn new(thread_count: usize) -> Self {
        Self {
            thread_count: thread_count.max(1),
        }
    }

    pub fn thread_count(&self) -> usize {
        self.thread_count
    }

    pub fn render_range<P, R, M, F>(
        &self,
        _timeline: Arc<CompiledTimeline>,
        _frame_range: Range<u64>,
        _make_provider: M,
        _make_renderer: impl Fn() -> R + Send + Sync,
        _on_frame: F,
    ) -> Result<(), RenderError>
    where
        P: FrameProvider + 'static,
        R: Renderer + 'static,
        M: Fn() -> P + Send + Sync,
        F: FnMut(u64, Vec<u8>) + Send,
    {
        Err(RenderError::Failed(
            "render orchestrator not implemented yet".to_string(),
        ))
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
