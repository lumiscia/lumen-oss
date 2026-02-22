use crate::clip::{Clip, ClipMeta, style::TextStyle};
use crate::render::context::FrameContext;

#[derive(Debug, Clone)]
pub struct TextClip {
    pub meta: ClipMeta,
    pub content: String,
    pub style: TextStyle,
}

impl Clip for TextClip {
    fn meta(&self) -> &ClipMeta {
        &self.meta
    }

    fn draw(&self, _frame: u32, _frame_ctx: &FrameContext) {}
}
