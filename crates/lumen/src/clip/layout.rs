use skia_safe::{Color, Paint, paint::Style as PaintStyle};
use taffy::prelude::{AvailableSpace, Size, Style, TaffyTree};
use taffy::tree::NodeId;

use crate::clip::style::BaseStyle;
use crate::clip::{Clip, ClipGeometry, ClipMeta};
use crate::render::backend::RenderError;
use crate::render::context::{FrameContext, RendererContext};

#[derive(Debug, Clone)]
pub enum LayoutContent {
    Shape(super::shape::ShapeClip),
    Text(super::text::TextClip),
    Image(super::media::ImageClip),
    Video(super::media::VideoClip),
}

#[derive(Debug, Clone)]
pub struct LayoutNode {
    pub id: Option<String>,
    pub style: Style,
    pub content: Option<LayoutContent>,
    pub children: Vec<LayoutNode>,
}

#[derive(Debug, Clone)]
pub struct LayoutNodeContext {
    pub id: Option<String>,
    pub content: Option<LayoutContent>,
}

#[derive(Debug)]
pub struct LayoutClip {
    pub meta: ClipMeta,
    pub geometry: ClipGeometry,
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

    fn draw_node_bounds(
        &self,
        node: NodeId,
        frame: u32,
        frame_ctx: &FrameContext,
        parent_offset: (f32, f32),
        depth: usize,
        renderer_ctx: &mut RendererContext,
    ) -> Result<(), RenderError> {
        let layout = self
            .tree
            .layout(node)
            .map_err(|_| RenderError::Unsupported("taffy layout not computed"))?;
        let x = parent_offset.0 + layout.location.x;
        let y = parent_offset.1 + layout.location.y;
        let width = layout.size.width.max(1.0);
        let height = layout.size.height.max(1.0);
        let rect = skia_safe::Rect::from_xywh(x, y, width, height);
        let context = self.tree.get_node_context(node).cloned();

        let mut fill = Paint::default();
        fill.set_anti_alias(true);
        fill.set_color(
            match context.as_ref().and_then(|ctx| ctx.content.as_ref()) {
                Some(LayoutContent::Shape(_)) => Color::from_argb(64, 70, 160, 255),
                Some(LayoutContent::Text(_)) => Color::from_argb(64, 80, 220, 120),
                Some(LayoutContent::Image(_)) => Color::from_argb(64, 240, 170, 80),
                Some(LayoutContent::Video(_)) => Color::from_argb(64, 240, 90, 90),
                None => Color::from_argb(32, 120, 220, 255),
            },
        );
        renderer_ctx.canvas().draw_rect(rect, &fill);

        let mut stroke = Paint::default();
        stroke.set_anti_alias(true);
        stroke.set_style(PaintStyle::Stroke);
        stroke.set_stroke_width(1.0 + depth.min(3) as f32);
        let depth_tint = (depth as u8).saturating_mul(24);
        stroke.set_color(Color::from_argb(
            220,
            120,
            220u8.saturating_sub(depth_tint / 2),
            255u8.saturating_sub(depth_tint),
        ));
        renderer_ctx.canvas().draw_rect(rect, &stroke);

        if let Some(content) = context.and_then(|ctx| ctx.content) {
            draw_layout_content(&content, rect, frame, frame_ctx, renderer_ctx)?;
        }

        for child in self
            .tree
            .children(node)
            .map_err(|_| RenderError::Unsupported("taffy tree traversal failed"))?
        {
            self.draw_node_bounds(child, frame, frame_ctx, (x, y), depth + 1, renderer_ctx)?;
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
                let geometry = self.geometry.resolve_with_defaults(
                    frame,
                    frame_ctx.width as f32 * 0.05,
                    frame_ctx.height as f32 * 0.05,
                    frame_ctx.width as f32 * 0.9,
                    frame_ctx.height as f32 * 0.9,
                    0.0,
                    0.0,
                );
                let bounds = geometry.rect();

                let mut paint = Paint::default();
                paint.set_anti_alias(true);
                paint.set_style(PaintStyle::Stroke);
                paint.set_stroke_width(2.0);
                paint.set_color(Color::from_argb(180, 120, 220, 255));

                renderer_ctx.canvas().draw_rect(bounds, &paint);

                if let Some(root_node) = self.root_node {
                    self.draw_node_bounds(
                        root_node,
                        frame,
                        frame_ctx,
                        (geometry.left(), geometry.top()),
                        0,
                        renderer_ctx,
                    )?;
                }

                Ok(())
            })
    }
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

