use std::sync::Arc;

use skia_safe::{Paint, Rect, SamplingOptions};

use crate::{
    error::LumenError,
    node::{
        InputPortDef, NodeEval, NodeInputs, OutputPortDef, PortKind, PortValue,
        pixel_utils::{make_skia_image, render_with_skia},
    },
    raster::{BitmapFrame, RasterFrame, RectI},
    render::RenderContext,
};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MediaOutput;

impl NodeEval for MediaOutput {
    fn input_port_defs(&self) -> &'static [InputPortDef] {
        &[InputPortDef {
            name: "source",
            kind: PortKind::RasterFrame,
            optional: false,
        }]
    }

    fn output_port_defs(&self) -> &'static [OutputPortDef] {
        &[OutputPortDef {
            name: "output",
            kind: PortKind::RasterFrame,
        }]
    }

    fn evaluate(
        &self,
        inputs: &NodeInputs,
        ctx: &mut RenderContext,
    ) -> Result<PortValue, LumenError> {
        let source = inputs.get_raster("source")?;
        let output_rect = ctx.request.output_rect;
        let (target_w, target_h) = (output_rect.width, output_rect.height);
        let (source_w, source_h) = source.dimensions();
        let source_format = source.format_rect();
        let source_data = source.data_rect();

        if source_w == target_w
            && source_h == target_h
            && source_format == output_rect
            && source_data == output_rect
        {
            let bitmap = source.clone().into_bitmap_frame()?;
            return Ok(PortValue::RasterFrame(RasterFrame::Bitmap(
                BitmapFrame::with_domain(
                    bitmap.pixels,
                    bitmap.storage_width,
                    bitmap.storage_height,
                    output_rect,
                    output_rect,
                )
                .with_alpha_mode(bitmap.alpha_mode),
            )));
        }

        if target_w == 0 || target_h == 0 {
            return Ok(PortValue::RasterFrame(RasterFrame::Bitmap(
                BitmapFrame::with_domain(Arc::new(Vec::new()), 0, 0, output_rect, output_rect),
            )));
        }

        let (bytes, width, height) = source.clone().into_parts();
        let source_alpha = source.alpha_mode();
        let Some(image) =
            make_skia_image(&bytes, width, height, (width as usize) * 4, source_alpha)
        else {
            return Ok(PortValue::RasterFrame(RasterFrame::Bitmap(
                BitmapFrame::with_domain(
                    Arc::new(vec![0u8; (target_w as usize) * (target_h as usize) * 4]),
                    target_w,
                    target_h,
                    output_rect,
                    output_rect,
                )
                .with_alpha_mode(source_alpha),
            )));
        };

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

        Ok(PortValue::RasterFrame(RasterFrame::Bitmap(
            BitmapFrame::with_domain(
                Arc::new(output),
                target_w,
                target_h,
                output_rect,
                output_rect,
            )
            .with_alpha_mode(source_alpha),
        )))
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
