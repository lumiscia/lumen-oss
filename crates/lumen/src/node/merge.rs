use std::sync::Arc;

use skia_safe::{Paint, Rect, SamplingOptions};

use crate::{
    error::LumenError,
    node::{
        BlendMode, InputPortDef, NodeEval, NodeInputs, OutputPortDef, PortKind, PortValue,
        pixel_utils::{make_skia_image, render_with_skia},
    },
    raster::{BitmapFrame, RasterFrame, RectI},
    render::RenderContext,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Merge {
    pub blend_mode: BlendMode,
    pub opacity: f32,
}

impl Default for Merge {
    fn default() -> Self {
        Self {
            blend_mode: BlendMode::Normal,
            opacity: 1.0,
        }
    }
}

const INPUT_PORT_DEFS: &[InputPortDef] = &[
    InputPortDef {
        name: "base",
        kind: PortKind::RasterFrame,
        optional: false,
    },
    InputPortDef {
        name: "overlay",
        kind: PortKind::RasterFrame,
        optional: false,
    },
    InputPortDef {
        name: "mask",
        kind: PortKind::RasterFrame,
        optional: true,
    },
];

const OUTPUT_PORT_DEFS: &[OutputPortDef] = &[OutputPortDef {
    name: "output",
    kind: PortKind::RasterFrame,
}];

impl NodeEval for Merge {
    fn input_port_defs(&self) -> &'static [InputPortDef] {
        INPUT_PORT_DEFS
    }

    fn output_port_defs(&self) -> &'static [OutputPortDef] {
        OUTPUT_PORT_DEFS
    }

    fn evaluate(
        &self,
        inputs: &NodeInputs,
        ctx: &mut RenderContext,
    ) -> Result<PortValue, LumenError> {
        if self.opacity <= 0.0 {
            return Ok(PortValue::RasterFrame(inputs.get_raster("base")?.clone()));
        }

        let base = inputs.get_raster("base")?;
        let overlay = inputs.get_raster("overlay")?;
        let (base_bytes, base_w, base_h) = base.clone().into_parts();
        let (overlay_bytes, overlay_w, overlay_h) = overlay.clone().into_parts();
        let base_alpha = base.alpha_mode();
        let overlay_alpha = overlay.alpha_mode();
        let base_format = base.format_rect();
        let base_data = base.data_rect();
        let overlay_format = overlay.format_rect();
        let overlay_data = overlay.data_rect();
        let mask = match inputs.get_raster_optional("mask")? {
            Some(raster) => Some((
                raster.clone().into_parts(),
                raster.alpha_mode(),
                raster.format_rect(),
                raster.data_rect(),
            )),
            None => None,
        };
        let out_format = union_rect(base_format, overlay_format);
        let mut out_data = union_rect(base_data, overlay_data);
        out_data =
            out_format
                .intersect(&out_data)
                .unwrap_or(RectI::new(out_format.x, out_format.y, 0, 0));
        let render_w = out_data.width.max(1);
        let render_h = out_data.height.max(1);

        let base_image = make_skia_image(
            &base_bytes,
            base_w,
            base_h,
            (base_w as usize) * 4,
            base_alpha,
        );
        let overlay_image = make_skia_image(
            &overlay_bytes,
            overlay_w,
            overlay_h,
            (overlay_w as usize) * 4,
            overlay_alpha,
        );

        let Some(base_image) = base_image else {
            return Ok(PortValue::RasterFrame(RasterFrame::Bitmap(
                BitmapFrame::with_domain(
                    Arc::new(vec![0u8; (render_w as usize) * (render_h as usize) * 4]),
                    render_w,
                    render_h,
                    out_format,
                    out_data,
                )
                .with_alpha_mode(base_alpha),
            )));
        };
        let Some(overlay_image) = overlay_image else {
            return Ok(PortValue::RasterFrame(base.clone()));
        };

        let opacity = self.opacity.clamp(0.0, 1.0);
        let skia_blend: skia_safe::BlendMode = self.blend_mode.into();

        let mask_image = match mask.as_ref() {
            Some(((mb, mw, mh), alpha_mode, _mask_format, _mask_data)) => {
                make_skia_image(mb.as_slice(), *mw, *mh, (*mw as usize) * 4, *alpha_mode)
            }
            None => None,
        };

        let merged = render_with_skia(render_w, render_h, Some(ctx), |canvas| {
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

            if let Some(ref mask_img) = mask_image {
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
                if let Some(((_, mask_w, mask_h), _, mask_format, mask_data)) = mask.as_ref() {
                    draw_frame_image(
                        canvas,
                        mask_img,
                        *mask_w,
                        *mask_h,
                        *mask_format,
                        *mask_data,
                        out_data,
                        Some(&mask_paint),
                    );
                }

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
        });

        Ok(PortValue::RasterFrame(RasterFrame::Bitmap(
            BitmapFrame::with_domain(Arc::new(merged), render_w, render_h, out_format, out_data)
                .with_alpha_mode(base_alpha),
        )))
    }
}

fn union_rect(left: RectI, right: RectI) -> RectI {
    let min_x = left.x.min(right.x);
    let min_y = left.y.min(right.y);
    let max_x = left.right().max(right.right());
    let max_y = left.bottom().max(right.bottom());
    let width = (max_x - i64::from(min_x)).max(1) as u32;
    let height = (max_y - i64::from(min_y)).max(1) as u32;
    RectI::new(min_x, min_y, width, height)
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
