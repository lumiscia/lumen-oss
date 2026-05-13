use std::time::Duration;

pub(super) const FRAME_TIMING_LOG_INTERVAL: u32 = 60;

#[derive(Default)]
pub(super) struct FrameTimingTotals {
    frames: u32,
    prefetch: Duration,
    render_submit: Duration,
    precompile: Duration,
    gpu_wait: Duration,
    cuda_copy: Duration,
    encode_write: Duration,
    progress: Duration,
}

impl FrameTimingTotals {
    pub(super) fn add(&mut self, timing: FrameTiming) {
        self.frames = self.frames.saturating_add(1);
        self.prefetch += timing.prefetch;
        self.render_submit += timing.render_submit;
        self.precompile += timing.precompile;
        self.gpu_wait += timing.gpu_wait;
        self.cuda_copy += timing.cuda_copy;
        self.encode_write += timing.encode_write;
        self.progress += timing.progress;
    }

    pub(super) fn log(&self, frame: u32, total_frames: u32, label: &str) {
        tracing::info!(
            label,
            frame,
            total_frames,
            frames_measured = self.frames,
            prefetch_ms = self.prefetch.as_millis(),
            render_submit_ms = self.render_submit.as_millis(),
            precompile_ms = self.precompile.as_millis(),
            gpu_wait_ms = self.gpu_wait.as_millis(),
            cuda_copy_ms = self.cuda_copy.as_millis(),
            encode_write_ms = self.encode_write.as_millis(),
            progress_callback_ms = self.progress.as_millis(),
            "render frame timing"
        );
    }
}

#[derive(Default)]
pub(super) struct FrameTiming {
    pub(super) prefetch: Duration,
    pub(super) render_submit: Duration,
    pub(super) precompile: Duration,
    pub(super) gpu_wait: Duration,
    pub(super) cuda_copy: Duration,
    pub(super) encode_write: Duration,
    pub(super) progress: Duration,
}
