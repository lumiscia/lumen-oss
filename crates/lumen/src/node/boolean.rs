use std::sync::Arc;

use skia_safe::Paint;

use crate::{
    error::LumenError,
    node::{
        InputPortDef, NodeEval, NodeInputs, OutputPortDef, PortKind, PortValue,
        pixel_utils::{make_skia_image, render_with_skia},
    },
    raster::RasterFrame,
    render::RenderContext,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MaskKind {
    Alpha,
    Luma,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Boolean {
    pub mask_kind: MaskKind,
    pub invert: bool,
}

impl Default for Boolean {
    fn default() -> Self {
        Self {
            mask_kind: MaskKind::Alpha,
            invert: false,
        }
    }
}

const INPUT_PORT_DEFS: &[InputPortDef] = &[
    InputPortDef {
        name: "source",
        kind: PortKind::RasterFrame,
        optional: false,
    },
    InputPortDef {
        name: "mask",
        kind: PortKind::RasterFrame,
        optional: true,
    },
    InputPortDef {
        name: "vector",
        kind: PortKind::Vector,
        optional: true,
    },
];

const OUTPUT_PORT_DEFS: &[OutputPortDef] = &[OutputPortDef {
    name: "output",
    kind: PortKind::RasterFrame,
}];

impl NodeEval for Boolean {
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
        let (source_bytes, source_w, source_h) =
            inputs.get_raster("source")?.clone().into_parts();
        let mask = match inputs.get_raster_optional("mask")? {
            Some(frame) => Some(frame.clone().into_parts()),
            None => None,
        };

        let Some((mask_bytes, mask_w, mask_h)) = mask else {
            return Ok(PortValue::RasterFrame(RasterFrame::Bitmap(
                source_bytes,
                source_w,
                source_h,
            )));
        };

        let out_w = source_w.min(mask_w);
        let out_h = source_h.min(mask_h);
        if out_w == 0 || out_h == 0 {
            return Ok(PortValue::RasterFrame(RasterFrame::Bitmap(
                Arc::new(Vec::new()),
                0,
                0,
            )));
        }

        let source_image = make_skia_image(&source_bytes, source_w, source_h);
        let mask_image = make_skia_image(&mask_bytes, mask_w, mask_h);

        let (source_image, mask_image) = match (source_image, mask_image) {
            (Some(s), Some(m)) => (s, m),
            _ => {
                return Ok(PortValue::RasterFrame(RasterFrame::Bitmap(
                    Arc::new(vec![0u8; (out_w as usize) * (out_h as usize) * 4]),
                    out_w,
                    out_h,
                )));
            }
        };

        let mask_kind = self.mask_kind;
        let invert = self.invert;

        let output = render_with_skia(out_w, out_h, |canvas| {
            canvas.draw_image(&source_image, (0.0, 0.0), None);

            // For luma masking, we need to convert the mask to a grayscale alpha.
            // For alpha masking, we use the mask's alpha channel directly.
            // In both cases, DstIn keeps destination where mask is opaque,
            // DstOut keeps destination where mask is transparent.
            let blend_mode = if invert {
                skia_safe::BlendMode::DstOut
            } else {
                skia_safe::BlendMode::DstIn
            };

            if mask_kind == MaskKind::Luma {
                // For luma masking, draw a luminance-based alpha mask.
                // We create a layer, draw the mask, then apply a color filter
                // that converts RGB to alpha based on luminance.
                let luma_cf = skia_safe::ColorFilter::luma();
                let mut mask_paint = Paint::default();
                mask_paint.set_blend_mode(blend_mode);
                mask_paint.set_color_filter(luma_cf);
                canvas.draw_image(&mask_image, (0.0, 0.0), Some(&mask_paint));
            } else {
                let mut mask_paint = Paint::default();
                mask_paint.set_blend_mode(blend_mode);
                canvas.draw_image(&mask_image, (0.0, 0.0), Some(&mask_paint));
            }
        });

        Ok(PortValue::RasterFrame(RasterFrame::Bitmap(
            Arc::new(output),
            out_w,
            out_h,
        )))
    }
}
