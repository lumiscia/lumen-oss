use std::sync::Arc;

use skia_safe::Paint;

use crate::{
    error::LumenError,
    node::{
        BlendMode, InputPortDef, NodeEval, NodeInputs, OutputPortDef, PortKind, PortValue,
        pixel_utils::{make_skia_image, render_with_skia},
    },
    raster::{BitmapFrame, RasterFrame},
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
        _ctx: &mut RenderContext,
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
        let mask = match inputs.get_raster_optional("mask")? {
            Some(raster) => Some((raster.clone().into_parts(), raster.alpha_mode())),
            None => None,
        };

        let out_w = base_w;
        let out_h = base_h;
        if out_w == 0 || out_h == 0 {
            return Ok(PortValue::RasterFrame(base.clone()));
        }

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
                    Arc::new(vec![0u8; (out_w as usize) * (out_h as usize) * 4]),
                    out_w,
                    out_h,
                    base_format,
                    base_data,
                )
                .with_alpha_mode(base_alpha),
            )));
        };
        let Some(overlay_image) = overlay_image else {
            return Ok(PortValue::RasterFrame(base.clone()));
        };

        let opacity = self.opacity.clamp(0.0, 1.0);
        let skia_blend: skia_safe::BlendMode = self.blend_mode.into();

        let mask_image = match mask {
            Some(((mb, mw, mh), alpha_mode)) => {
                make_skia_image(&mb, mw, mh, (mw as usize) * 4, alpha_mode)
            }
            None => None,
        };

        let merged = render_with_skia(out_w, out_h, |canvas| {
            canvas.draw_image(&base_image, (0.0, 0.0), None);

            if let Some(ref mask_img) = mask_image {
                canvas.save_layer_alpha(None, 255);

                let mut overlay_paint = Paint::default();
                overlay_paint.set_blend_mode(skia_blend);
                overlay_paint.set_alpha_f(opacity);
                canvas.draw_image(&overlay_image, (0.0, 0.0), Some(&overlay_paint));

                let mut mask_paint = Paint::default();
                mask_paint.set_blend_mode(skia_safe::BlendMode::DstIn);
                canvas.draw_image(mask_img, (0.0, 0.0), Some(&mask_paint));

                canvas.restore();
            } else {
                let mut overlay_paint = Paint::default();
                overlay_paint.set_blend_mode(skia_blend);
                overlay_paint.set_alpha_f(opacity);
                canvas.draw_image(&overlay_image, (0.0, 0.0), Some(&overlay_paint));
            }
        });

        Ok(PortValue::RasterFrame(RasterFrame::Bitmap(
            BitmapFrame::with_domain(Arc::new(merged), out_w, out_h, base_format, base_format)
                .with_alpha_mode(base_alpha),
        )))
    }
}
