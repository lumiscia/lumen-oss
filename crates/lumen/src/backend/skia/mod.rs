use std::collections::HashMap;

use skia_safe::canvas::SaveLayerRec;
use skia_safe::{AlphaType, Canvas, Color, ColorType, ImageInfo, Rect, Surface, surfaces};

use crate::backend::{FrameImage, FrameProvider, RenderError, Renderer, pixel_len};
use crate::compile::{
    CompiledBaseStyle, CompiledImage, CompiledLayerItem, CompiledLayoutNode,
    CompiledLayoutNodeKind, CompiledOperation, CompiledOperationKind, CompiledText,
    CompiledTimeline, RuntimeFrameContext,
};

mod layout;
mod mask;
mod primitives;
mod shadow;

pub struct SkiaRenderer {
    width: u32,
    height: u32,
    surface: Surface,
    video_frame_counts: HashMap<String, Option<u64>>,
}

// SAFETY: CPU raster Skia surfaces are !Send in the type system, but each renderer is created,
// owned, and used by exactly one worker thread. The orchestrator never shares a renderer across
// threads and never moves it after construction.
unsafe impl Send for SkiaRenderer {}

impl SkiaRenderer {
    pub fn new(width: u32, height: u32) -> Result<Self, RenderError> {
        let surface =
            surfaces::raster_n32_premul((width as i32, height as i32)).ok_or_else(|| {
                RenderError::RendererInit("failed to create raster surface".to_string())
            })?;
        Ok(Self {
            width,
            height,
            surface,
            video_frame_counts: HashMap::new(),
        })
    }
}

impl Renderer for SkiaRenderer {
    fn render_frame(
        &mut self,
        timeline: &CompiledTimeline,
        frame: u64,
        provider: &mut dyn FrameProvider,
    ) -> Result<Vec<u8>, RenderError> {
        if frame >= timeline.total_frames() {
            return Err(RenderError::FrameOutOfRange {
                frame,
                total_frames: timeline.total_frames(),
            });
        }

        let initial_frame_state = timeline.resolve_frame_context(frame)?;
        let layout_overrides = collect_layout_overrides(timeline, frame, &initial_frame_state)?;
        let frame_state = if layout_overrides.is_empty() {
            initial_frame_state
        } else {
            timeline.resolve_frame_context_with_overrides(frame, &layout_overrides)?
        };

        {
            let canvas = self.surface.canvas();
            let bg = timeline.canvas.background;
            canvas.clear(Color::from_argb(bg[3], bg[0], bg[1], bg[2]));

            for layer in &timeline.layers {
                for item in &layer.items {
                    draw_layer_item(
                        canvas,
                        timeline,
                        frame,
                        item,
                        &frame_state,
                        provider,
                        1.0,
                        &mut self.video_frame_counts,
                        false,
                    )?;
                }
            }
        }

        let mut pixels = vec![0; pixel_len(self.width, self.height)?];
        let info = ImageInfo::new(
            (self.width as i32, self.height as i32),
            ColorType::RGBA8888,
            AlphaType::Unpremul,
            None,
        );
        let ok = self.surface.read_pixels(
            &info,
            pixels.as_mut_slice(),
            (self.width as usize) * 4,
            (0, 0),
        );
        if !ok {
            return Err(RenderError::Failed(
                "failed to read pixels from skia surface".to_string(),
            ));
        }

        Ok(pixels)
    }
}

