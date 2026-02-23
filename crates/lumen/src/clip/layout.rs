use skia_safe::ClipOp;
use taffy::Overflow;
use taffy::prelude::{AvailableSpace, Size, Style, TaffyTree, length};
use taffy::tree::NodeId;

use crate::clip::style::BaseStyle;
use crate::clip::style::StyleContext;
use crate::clip::{Clip, ClipGeometry, ClipMeta};
use crate::render::backend::RenderError;
use crate::render::context::{FrameContext, RendererContext};

#[derive(Debug, Clone)]
pub enum LayoutContent {
    Shape(super::shape::ShapeClip),
    Text(super::text::TextClip),
    Image(super::media::ImageClip),
    Video(super::media::VideoClip),
    Layout(Box<LayoutClip>),
}

#[derive(Debug, Clone)]
pub struct LayoutNode {
    pub id: Option<String>,
    pub style: Style,
    pub content: Option<LayoutContent>,
    pub children: Vec<LayoutNode>,
}

/// Transient context attached to taffy nodes during layout computation and rendering.
#[derive(Debug, Clone)]
struct LayoutNodeContext {
    content: Option<LayoutContent>,
}

#[derive(Debug, Clone)]
pub struct LayoutClip {
    pub meta: ClipMeta,
    pub geometry: ClipGeometry,
    pub style: BaseStyle,
    pub children: Vec<LayoutNode>,
}

impl LayoutClip {
    /// Build a taffy tree from the IR children, compute layout, and return
    /// the tree with the synthetic root node.
    fn build_layout(
        &self,
        style_ctx: &StyleContext<'_>,
        available_width: f32,
        available_height: f32,
    ) -> Result<(TaffyTree<LayoutNodeContext>, NodeId), RenderError> {
        let mut tree = TaffyTree::new();

        let child_ids = self
            .children
            .iter()
            .map(|child| insert_node(&mut tree, child))
            .collect::<Result<Vec<_>, _>>()?;

        let root = tree
            .new_with_children(
                Style {
                    size: Size {
                        width: length(available_width),
                        height: length(available_height),
                    },
                    ..Default::default()
                },
                &child_ids,
            )
            .map_err(|_| RenderError::Unsupported("taffy root creation failed"))?;

        tree.compute_layout_with_measure(
            root,
            Size {
                width: AvailableSpace::Definite(available_width),
                height: AvailableSpace::Definite(available_height),
            },
            |known_dimensions, available_space, _node_id, node_context, _style| {
                let Some(node_context) = node_context else {
                    return known_dimensions.unwrap_or(Size::ZERO);
                };
                let Some(LayoutContent::Text(clip)) = node_context.content.as_ref() else {
                    return known_dimensions.unwrap_or(Size::ZERO);
                };

                let style_ctx = *style_ctx;
                let available_width =
                    known_dimensions
                        .width
                        .unwrap_or_else(|| match available_space.width {
                            AvailableSpace::Definite(value) => value,
                            AvailableSpace::MinContent | AvailableSpace::MaxContent => f32::MAX,
                        });
                let (measured_width, measured_height) = clip.measure(available_width, &style_ctx);
                Size {
                    width: known_dimensions.width.unwrap_or(measured_width),
                    height: known_dimensions.height.unwrap_or(measured_height),
                }
            },
        )
        .map_err(|_| RenderError::Unsupported("taffy layout computation failed"))?;

        Ok((tree, root))
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
                let expression_scope = renderer_ctx.expression_scope().clone();
                let style_ctx = StyleContext::with_scope(frame, &expression_scope);
                let geometry = self.geometry.resolve_with_context(
                    &style_ctx,
                    frame_ctx.width as f32 * 0.05,
                    frame_ctx.height as f32 * 0.05,
                    frame_ctx.width as f32 * 0.9,
                    frame_ctx.height as f32 * 0.9,
                    0.0,
                    0.0,
                );
                if self.children.is_empty() {
                    return Ok(());
                }

                let (tree, root) =
                    self.build_layout(&style_ctx, geometry.width, geometry.height)?;

                for child in tree
                    .children(root)
                    .map_err(|_| RenderError::Unsupported("taffy tree traversal failed"))?
                {
                    draw_node_bounds(
                        &tree,
                        child,
                        frame,
                        frame_ctx,
                        (geometry.left(), geometry.top()),
                        renderer_ctx,
                    )?;
                }

                Ok(())
            })
    }
}

