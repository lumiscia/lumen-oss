use crate::model::{Canvas, Timeline};

#[derive(Debug, Clone)]
pub struct CompiledTimeline {
    pub canvas: Canvas,
    pub timeline: Timeline,
}

impl CompiledTimeline {
    pub fn total_frames(&self) -> u64 {
        self.timeline.duration_frames
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RuntimeFrameContext {
    pub frame: u64,
}