fn draw_layer_item(
    canvas: &Canvas,
    timeline: &CompiledTimeline,
    frame: u64,
    item: &CompiledLayerItem,
    frame_state: &RuntimeFrameContext,
    provider: &mut dyn FrameProvider,
    parent_opacity: f32,
    video_frame_counts: &mut HashMap<String, Option<u64>>,
    render_mask_items: bool,
) -> Result<(), RenderError> {
    match item {
        CompiledLayerItem::Clip(node) => {
            let Some(operation) = timeline.operation(node.operation_index) else {
                return Ok(());
            };

            if (!render_mask_items && operation.is_mask)
                || !operation.contains_frame(frame)
                || !operation.style.base.visible
            {
                return Ok(());
            }

            let bounds = resolved_bounds(&operation.style.base, frame_state);
            let opacity =
                (parent_opacity * operation.resolved_opacity(frame_state)).clamp(0.0, 1.0);
            if opacity <= 0.0 {
                return Ok(());
            }

            canvas.save();
            apply_transform(canvas, &operation.style.base, frame_state);

            if let Some(mask_item) = node.mask.as_deref() {
                let simple_mask = simple_mask_geometry(timeline, frame, mask_item, frame_state);
                mask::render_masked(canvas, bounds, simple_mask, |canvas, phase| match phase {
                    mask::MaskPhase::Content => draw_clip_with_shadow(
                        canvas,
                        timeline,
                        frame,
                        operation,
                        frame_state,
                        provider,
                        opacity,
                        video_frame_counts,
                    ),
                    mask::MaskPhase::Mask => draw_layer_item(
                        canvas,
                        timeline,
                        frame,
                        mask_item,
                        frame_state,
                        provider,
                        1.0,
                        video_frame_counts,
                        true,
                    ),
                })?;
            } else {
                draw_clip_with_shadow(
                    canvas,
                    timeline,
                    frame,
                    operation,
                    frame_state,
                    provider,
                    opacity,
                    video_frame_counts,
                )?;
            }

            canvas.restore();
            Ok(())
        }
        CompiledLayerItem::Group(group) => {
            if !group.style.visible {
                return Ok(());
            }
            let opacity =
                (parent_opacity * group.style.opacity.resolve(frame_state)).clamp(0.0, 1.0);
            if opacity <= 0.0 {
                return Ok(());
            }

            let bounds = resolved_bounds(&group.style, frame_state);

            canvas.save();
            apply_transform(canvas, &group.style, frame_state);

            if let Some(mask_item) = group.mask.as_deref() {
                let simple_mask = simple_mask_geometry(timeline, frame, mask_item, frame_state);
                mask::render_masked(canvas, bounds, simple_mask, |canvas, phase| match phase {
                    mask::MaskPhase::Content => draw_group_contents(
                        canvas,
                        timeline,
                        frame,
                        &group.items,
                        frame_state,
                        provider,
                        opacity,
                        video_frame_counts,
                    ),
                    mask::MaskPhase::Mask => draw_layer_item(
                        canvas,
                        timeline,
                        frame,
                        mask_item,
                        frame_state,
                        provider,
                        1.0,
                        video_frame_counts,
                        true,
                    ),
                })?;
            } else {
                draw_group_contents(
                    canvas,
                    timeline,
                    frame,
                    &group.items,
                    frame_state,
                    provider,
                    opacity,
                    video_frame_counts,
                )?;
            }

            canvas.restore();
            Ok(())
        }
    }
}

fn draw_group_contents(
    canvas: &Canvas,
    timeline: &CompiledTimeline,
    frame: u64,
    items: &[CompiledLayerItem],
    frame_state: &RuntimeFrameContext,
    provider: &mut dyn FrameProvider,
    opacity: f32,
    video_frame_counts: &mut HashMap<String, Option<u64>>,
) -> Result<(), RenderError> {
    canvas.save_layer_alpha_f(None, opacity);
    for child in items {
        draw_layer_item(
            canvas,
            timeline,
            frame,
            child,
            frame_state,
            provider,
            1.0,
            video_frame_counts,
            false,
        )?;
    }
    canvas.restore();
    Ok(())
}