/// Recursively insert a `LayoutNode` IR tree into a taffy tree.
fn insert_node(
    tree: &mut TaffyTree<LayoutNodeContext>,
    node: &LayoutNode,
) -> Result<NodeId, RenderError> {
    let child_ids = node
        .children
        .iter()
        .map(|child| insert_node(tree, child))
        .collect::<Result<Vec<_>, _>>()?;

    let node_id = tree
        .new_with_children(node.style.clone(), &child_ids)
        .map_err(|_| RenderError::Unsupported("taffy tree construction failed"))?;

    tree.set_node_context(
        node_id,
        Some(LayoutNodeContext {
            content: node.content.clone(),
        }),
    )
    .map_err(|_| RenderError::Unsupported("taffy context set failed"))?;

    Ok(node_id)
}

/// Walk a computed taffy node, draw content, and recurse into children.
fn draw_node_bounds(
    tree: &TaffyTree<LayoutNodeContext>,
    node: NodeId,
    frame: u32,
    frame_ctx: &FrameContext,
    parent_offset: (f32, f32),
    renderer_ctx: &mut RendererContext,
) -> Result<(), RenderError> {
    let layout = tree
        .layout(node)
        .map_err(|_| RenderError::Unsupported("taffy layout not computed"))?;
    let x = parent_offset.0 + layout.location.x;
    let y = parent_offset.1 + layout.location.y;
    let width = layout.size.width.max(0.0);
    let height = layout.size.height.max(0.0);
    let rect = skia_safe::Rect::from_xywh(x, y, width, height);
    let context = tree.get_node_context(node).cloned();
    let style = tree
        .style(node)
        .map_err(|_| RenderError::Unsupported("taffy style lookup failed"))?;

    let is_overflow_hidden =
        style.overflow.x == Overflow::Hidden || style.overflow.y == Overflow::Hidden;
    if is_overflow_hidden {
        renderer_ctx.canvas().save();
        renderer_ctx
            .canvas()
            .clip_rect(rect, ClipOp::Intersect, true);
    }

    if let Some(content) = context.and_then(|ctx| ctx.content) {
        content.draw(rect, frame, frame_ctx, renderer_ctx)?;
    }

    for child in tree
        .children(node)
        .map_err(|_| RenderError::Unsupported("taffy tree traversal failed"))?
    {
        draw_node_bounds(tree, child, frame, frame_ctx, (x, y), renderer_ctx)?;
    }
    if is_overflow_hidden {
        renderer_ctx.canvas().restore();
    }

    Ok(())
}

fn literal_f32(value: f32) -> crate::clip::style::StyleProperty<f32> {
    crate::clip::style::StyleProperty::Value(crate::clip::style::StyleValue::Literal(value))
}

fn geometry_from_rect(rect: skia_safe::Rect) -> ClipGeometry {
    ClipGeometry {
        x: literal_f32(rect.left),
        y: literal_f32(rect.top),
        width: literal_f32(rect.width()),
        height: literal_f32(rect.height()),
        anchor_x: literal_f32(0.0),
        anchor_y: literal_f32(0.0),
    }
}

