use std::ops::Range;

use crate::clip::{Clip, ClipMeta, style::BaseStyle};
use crate::render::context::FrameContext;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopMode {
    None,
    Repeat,
    PingPong,
}

#[derive(Debug, Clone)]
pub struct ImageClip {
    pub meta: ClipMeta,
    pub source: String,
    pub style: BaseStyle,
}

impl Clip for ImageClip {
    fn meta(&self) -> &ClipMeta {
        &self.meta
    }

    fn draw(&self, _frame: u32, _frame_ctx: &FrameContext) {}
}

#[derive(Debug, Clone)]
pub struct VideoClip {
    pub meta: ClipMeta,
    pub source: String,
    pub style: BaseStyle,
    pub trim: Option<Range<f32>>,
    pub speed: f32,
    pub r#loop: LoopMode,
}

impl Clip for VideoClip {
    fn meta(&self) -> &ClipMeta {
        &self.meta
    }

    fn draw(&self, _frame: u32, _frame_ctx: &FrameContext) {}
}