fn draw_clip_with_shadow(
    canvas: &Canvas,
    timeline: &CompiledTimeline,
    frame: u64,
    operation: &CompiledOperation,
    frame_state: &RuntimeFrameContext,
    provider: &mut dyn FrameProvider,
    opacity: f32,
    video_frame_counts: &mut HashMap<String, Option<u64>>,
) -> Result<(), RenderError> {
    if let Some(shadow_style) = &operation.style.base.shadow {
        let bounds = resolved_bounds(&operation.style.base, frame_state);
        let shadow_paint = shadow::build_shadow_paint(
            shadow_style,
            frame_state,
            opacity,
            operation.style.base.blend_mode,
        );
        let layer = SaveLayerRec::default().bounds(&bounds).paint(&shadow_paint);
        canvas.save_layer(&layer);
        draw_clip_content(
            canvas,
            timeline,
            frame,
            operation,
            frame_state,
            provider,
            opacity,
            video_frame_counts,
        )?;
        canvas.restore();
        return Ok(());
    }

    draw_clip_content(
        canvas,
        timeline,
        frame,
        operation,
        frame_state,
        provider,
        opacity,
        video_frame_counts,
    )
}

fn draw_clip_content(
    canvas: &Canvas,
    timeline: &CompiledTimeline,
    frame: u64,
    operation: &CompiledOperation,
    frame_state: &RuntimeFrameContext,
    provider: &mut dyn FrameProvider,
    opacity: f32,
    video_frame_counts: &mut HashMap<String, Option<u64>>,
) -> Result<(), RenderError> {
    let bounds = resolved_bounds(&operation.style.base, frame_state);

    if let CompiledOperationKind::Layout(layout_clip) = &operation.kind {
        return draw_layout_clip(
            canvas,
            timeline,
            frame,
            operation,
            &layout_clip.root,
            frame_state,
            provider,
            opacity,
            video_frame_counts,
            bounds,
        );
    }

    let frame_image = load_frame_image(timeline, frame, operation, provider, video_frame_counts)?;
    primitives::draw_operation_content(
        canvas,
        operation,
        frame_state,
        frame_image.as_ref(),
        bounds,
        opacity,
    )
}

fn draw_layout_clip(
    canvas: &Canvas,
    timeline: &CompiledTimeline,
    frame: u64,
    operation: &CompiledOperation,
    root: &CompiledLayoutNode,
    frame_state: &RuntimeFrameContext,
    provider: &mut dyn FrameProvider,
    opacity: f32,
    video_frame_counts: &mut HashMap<String, Option<u64>>,
    bounds: Rect,
) -> Result<(), RenderError> {
    let layout_boxes = layout::compute_layout_boxes(root, frame_state, bounds)?;
    draw_layout_node(
        canvas,
        timeline,
        frame,
        operation,
        root,
        &layout_boxes,
        frame_state,
        provider,
        opacity,
        video_frame_counts,
    )
}

fn draw_layout_node(
    canvas: &Canvas,
    timeline: &CompiledTimeline,
    frame: u64,
    operation: &CompiledOperation,
    node: &CompiledLayoutNode,
    layout_boxes: &HashMap<String, layout::LayoutBox>,
    frame_state: &RuntimeFrameContext,
    provider: &mut dyn FrameProvider,
    opacity: f32,
    video_frame_counts: &mut HashMap<String, Option<u64>>,
) -> Result<(), RenderError> {
    let Some(layout_box) = layout_boxes.get(node.id.as_str()) else {
        return Ok(());
    };
    let node_bounds = Rect::from_xywh(
        layout_box.x,
        layout_box.y,
        layout_box.width.max(0.0),
        layout_box.height.max(0.0),
    );

    match &node.kind {
        CompiledLayoutNodeKind::Container { children } => {
            for child in children {
                draw_layout_node(
                    canvas,
                    timeline,
                    frame,
                    operation,
                    child,
                    layout_boxes,
                    frame_state,
                    provider,
                    opacity,
                    video_frame_counts,
                )?;
            }
            Ok(())
        }
        CompiledLayoutNodeKind::Text { content } => {
            let mut text_operation = operation.clone();
            text_operation.kind = CompiledOperationKind::Text(CompiledText {
                content: content.clone(),
            });
            primitives::draw_operation_content(
                canvas,
                &text_operation,
                frame_state,
                None,
                node_bounds,
                opacity,
            )
        }
        CompiledLayoutNodeKind::Image { source_index } => {
            let mut image_operation = operation.clone();
            image_operation.kind = CompiledOperationKind::Image(CompiledImage {
                source_index: *source_index,
            });
            let frame_image = load_frame_image(
                timeline,
                frame,
                &image_operation,
                provider,
                video_frame_counts,
            )?;
            primitives::draw_operation_content(
                canvas,
                &image_operation,
                frame_state,
                frame_image.as_ref(),
                node_bounds,
                opacity,
            )
        }
    }
}

