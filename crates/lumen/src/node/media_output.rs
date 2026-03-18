use std::sync::Arc;

use skia_safe::{Paint, Rect, SamplingOptions};

use crate::{
    node::{
        NodeId, NodeResult, PortRef,
        pixel_utils::{make_skia_image, render_with_skia},
    },
    raster::{BitmapFrame, RasterFrame, RectI},
    render::context::RenderContext,
};
use lumen_macros::{Node, node_impl};

#[derive(Debug, Clone, Node)]
pub struct MediaOutput {
    pub id: NodeId,

    #[input(kind = Raster)]
    pub source: PortRef,
}

impl Default for MediaOutput {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            source: PortRef::empty(),
        }
    }
}

#[node_impl]
impl MediaOutput {
    #[output(port = "output", kind = Raster)]
    fn eval_output(&self, ctx: &mut RenderContext<'_>) -> crate::Result<RasterFrame> {
        let source = match ctx.eval_once(self.source.clone())? {
            NodeResult::Raster(raster) => raster,
            NodeResult::Vector(vector) => {
                todo!("return proper error type")
            }
            NodeResult::None => {
                todo!("return proper error type")
            }
        };

        let output_rect = RectI::from_size(
            ctx.renderer.composition.render_settings.width,
            ctx.renderer.composition.render_settings.height,
        );
        let (target_w, target_h) = (output_rect.width, output_rect.height);
        let (source_w, source_h) = source.dimensions();
        let source_format = source.format_rect();
        let source_data = source.data_rect();

        if source_w == target_w
            && source_h == target_h
            && source_format == output_rect
            && source_data == output_rect
        {
            return Ok(source.to_bitmap()?);
        }

        if target_w == 0 || target_h == 0 {
            return Ok(RasterFrame::Bitmap(BitmapFrame::with_domain(
                Arc::new(Vec::new()),
                0,
                0,
                output_rect,
                output_rect,
            )));
        }

        let (width, height) = source.dimensions();

        let surface = source.promote_to_surface(ctx.renderer.surface_pool)?;
        surface.surface.surface_mut().resi

        let output = render_with_skia(target_w, target_h, Some(ctx), |canvas| {
            draw_frame_image(
                canvas,
                &image,
                width,
                height,
                source_format,
                source_data,
                output_rect,
                None,
            );
        });

        Ok(RasterFrame::Bitmap(
            BitmapFrame::with_domain(
                Arc::new(output),
                target_w,
                target_h,
                output_rect,
                output_rect,
            )
            .with_alpha_mode(source_alpha),
        ))
    }
}

fn draw_frame_image(
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
