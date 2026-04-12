use std::rc::Rc;

use skia_safe::{Paint, Rect, SamplingOptions};

use crate::{
    error::{LumenError, RenderError},
    node::{
        NodeId, NodeKind, NodeProperty, NodeResult, PortRef,
        compositing::BlendMode,
        pixel_utils::{ClearMode, render_to_surface_ephemeral},
    },
    raster::{RasterFrame, RectI},
    render::RenderContext,
};
use lumen_macros::{Node, node_impl};

#[derive(Debug, Clone, Node)]
pub struct Merge {
    pub id: NodeId,

    #[property(expected = Float)]
    pub opacity: NodeProperty,
    #[property(expected = Int)]
    pub blend_mode: NodeProperty,

    #[input(kind = Raster)]
    pub base: PortRef,
    #[input(kind = Raster)]
    pub overlay: PortRef,
    #[input(kind = Raster, optional)]
    pub mask: PortRef,
}

impl Default for Merge {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            opacity: NodeProperty::Float(1.0),
            blend_mode: NodeProperty::Int(BlendMode::Normal as i64),
            base: PortRef::empty(),
            overlay: PortRef::empty(),
            mask: PortRef::empty(),
        }
    }
}

#[node_impl]
impl Merge {
    #[output(port = "output", kind = Raster)]
    fn eval_output(&self, ctx: &mut RenderContext) -> crate::Result<RasterFrame> {
        let opacity = self.resolve_opacity(ctx)? as f32;
        let blend_mode =
            BlendMode::try_from(self.resolve_blend_mode(ctx)? as usize).map_err(|err| {
                LumenError::Render(RenderError::NodeEvaluation {
                    frame: ctx.frame,
                    node_id: self.node_id(),
                    node_kind: "merge",
                    details: err.into(),
                })
            })?;

        let base_result = ctx.eval(self.base.clone())?;
        let base = base_result.as_raster()?;

        if opacity <= 0.0 {
            return base.snapshot();
        }

        let overlay_result = ctx.eval(self.overlay.clone())?;
        let overlay = overlay_result.as_raster()?;
        let mask_result = if !self.mask.is_empty() {
            Some(ctx.eval(self.mask.clone())?)
        } else {
            None
        };
        let mask = mask_result.as_ref().map(|v| v.as_raster()).transpose()?;

        let base_alpha = base.alpha_mode();
        let base_format = base.format_rect();
        let base_data = base.data_rect();
        let overlay_format = overlay.format_rect();
        let overlay_data = overlay.data_rect();

        let (base_image, base_w, base_h) = match base.image_parts() {
            Some(parts) => parts,
            None => {
                let out_format = union_rect(base_format, overlay_format);
                let out_data = union_rect(base_data, overlay_data);
                let render_w = out_data.width.max(1);
                let render_h = out_data.height.max(1);
                return RasterFrame::transparent(
                    render_w, render_h, out_format, out_data, base_alpha,
                );
            }
        };

        let (overlay_image, overlay_w, overlay_h) = match overlay.image_parts() {
            Some(parts) => parts,
            None => return base.snapshot(),
        };

        let mask_parts = if let Some(raster) = mask {
            raster
                .image_parts()
                .map(|(img, w, h)| (img, w, h, raster.format_rect(), raster.data_rect()))
        } else {
            None
        };

        let out_format = union_rect(base_format, overlay_format);
        let mut out_data = union_rect(base_data, overlay_data);
        out_data =
            out_format
                .intersect(&out_data)
                .unwrap_or(RectI::new(out_format.x, out_format.y, 0, 0));
        let render_w = out_data.width.max(1);
        let render_h = out_data.height.max(1);

        let opacity = opacity.clamp(0.0, 1.0);
        let skia_blend: skia_safe::BlendMode = blend_mode.into();
        let output_rect = RectI::from_size(
            ctx.renderer.composition.render_settings.width,
            ctx.renderer.composition.render_settings.height,
        );
        let feeds_media_output = feeds_media_output(ctx, self.node_id());

        if mask_parts.is_none()
            && feeds_media_output
            && base_format == output_rect
            && base_data == output_rect
        {
            if let Ok(NodeResult::Raster(RasterFrame::Surface(mut base_surface))) =
                Rc::try_unwrap(base_result)
            {
                let canvas = base_surface.surface.surface_mut().canvas();
                canvas.restore_to_count(1);
                canvas.reset_matrix();

                let mut overlay_paint = Paint::default();
                overlay_paint.set_blend_mode(skia_blend);
                overlay_paint.set_alpha_f(opacity);
                draw_frame_image(
                    canvas,
                    &overlay_image,
                    overlay_w,
                    overlay_h,
                    overlay_format,
                    overlay_data,
                    output_rect,
                    Some(&overlay_paint),
                );
                if !can_skip_snapshot_refresh(
                    ctx,
                    self.node_id(),
                    output_rect,
                    output_rect,
                    output_rect,
                ) {
                    base_surface.refresh_snapshot();
                }
                return Ok(RasterFrame::Surface(base_surface));
            }

            return render_to_surface_ephemeral(
                output_rect.width.max(1),
                output_rect.height.max(1),
                ctx,
                output_rect,
                output_rect,
                base_alpha,
                ClearMode::Transparent,
                |canvas| {
                    draw_frame_image(
                        canvas,
                        &base_image,
                        base_w,
                        base_h,
                        base_format,
                        base_data,
                        output_rect,
                        None,
                    );

                    let mut overlay_paint = Paint::default();
                    overlay_paint.set_blend_mode(skia_blend);
                    overlay_paint.set_alpha_f(opacity);
                    draw_frame_image(
                        canvas,
                        &overlay_image,
                        overlay_w,
                        overlay_h,
                        overlay_format,
                        overlay_data,
                        output_rect,
                        Some(&overlay_paint),
                    );
                },
            );
        }

        if mask_parts.is_none()
            && base_format == out_format
            && base_data == out_data
            && let Ok(NodeResult::Raster(RasterFrame::Surface(mut base_surface))) =
                Rc::try_unwrap(base_result)
        {
            let canvas = base_surface.surface.surface_mut().canvas();
            canvas.restore_to_count(1);
            canvas.reset_matrix();

            let mut overlay_paint = Paint::default();
            overlay_paint.set_blend_mode(skia_blend);
            overlay_paint.set_alpha_f(opacity);
            draw_frame_image(
                canvas,
                &overlay_image,
                overlay_w,
                overlay_h,
                overlay_format,
                overlay_data,
                out_data,
                Some(&overlay_paint),
            );
            if !can_skip_snapshot_refresh(ctx, self.node_id(), out_format, out_data, output_rect) {
                base_surface.refresh_snapshot();
            }
            return Ok(RasterFrame::Surface(base_surface));
        }

        render_to_surface_ephemeral(
            render_w,
            render_h,
            ctx,
            out_format,
            out_data,
            base_alpha,
            ClearMode::Transparent,
            |canvas| {
                draw_frame_image(
                    canvas,
                    &base_image,
                    base_w,
                    base_h,
                    base_format,
                    base_data,
                    out_data,
                    None,
                );

                if let Some((ref mask_img, mask_w, mask_h, mask_format, mask_data)) = mask_parts {
                    canvas.save_layer_alpha(None, 255);

                    let mut overlay_paint = Paint::default();
                    overlay_paint.set_blend_mode(skia_blend);
                    overlay_paint.set_alpha_f(opacity);
                    draw_frame_image(
                        canvas,
                        &overlay_image,
                        overlay_w,
                        overlay_h,
                        overlay_format,
                        overlay_data,
                        out_data,
                        Some(&overlay_paint),
                    );

                    let mut mask_paint = Paint::default();
                    mask_paint.set_blend_mode(skia_safe::BlendMode::DstIn);
                    draw_frame_image(
                        canvas,
                        mask_img,
                        mask_w,
                        mask_h,
                        mask_format,
                        mask_data,
                        out_data,
                        Some(&mask_paint),
                    );

                    canvas.restore();
                } else {
                    let mut overlay_paint = Paint::default();
                    overlay_paint.set_blend_mode(skia_blend);
                    overlay_paint.set_alpha_f(opacity);
                    draw_frame_image(
                        canvas,
                        &overlay_image,
                        overlay_w,
                        overlay_h,
                        overlay_format,
                        overlay_data,
                        out_data,
                        Some(&overlay_paint),
                    );
                }
            },
        )
    }
}

