use skia_safe::{AlphaType, Canvas, Color, ColorType, ImageInfo, Rect, Surface, surfaces};

use crate::backend::{FrameImage, FrameProvider, RenderError, Renderer, pixel_len};
use crate::compile::{
    CompiledBaseStyle, CompiledLayerItem, CompiledOperation, CompiledOperationKind,
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
}

// SAFETY: renderer instances are never shared between threads and are created per worker.
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

        let frame_state = timeline.resolve_frame_context(frame)?;

        {
            let canvas = self.surface.canvas();
            let bg = timeline.canvas.background;
            canvas.clear(Color::from_argb(bg[3], bg[0], bg[1], bg[2]));

            for layer in &timeline.layers {
                for item in &layer.items {
                    draw_layer_item(canvas, timeline, frame, item, &frame_state, provider, 1.0)?;
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

            let bounds = resolved_bounds(&operation.style.base, frame_state);
            let opacity =
                (parent_opacity * operation.resolved_opacity(frame_state)).clamp(0.0, 1.0);
            if opacity <= 0.0 {
                return Ok(());
            }

            canvas.save();
            apply_transform(canvas, &operation.style.base, frame_state);

            if let Some(mask_item) = node.mask.as_deref() {
                mask::render_masked(canvas, bounds, |canvas| {
                    draw_clip_content(
                        canvas,
                        timeline,
                        frame,
                        operation,
                        frame_state,
                        provider,
                        opacity,
                    )?;
                    let _ = mask_item;
                    Ok(())
                })?;
            } else {
                if let Some(shadow) = &operation.style.base.shadow {
                    let _ = shadow::build_shadow_paint(
                        shadow,
                        frame_state,
                        opacity,
                        operation.style.base.blend_mode,
                    );
                }
                draw_clip_content(
                    canvas,
                    timeline,
                    frame,
                    operation,
                    frame_state,
                    provider,
                    opacity,
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
            canvas.save_layer_alpha_f(Some(bounds), opacity);

            for child in &group.items {
                draw_layer_item(canvas, timeline, frame, child, frame_state, provider, 1.0)?;
            }

            canvas.restore();
            canvas.restore();
            Ok(())
        }
    }
}

fn draw_clip_content(
    canvas: &Canvas,
    timeline: &CompiledTimeline,
    frame: u64,
    operation: &CompiledOperation,
    frame_state: &RuntimeFrameContext,
    provider: &mut dyn FrameProvider,
    opacity: f32,
) -> Result<(), RenderError> {
    let bounds = resolved_bounds(&operation.style.base, frame_state);

    if let CompiledOperationKind::Layout(layout_clip) = &operation.kind {
        let _ = layout::compute_layout_boxes(&layout_clip.root, frame_state, bounds)?;
        return Ok(());
    }

    let frame_image = load_frame_image(timeline, frame, operation, provider)?;
    primitives::draw_operation_content(
        canvas,
        operation,
        frame_state,
        frame_image.as_ref(),
        bounds,
        opacity,
    )
}

fn load_frame_image(
    timeline: &CompiledTimeline,
    frame: u64,
    operation: &CompiledOperation,
    provider: &mut dyn FrameProvider,
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
            let source_frame = operation
                .source_frame_at(frame, u64::MAX / 4)
                .unwrap_or(operation.local_frame(frame));
            provider
                .video_frame(source.id.as_str(), source_frame)
                .map_err(RenderError::from)
        }
        _ => Ok(None),
    }
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
