use taffy::prelude::{AvailableSpace, Size, Style, TaffyTree};
use taffy::tree::NodeId;

use crate::clip::{Clip, ClipMeta};
use crate::render::context::FrameContext;

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

    fn draw(&self, _frame: u32, _frame_ctx: &FrameContext) {}
}
