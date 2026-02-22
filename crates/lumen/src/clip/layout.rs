use skia_safe::{Color, Paint, Rect, paint::Style as PaintStyle};
use taffy::prelude::{AvailableSpace, Size, Style, TaffyTree};
use taffy::tree::NodeId;

use crate::clip::style::BaseStyle;
use crate::clip::{Clip, ClipMeta};
use crate::render::backend::RenderError;
use crate::render::context::{FrameContext, RendererContext};

#[derive(Debug, Clone)]
pub struct LayoutNode {
    pub id: Option<String>,
    pub style: Style,
    pub children: Vec<LayoutNode>,
}

#[derive(Debug, Clone)]
pub struct LayoutNodeContext {
    pub id: Option<String>,
}

#[derive(Debug)]
pub struct LayoutClip {
    pub meta: ClipMeta,
    pub style: BaseStyle,
    pub tree: TaffyTree<LayoutNodeContext>,
    pub root_node: Option<NodeId>,
}

impl LayoutClip {
    pub fn compute_layout(
        &mut self,
        available_space: Size<AvailableSpace>,
    ) -> Result<(), taffy::TaffyError> {
        if let Some(root_node) = self.root_node {
            self.tree.compute_layout(root_node, available_space)?;
        }

        Ok(())
    }
}

impl Clip for LayoutClip {
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

        self.style
            .draw(frame, frame_ctx, renderer_ctx, |renderer_ctx, _resolved| {
                let mut paint = Paint::default();
                paint.set_anti_alias(true);
                paint.set_style(PaintStyle::Stroke);
                paint.set_stroke_width(2.0);
                paint.set_color(Color::from_argb(180, 120, 220, 255));

                renderer_ctx.canvas().draw_rect(
                    Rect::from_xywh(
                        frame_ctx.width as f32 * 0.05,
                        frame_ctx.height as f32 * 0.05,
                        frame_ctx.width as f32 * 0.9,
                        frame_ctx.height as f32 * 0.9,
                    ),
                    &paint,
                );

                if self.root_node.is_some() {
                    let mut root_mark = Paint::default();
                    root_mark.set_anti_alias(true);
                    root_mark.set_color(Color::from_argb(220, 90, 200, 120));
                    renderer_ctx
                        .canvas()
                        .draw_circle((24.0, 24.0), 10.0, &root_mark);
                }

                Ok(())
            })
    }
}