fn collect_layout_overrides(
    timeline: &CompiledTimeline,
    frame: u64,
    frame_state: &RuntimeFrameContext,
) -> Result<HashMap<String, f32>, RenderError> {
    let mut overrides = HashMap::new();
    for layer in &timeline.layers {
        for item in &layer.items {
            collect_layout_overrides_from_item(timeline, frame, item, frame_state, &mut overrides)?;
        }
    }
    Ok(overrides)
}

fn collect_layout_overrides_from_item(
    timeline: &CompiledTimeline,
    frame: u64,
    item: &CompiledLayerItem,
    frame_state: &RuntimeFrameContext,
    overrides: &mut HashMap<String, f32>,
) -> Result<(), RenderError> {
    match item {
        CompiledLayerItem::Clip(node) => {
            let Some(operation) = timeline.operation(node.operation_index) else {
                return Ok(());
            };
            if operation.is_mask
                || !operation.contains_frame(frame)
                || !operation.style.base.visible
            {
                return Ok(());
            }
            if let CompiledOperationKind::Layout(layout_clip) = &operation.kind {
                let bounds = resolved_bounds(&operation.style.base, frame_state);
                let boxes = layout::compute_layout_boxes(&layout_clip.root, frame_state, bounds)?;
                for (id, layout_box) in boxes {
                    overrides.insert(format!("{id}.x"), layout_box.x);
                    overrides.insert(format!("{id}.y"), layout_box.y);
                    overrides.insert(format!("{id}.width"), layout_box.width);
                    overrides.insert(format!("{id}.height"), layout_box.height);
                }
            }
        }
        CompiledLayerItem::Group(group) => {
            if !group.style.visible {
                return Ok(());
            }
            for child in &group.items {
                collect_layout_overrides_from_item(timeline, frame, child, frame_state, overrides)?;
            }
        }
    }

    Ok(())
}

fn load_frame_image(
    timeline: &CompiledTimeline,
    frame: u64,
    operation: &CompiledOperation,
    provider: &mut dyn FrameProvider,
    video_frame_counts: &mut HashMap<String, Option<u64>>,
) -> Result<Option<FrameImage>, RenderError> {
    match &operation.kind {
        CompiledOperationKind::Image(image) => {
            let Some(source) = timeline.source(image.source_index) else {
                return Ok(None);
            };
            provider
                .image(source.id.as_str())
                .map_err(RenderError::from)
        }
        CompiledOperationKind::Video(video) => {
            let Some(source) = timeline.source(video.source_index) else {
                return Ok(None);
            };
            let source_frame_count =
                video_frame_count(source.id.as_str(), provider, video_frame_counts)?;
            let source_frame = match source_frame_count {
                Some(source_length) if source_length > 0 => operation
                    .source_frame_at(frame, source_length)
                    .unwrap_or(operation.local_frame(frame)),
                _ => operation.local_frame(frame),
            };
            provider
                .video_frame(source.id.as_str(), source_frame)
                .map_err(RenderError::from)
        }
        _ => Ok(None),
    }
}

fn video_frame_count(
    source_id: &str,
    provider: &mut dyn FrameProvider,
    video_frame_counts: &mut HashMap<String, Option<u64>>,
) -> Result<Option<u64>, RenderError> {
    if let Some(count) = video_frame_counts.get(source_id) {
        return Ok(*count);
    }

    let count = provider.video_frame_count(source_id)?;
    video_frame_counts.insert(source_id.to_string(), count);
    Ok(count)
}