fn can_skip_snapshot_refresh<S, M>(
    ctx: &RenderContext<'_, S, M>,
    node_id: NodeId,
    out_format: RectI,
    out_data: RectI,
    output_rect: RectI,
) -> bool
where
    S: crate::render::surface::SurfacePool,
    M: crate::media::MediaStore,
{
    if out_format != output_rect || out_data != output_rect {
        return false;
    }

    feeds_media_output(ctx, node_id)
}

fn feeds_media_output<S, M>(ctx: &RenderContext<'_, S, M>, node_id: NodeId) -> bool
where
    S: crate::render::surface::SurfacePool,
    M: crate::media::MediaStore,
{
    let graph = &ctx.renderer.composition.graph;
    if graph.outgoing_connection_count(node_id) != 1 {
        return false;
    }

    graph
        .connections
        .iter()
        .find(|connection| connection.from_node == node_id)
        .and_then(|connection| graph.nodes.get(&connection.to_node))
        .is_some_and(|node| matches!(node, NodeKind::MediaOutput(_)))
}

pub(crate) fn union_rect(left: RectI, right: RectI) -> RectI {
    let min_x = left.x.min(right.x);
    let min_y = left.y.min(right.y);
    let max_x = left.right().max(right.right());
    let max_y = left.bottom().max(right.bottom());
    let width = (max_x - i64::from(min_x)).max(1) as u32;
    let height = (max_y - i64::from(min_y)).max(1) as u32;
    RectI::new(min_x, min_y, width, height)
}

pub(crate) fn draw_frame_image(
    canvas: &skia_safe::Canvas,
    image: &skia_safe::Image,
    storage_w: u32,
    storage_h: u32,
    format_rect: RectI,
    data_rect: RectI,
    target_rect: RectI,
    paint: Option<&Paint>,
) {
    if data_rect.width == 0
        || data_rect.height == 0
        || format_rect.width == 0
        || format_rect.height == 0
    {
        return;
    }
    let Some(clipped) = data_rect.intersect(&target_rect) else {
        return;
    };

    let sx = storage_w as f32 / format_rect.width as f32;
    let sy = storage_h as f32 / format_rect.height as f32;
    let src_x = (clipped.x - format_rect.x) as f32 * sx;
    let src_y = (clipped.y - format_rect.y) as f32 * sy;
    let src_w = clipped.width as f32 * sx;
    let src_h = clipped.height as f32 * sy;
    let dst_x = (clipped.x - target_rect.x) as f32;
    let dst_y = (clipped.y - target_rect.y) as f32;
    let dst_rect = Rect::from_xywh(dst_x, dst_y, clipped.width as f32, clipped.height as f32);
    let src_rect = Rect::from_xywh(src_x, src_y, src_w, src_h);
    let sampling = SamplingOptions::default();

    match paint {
        Some(paint) => {
            canvas.draw_image_rect_with_sampling_options(
                image,
                Some((&src_rect, skia_safe::canvas::SrcRectConstraint::Fast)),
                dst_rect,
                sampling,
                paint,
            );
        }
        None => {
            let default_paint = Paint::default();
            canvas.draw_image_rect_with_sampling_options(
                image,
                Some((&src_rect, skia_safe::canvas::SrcRectConstraint::Fast)),
                dst_rect,
                sampling,
                &default_paint,
            );
        }
    }
}