impl LayoutContent {
    fn draw(
        &self,
        rect: skia_safe::Rect,
        frame: u32,
        frame_ctx: &FrameContext,
        renderer_ctx: &mut RendererContext,
    ) -> Result<(), RenderError> {
        match self {
            Self::Shape(clip) => {
                let mut clip = clip.clone();
                clip.geometry = geometry_from_rect(rect);
                clip.draw(frame, frame_ctx, renderer_ctx)
            }
            Self::Text(clip) => {
                let mut clip = clip.clone();
                clip.geometry = geometry_from_rect(rect);
                clip.draw(frame, frame_ctx, renderer_ctx)
            }
            Self::Image(clip) => {
                let mut clip = clip.clone();
                clip.geometry = geometry_from_rect(rect);
                clip.draw(frame, frame_ctx, renderer_ctx)
            }
            Self::Video(clip) => {
                let mut clip = clip.clone();
                clip.geometry = geometry_from_rect(rect);
                clip.draw(frame, frame_ctx, renderer_ctx)
            }
            Self::Layout(clip) => {
                let mut clip = clip.as_ref().clone();
                clip.geometry = geometry_from_rect(rect);
                clip.draw(frame, frame_ctx, renderer_ctx)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use skia_safe::{BlendMode, font_style::Slant as FontSlant};
    use taffy::{
        Overflow,
        geometry::{Point, Rect},
        prelude::{Display, FlexDirection, LengthPercentageAuto, Position, Size, Style, length},
    };

    use super::{LayoutClip, LayoutContent, LayoutNode};
    use crate::clip::{
        Clip, ClipGeometry, ClipMeta,
        shape::{ShapeClip, ShapeKind},
        style::{
            BaseStyle, Fill, RectStyle, StyleContext, StyleProperty, StyleValue, TextAlign,
            TextDecoration, TextOverflow, TextStyle, TransformStyle, VerticalAlign,
        },
        text::TextClip,
    };
    use crate::render::{
        backend::read_surface_rgba,
        context::{FrameContext, RendererContext},
    };
    use crate::time::Rational;

    fn literal<T>(value: T) -> StyleProperty<T> {
        StyleProperty::Value(StyleValue::Literal(value))
    }

    fn base_style() -> BaseStyle {
        BaseStyle {
            visible: literal(true),
            opacity: literal(1.0),
            blend_mode: BlendMode::SrcOver,
            blur: literal(0.0),
            shadows: Vec::new(),
            clip_radius: [literal(0.0), literal(0.0), literal(0.0), literal(0.0)],
            transform: TransformStyle {
                translate: [literal(0.0), literal(0.0)],
                scale: [literal(1.0), literal(1.0)],
                rotation: literal(0.0),
                skew: [literal(0.0), literal(0.0)],
                origin: [literal(0.0), literal(0.0)],
            },
            alignment: [literal(0.0), literal(0.0)],
            mask: None,
        }
    }

    fn shape_content(color: [u8; 4]) -> LayoutContent {
        LayoutContent::Shape(ShapeClip {
            meta: ClipMeta {
                id: Some("node-shape".to_owned()),
                start_frame: 0,
                end_frame: 10,
            },
            geometry: ClipGeometry::default(),
            kind: ShapeKind::Rectangle(RectStyle {
                base: base_style(),
                width: literal(20.0),
                height: literal(10.0),
                corner_radius: [literal(0.0), literal(0.0), literal(0.0), literal(0.0)],
                fill: Some(Fill::Solid {
                    color: [
                        literal(color[0]),
                        literal(color[1]),
                        literal(color[2]),
                        literal(color[3]),
                    ],
                }),
                stroke: None,
            }),
        })
    }

    fn text_content(content: &str) -> LayoutContent {
        LayoutContent::Text(TextClip {
            meta: ClipMeta {
                id: Some("node-text".to_owned()),
                start_frame: 0,
                end_frame: 10,
            },
            geometry: ClipGeometry::default(),
            content: content.to_owned(),
            style: TextStyle {
                base: base_style(),
                font_family: "sans-serif".to_owned(),
                font_size: literal(16.0),
                font_weight: literal(600),
                font_style: FontSlant::Upright,
                color: [literal(220), literal(30), literal(40), literal(255)],
                line_height: literal(1.2),
                letter_spacing: literal(0.0),
                text_align: TextAlign::Left,
                vertical_align: VerticalAlign::Top,
                max_width: None,
                max_lines: None,
                overflow: TextOverflow::Clip,
                decoration: TextDecoration::None,
            },
        })
    }

    fn frame_context() -> FrameContext {
        FrameContext {
            frame: 0,
            time_seconds: 0.0,
            width: 140,
            height: 120,
            device_scale: 1.0,
        }
    }

    fn rgba_at(pixels: &[u8], width: usize, x: usize, y: usize) -> [u8; 4] {
        let idx = (y * width + x) * 4;
        [
            pixels[idx],
            pixels[idx + 1],
            pixels[idx + 2],
            pixels[idx + 3],
        ]
    }

    #[test]
    fn layout_clip_draws_taffy_computed_child_bounds() {
        let clip = LayoutClip {
            meta: ClipMeta {
                id: Some("layout".to_owned()),
                start_frame: 0,
                end_frame: 0,
            },
            geometry: ClipGeometry {
                x: literal(10.0),
                y: literal(10.0),
                width: literal(80.0),
                height: literal(60.0),
                anchor_x: literal(0.0),
                anchor_y: literal(0.0),
            },
            style: base_style(),
            children: vec![LayoutNode {
                id: Some("root".to_owned()),
                style: Style {
                    size: Size {
                        width: length(60.0),
                        height: length(40.0),
                    },
                    ..Default::default()
                },
                content: None,
                children: vec![LayoutNode {
                    id: Some("child".to_owned()),
                    style: Style {
                        size: Size {
                            width: length(20.0),
                            height: length(10.0),
                        },
                        ..Default::default()
                    },
                    content: Some(shape_content([5, 150, 220, 255])),
                    children: vec![],
                }],
            }],
        };

        let mut renderer_ctx =
            RendererContext::new(140, 120, Rational::new(30, 1)).expect("renderer");
        renderer_ctx.clear();

        clip.draw(0, &frame_context(), &mut renderer_ctx)
            .expect("layout draw");

        let pixels = read_surface_rgba(&mut renderer_ctx).expect("readback");
        assert_eq!(rgba_at(&pixels, 140, 15, 15), [5, 150, 220, 255]);
    }

    #[test]
    fn layout_uses_text_measure_for_intrinsic_size() {
        let clip = LayoutClip {
            meta: ClipMeta {
                id: Some("layout".to_owned()),
                start_frame: 0,
                end_frame: 0,
            },
            geometry: ClipGeometry::default(),
            style: base_style(),
            children: vec![LayoutNode {
                id: Some("text".to_owned()),
                style: Style::default(),
                content: Some(text_content("Hello measured world")),
                children: vec![],
            }],
        };

        let style_ctx = StyleContext::new(0);
        let (tree, root) = clip
            .build_layout(&style_ctx, 240.0, 120.0)
            .expect("layout build");
        let child = tree
            .children(root)
            .expect("children")
            .first()
            .copied()
            .expect("text child");
        let layout = tree.layout(child).expect("layout");
        assert!(layout.size.width > 0.0);
        assert!(layout.size.height > 0.0);
    }

    #[test]
    fn overflow_hidden_clips_out_of_bounds_children() {
        let clip = LayoutClip {
            meta: ClipMeta {
                id: Some("layout".to_owned()),
                start_frame: 0,
                end_frame: 0,
            },
            geometry: ClipGeometry {
                x: literal(10.0),
                y: literal(10.0),
                width: literal(80.0),
                height: literal(60.0),
                anchor_x: literal(0.0),
                anchor_y: literal(0.0),
            },
            style: base_style(),
            children: vec![LayoutNode {
                id: Some("clipped-parent".to_owned()),
                style: Style {
                    size: Size {
                        width: length(30.0),
                        height: length(20.0),
                    },
                    overflow: Point {
                        x: Overflow::Hidden,
                        y: Overflow::Hidden,
                    },
                    ..Default::default()
                },
                content: None,
                children: vec![LayoutNode {
                    id: Some("wide-child".to_owned()),
                    style: Style {
                        size: Size {
                            width: length(60.0),
                            height: length(20.0),
                        },
                        ..Default::default()
                    },
                    content: Some(shape_content([200, 20, 20, 255])),
                    children: vec![],
                }],
            }],
        };

        let mut renderer_ctx =
            RendererContext::new(140, 120, Rational::new(30, 1)).expect("renderer");
        renderer_ctx.clear();

        clip.draw(0, &frame_context(), &mut renderer_ctx)
            .expect("layout draw");

        let pixels = read_surface_rgba(&mut renderer_ctx).expect("readback");
        assert_eq!(rgba_at(&pixels, 140, 22, 18), [200, 20, 20, 255]);
        assert_eq!(rgba_at(&pixels, 140, 50, 18)[3], 0);
    }

    #[test]
    fn absolute_positioned_child_renders_at_inset_offset() {
        let clip = LayoutClip {
            meta: ClipMeta {
                id: Some("layout".to_owned()),
                start_frame: 0,
                end_frame: 0,
            },
            geometry: ClipGeometry {
                x: literal(10.0),
                y: literal(10.0),
                width: literal(100.0),
                height: literal(70.0),
                anchor_x: literal(0.0),
                anchor_y: literal(0.0),
            },
            style: base_style(),
            children: vec![LayoutNode {
                id: Some("root".to_owned()),
                style: Style {
                    display: Display::Flex,
                    flex_direction: FlexDirection::Row,
                    size: Size {
                        width: length(80.0),
                        height: length(50.0),
                    },
                    ..Default::default()
                },
                content: None,
                children: vec![LayoutNode {
                    id: Some("absolute-child".to_owned()),
                    style: Style {
                        position: Position::Absolute,
                        inset: Rect {
                            left: LengthPercentageAuto::length(12.0),
                            right: LengthPercentageAuto::auto(),
                            top: LengthPercentageAuto::length(8.0),
                            bottom: LengthPercentageAuto::auto(),
                        },
                        size: Size {
                            width: length(20.0),
                            height: length(10.0),
                        },
                        ..Default::default()
                    },
                    content: Some(shape_content([20, 180, 60, 255])),
                    children: vec![],
                }],
            }],
        };

        let mut renderer_ctx =
            RendererContext::new(140, 120, Rational::new(30, 1)).expect("renderer");
        renderer_ctx.clear();

        clip.draw(0, &frame_context(), &mut renderer_ctx)
            .expect("layout draw");

        let pixels = read_surface_rgba(&mut renderer_ctx).expect("readback");
        assert_eq!(rgba_at(&pixels, 140, 24, 19), [20, 180, 60, 255]);
    }

    #[test]
    fn flex_grow_children_split_parent_width_evenly() {
        let clip = LayoutClip {
            meta: ClipMeta {
                id: Some("layout".to_owned()),
                start_frame: 0,
                end_frame: 0,
            },
            geometry: ClipGeometry {
                x: literal(10.0),
                y: literal(10.0),
                width: literal(100.0),
                height: literal(60.0),
                anchor_x: literal(0.0),
                anchor_y: literal(0.0),
            },
            style: base_style(),
            children: vec![LayoutNode {
                id: Some("flex-root".to_owned()),
                style: Style {
                    display: Display::Flex,
                    flex_direction: FlexDirection::Row,
                    size: Size {
                        width: length(80.0),
                        height: length(20.0),
                    },
                    ..Default::default()
                },
                content: None,
                children: vec![
                    LayoutNode {
                        id: Some("left".to_owned()),
                        style: Style {
                            flex_grow: 1.0,
                            flex_basis: length(0.0),
                            ..Default::default()
                        },
                        content: Some(shape_content([220, 30, 30, 255])),
                        children: vec![],
                    },
                    LayoutNode {
                        id: Some("right".to_owned()),
                        style: Style {
                            flex_grow: 1.0,
                            flex_basis: length(0.0),
                            ..Default::default()
                        },
                        content: Some(shape_content([30, 30, 220, 255])),
                        children: vec![],
                    },
                ],
            }],
        };

        let mut renderer_ctx =
            RendererContext::new(140, 120, Rational::new(30, 1)).expect("renderer");
        renderer_ctx.clear();

        clip.draw(0, &frame_context(), &mut renderer_ctx)
            .expect("layout draw");

        let pixels = read_surface_rgba(&mut renderer_ctx).expect("readback");
        assert_eq!(rgba_at(&pixels, 140, 24, 18), [220, 30, 30, 255]);
        assert_eq!(rgba_at(&pixels, 140, 64, 18), [30, 30, 220, 255]);
    }
}
