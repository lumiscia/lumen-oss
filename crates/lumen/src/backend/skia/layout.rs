use std::collections::HashMap;

use skia_safe::Rect;
use taffy::prelude::{
    AlignItems, AvailableSpace, Dimension, Display, FlexDirection, JustifyContent,
    LengthPercentage, NodeId, Rect as TaffyRect, Size as TaffySize, Style as TaffyStyle, TaffyTree,
};

use crate::backend::RenderError;
use crate::compile::{CompiledLayoutNode, CompiledLayoutNodeKind, RuntimeFrameContext};
use crate::model::{LayoutAlign, LayoutDirection, LayoutJustify};

#[derive(Debug, Clone)]
pub struct LayoutBox {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

pub fn compute_layout_boxes(
    root: &CompiledLayoutNode,
    frame_state: &RuntimeFrameContext,
    bounds: Rect,
) -> Result<HashMap<String, LayoutBox>, RenderError> {
    let mut taffy = TaffyTree::<()>::new();
    let mut id_map = HashMap::<String, NodeId>::new();
    let root_node = build_layout_node(&mut taffy, root, frame_state, &mut id_map)?;

    taffy
        .compute_layout(
            root_node,
            TaffySize {
                width: AvailableSpace::Definite(bounds.width()),
                height: AvailableSpace::Definite(bounds.height()),
            },
        )
        .map_err(|err| RenderError::Failed(format!("taffy layout failed: {err}")))?;

    let mut boxes = HashMap::new();
    for (id, node_id) in id_map {
        if let Ok(layout) = taffy.layout(node_id) {
            boxes.insert(
                id,
                LayoutBox {
                    x: bounds.left + layout.location.x,
                    y: bounds.top + layout.location.y,
                    width: layout.size.width,
                    height: layout.size.height,
                },
            );
        }
    }

    Ok(boxes)
}

fn build_layout_node(
    taffy: &mut TaffyTree<()>,
    node: &CompiledLayoutNode,
    frame_state: &RuntimeFrameContext,
    id_map: &mut HashMap<String, NodeId>,
) -> Result<NodeId, RenderError> {
    let style = taffy_style(node, frame_state);

    let node_id = match &node.kind {
        CompiledLayoutNodeKind::Container { children } => {
            let mut child_nodes = Vec::with_capacity(children.len());
            for child in children {
                child_nodes.push(build_layout_node(taffy, child, frame_state, id_map)?);
            }
            taffy
                .new_with_children(style, child_nodes.as_slice())
                .map_err(|err| {
                    RenderError::Failed(format!("taffy new_with_children failed: {err}"))
                })?
        }
        CompiledLayoutNodeKind::Text { .. } | CompiledLayoutNodeKind::Image { .. } => taffy
            .new_leaf(style)
            .map_err(|err| RenderError::Failed(format!("taffy new_leaf failed: {err}")))?,
    };

    id_map.insert(node.id.clone(), node_id);
    Ok(node_id)
}

fn taffy_style(node: &CompiledLayoutNode, frame_state: &RuntimeFrameContext) -> TaffyStyle {
    let mut style = TaffyStyle::default();
    style.display = Display::Flex;
    style.flex_direction = match node.style.direction.unwrap_or(LayoutDirection::Column) {
        LayoutDirection::Row => FlexDirection::Row,
        LayoutDirection::Column => FlexDirection::Column,
    };
    style.justify_content = node.style.justify.map(to_justify_content);
    style.align_items = node.style.align.map(to_align_items);

    style.size = TaffySize {
        width: dimension(node.style.width.as_ref(), frame_state),
        height: dimension(node.style.height.as_ref(), frame_state),
    };
    style.min_size = TaffySize {
        width: dimension(node.style.min_width.as_ref(), frame_state),
        height: dimension(node.style.min_height.as_ref(), frame_state),
    };
    style.max_size = TaffySize {
        width: dimension(node.style.max_width.as_ref(), frame_state),
        height: dimension(node.style.max_height.as_ref(), frame_state),
    };

    style.padding = TaffyRect {
        left: length_percentage(node.style.padding_left.as_ref(), frame_state),
        right: length_percentage(node.style.padding_right.as_ref(), frame_state),
        top: length_percentage(node.style.padding_top.as_ref(), frame_state),
        bottom: length_percentage(node.style.padding_bottom.as_ref(), frame_state),
    };

    let gap = node
        .style
        .gap
        .as_ref()
        .map(|value| value.resolve(frame_state))
        .unwrap_or(0.0)
        .max(0.0);
    style.gap = TaffySize {
        width: LengthPercentage::length(gap),
        height: LengthPercentage::length(gap),
    };

    style.flex_grow = node
        .style
        .grow
        .as_ref()
        .map(|value| value.resolve(frame_state))
        .unwrap_or(0.0)
        .max(0.0);
    style.flex_shrink = node
        .style
        .shrink
        .as_ref()
        .map(|value| value.resolve(frame_state))
        .unwrap_or(1.0)
        .max(0.0);

    style.flex_basis = dimension(node.style.basis.as_ref(), frame_state);

    style
}

fn to_justify_content(justify: LayoutJustify) -> JustifyContent {
    match justify {
        LayoutJustify::Start => JustifyContent::Start,
        LayoutJustify::Center => JustifyContent::Center,
        LayoutJustify::End => JustifyContent::End,
        LayoutJustify::SpaceBetween => JustifyContent::SpaceBetween,
        LayoutJustify::SpaceAround => JustifyContent::SpaceAround,
        LayoutJustify::SpaceEvenly => JustifyContent::SpaceEvenly,
    }
}

fn to_align_items(align: LayoutAlign) -> AlignItems {
    match align {
        LayoutAlign::Start => AlignItems::Start,
        LayoutAlign::Center => AlignItems::Center,
        LayoutAlign::End => AlignItems::End,
        LayoutAlign::Stretch => AlignItems::Stretch,
    }
}

fn dimension(
    handle: Option<&crate::compile::ScalarHandle>,
    frame_state: &RuntimeFrameContext,
) -> Dimension {
    match handle {
        Some(handle) => Dimension::length(handle.resolve(frame_state).max(0.0)),
        None => Dimension::auto(),
    }
}

fn length_percentage(
    handle: Option<&crate::compile::ScalarHandle>,
    frame_state: &RuntimeFrameContext,
) -> LengthPercentage {
    match handle {
        Some(handle) => LengthPercentage::length(handle.resolve(frame_state).max(0.0)),
        None => LengthPercentage::length(0.0),
    }
}
