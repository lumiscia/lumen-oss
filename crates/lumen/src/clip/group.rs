use crate::clip::{Clip, ClipMeta, ClipType};
use crate::render::context::FrameContext;

#[derive(Debug)]
pub struct GroupClip {
    pub meta: ClipMeta,
    pub children: Vec<ClipType>,
}

impl Clip for GroupClip {
    fn meta(&self) -> &ClipMeta {
        &self.meta
    }

    fn draw(&self, _frame: u32, _frame_ctx: &FrameContext) {}
}
