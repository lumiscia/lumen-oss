use crate::clip::style::BaseStyle;
use crate::clip::{Clip, ClipMeta, ClipType, draw_with_base_style};
use crate::render::backend::RenderError;
use crate::render::context::{FrameContext, RendererContext};

#[derive(Debug)]
pub struct GroupClip {
    pub meta: ClipMeta,
    pub style: BaseStyle,
    pub children: Vec<ClipType>,
}

impl Clip for GroupClip {
    fn meta(&self) -> &ClipMeta {
        &self.meta
    }

    fn draw(
        &self,
        frame: u32,
        frame_ctx: &FrameContext,
        renderer_ctx: &mut RendererContext,
    ) -> Result<(), RenderError> {
        if !self.contains_frame(frame) {
            return Ok(());
        }

        draw_with_base_style(
            &self.style,
            frame_ctx,
            renderer_ctx,
            |renderer_ctx, _resolved| {
                for child in &self.children {
                    child.draw(frame, frame_ctx, renderer_ctx)?;
                }
                Ok(())
            },
        )
    }
}