fn simple_mask_geometry(
    timeline: &CompiledTimeline,
    frame: u64,
    mask_item: &CompiledLayerItem,
    frame_state: &RuntimeFrameContext,
) -> Option<mask::SimpleMaskGeometry> {
    let CompiledLayerItem::Clip(mask_clip) = mask_item else {
        return None;
    };
    let operation = timeline.operation(mask_clip.operation_index)?;
    if !operation.contains_frame(frame) || !operation.style.base.visible {
        return None;
    }
    if !is_axis_aligned_transform(&operation.style.base, frame_state) {
        return None;
    }
    // Only use simple clip when mask is guaranteed to be fully opaque
    let opacity = operation.style.base.opacity.resolve(frame_state);
    if (opacity - 1.0).abs() > f32::EPSILON {
        return None;
    }
    let blur = operation.style.base.blur.resolve(frame_state);
    if blur > 0.0 {
        return None;
    }
    let bounds = resolved_bounds(&operation.style.base, frame_state);
    match &operation.kind {
        CompiledOperationKind::Solid => {
            if let Some(fill) = operation.style.fill {
                if fill[3] < 255 {
                    return None;
                }
            } else {
                return None;
            }
            Some(mask::SimpleMaskGeometry::Rect {
                bounds,
                corner_radius: resolved_corner_radius(operation, frame_state),
            })
        }
        CompiledOperationKind::Shape(crate::compile::CompiledShape {
            geometry: crate::model::ShapeGeometry::Rect,
        }) => {
            if let Some(fill) = operation.style.fill {
                if fill[3] < 255 {
                    return None;
                }
            } else {
                return None;
            }
            Some(mask::SimpleMaskGeometry::Rect {
                bounds,
                corner_radius: resolved_corner_radius(operation, frame_state),
            })
        }
        CompiledOperationKind::Shape(crate::compile::CompiledShape {
            geometry: crate::model::ShapeGeometry::Ellipse,
        }) => {
            if let Some(fill) = operation.style.fill {
                if fill[3] < 255 {
                    return None;
                }
            } else {
                return None;
            }
            Some(mask::SimpleMaskGeometry::Ellipse { bounds })
        }
        _ => None,
    }
}

fn resolved_corner_radius(operation: &CompiledOperation, state: &RuntimeFrameContext) -> [f32; 4] {
    let Some(corner_radius) = &operation.style.corner_radius else {
        return [0.0; 4];
    };
    [
        corner_radius[0].resolve(state).max(0.0),
        corner_radius[1].resolve(state).max(0.0),
        corner_radius[2].resolve(state).max(0.0),
        corner_radius[3].resolve(state).max(0.0),
    ]
}

fn is_axis_aligned_transform(base: &CompiledBaseStyle, state: &RuntimeFrameContext) -> bool {
    let transform = base.transform.resolve(state);
    (transform.rotation == 0.0)
        && (transform.skew_x == 0.0)
        && (transform.skew_y == 0.0)
        && (transform.scale_x == 1.0)
        && (transform.scale_y == 1.0)
}

fn resolved_bounds(base: &CompiledBaseStyle, state: &RuntimeFrameContext) -> Rect {
    let transform = base.transform.resolve(state);
    let width = transform.width.max(0.0);
    let height = transform.height.max(0.0);

    let align_x = base.alignment[0].resolve(state).clamp(-1.0, 1.0);
    let align_y = base.alignment[1].resolve(state).clamp(-1.0, 1.0);

    let left = transform.x - (align_x + 1.0) * 0.5 * width;
    let top = transform.y - (align_y + 1.0) * 0.5 * height;
    Rect::from_xywh(left, top, width, height)
}

fn apply_transform(canvas: &Canvas, base: &CompiledBaseStyle, state: &RuntimeFrameContext) {
    let transform = base.transform.resolve(state);
    let bounds = resolved_bounds(base, state);

    let anchor_x = bounds.left + transform.anchor_x * bounds.width();
    let anchor_y = bounds.top + transform.anchor_y * bounds.height();

    canvas.translate((anchor_x, anchor_y));
    canvas.rotate(transform.rotation, None);
    canvas.skew((transform.skew_x, transform.skew_y));
    canvas.scale((transform.scale_x, transform.scale_y));
    canvas.translate((-anchor_x, -anchor_y));
}
