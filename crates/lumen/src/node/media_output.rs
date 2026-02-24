use std::sync::Arc;

use crate::{
    error::LumenError,
    node::{
        InputPortDef, NodeEval, NodeInputs, OutputPortDef, PortKind, PortValue,
        pixel_utils::{make_skia_image, render_with_skia},
    },
    raster::{BitmapFrame, RasterFrame},
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

        if source_w == target_w && source_h == target_h {
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

        let output = render_with_skia(target_w, target_h, |canvas| {
            canvas.draw_image(&image, (0.0, 0.0), None);
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