fn draw_layout_content(
    content: &LayoutContent,
    rect: skia_safe::Rect,
    frame: u32,
    frame_ctx: &FrameContext,
    renderer_ctx: &mut RendererContext,
) -> Result<(), RenderError> {
    match content {
        LayoutContent::Shape(clip) => {
            let mut clip = clip.clone();
            clip.geometry = geometry_from_rect(rect);
            clip.draw(frame, frame_ctx, renderer_ctx)
        }
        LayoutContent::Text(clip) => {
            let mut clip = clip.clone();
            clip.geometry = geometry_from_rect(rect);
            clip.draw(frame, frame_ctx, renderer_ctx)
        }
        LayoutContent::Image(clip) => {
            let mut clip = clip.clone();
            clip.geometry = geometry_from_rect(rect);
            clip.draw(frame, frame_ctx, renderer_ctx)
        }
        LayoutContent::Video(clip) => {
            let mut clip = clip.clone();
            clip.geometry = geometry_from_rect(rect);
            clip.draw(frame, frame_ctx, renderer_ctx)
        }
    }
}

#[cfg(test)]
mod tests {
    use skia_safe::BlendMode;
    use taffy::prelude::{AvailableSpace, Size, Style, TaffyTree, length};

    use super::{LayoutClip, LayoutContent, LayoutNodeContext};
    use crate::clip::{
        Clip, ClipGeometry, ClipMeta,
        shape::{ShapeClip, ShapeKind},
        style::{
            BaseStyle, Fill, RectStyle, StyleProperty, StyleValue, TransformStyle,
        },
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
            shadow: None,
            clip_radius: [literal(0.0), literal(0.0), literal(0.0), literal(0.0)],
            transform: TransformStyle {
                translate: [literal(0.0), literal(0.0)],
                scale: [literal(1.0), literal(1.0)],
                rotation: literal(0.0),
                skew: [literal(0.0), literal(0.0)],
                origin: [literal(0.0), literal(0.0)],
            },
            alignment: [literal(0.0), literal(0.0)],
        }
    }

    fn shape_content() -> LayoutContent {
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
                    color: [literal(5), literal(150), literal(220), literal(255)],
                }),
                stroke: None,
            }),
        })
    }

    #[test]
    fn layout_clip_draws_taffy_computed_child_bounds() {
        let mut tree = TaffyTree::new();
        let child = tree
            .new_leaf_with_context(
                Style {
                    size: Size {
                        width: length(20.0),
                        height: length(10.0),
                    },
                    ..Default::default()
                },
                LayoutNodeContext {
                    id: Some("child".to_owned()),
                    content: Some(shape_content()),
                },
            )
            .expect("child");
        let root = tree
            .new_with_children(
                Style {
                    size: Size {
                        width: length(60.0),
                        height: length(40.0),
                    },
                    ..Default::default()
                },
                &[child],
            )
            .expect("root");
        tree.set_node_context(
            root,
            Some(LayoutNodeContext {
                id: Some("root".to_owned()),
                content: None,
            }),
        )
        .expect("root context");

        let mut clip = LayoutClip {
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
            tree,
            root_node: Some(root),
        };
        clip.compute_layout(Size {
            width: AvailableSpace::Definite(80.0),
            height: AvailableSpace::Definite(60.0),
        })
        .expect("layout");

        let mut renderer_ctx =
            RendererContext::new(120, 120, Rational::new(30, 1)).expect("renderer");
        renderer_ctx.clear();
        let frame_ctx = FrameContext {
            frame: 0,
            time_seconds: 0.0,
            width: 120,
            height: 120,
            device_scale: 1.0,
        };

        clip.draw(0, &frame_ctx, &mut renderer_ctx)
            .expect("layout draw");

        let pixels = read_surface_rgba(&mut renderer_ctx).expect("readback");
        let idx = |x: usize, y: usize| (y * 120 + x) * 4;

        assert_eq!(&pixels[idx(15, 15)..idx(15, 15) + 4], &[5, 150, 220, 255]);
        assert!(pixels[idx(11, 11) + 3] > 0);
    }
}
