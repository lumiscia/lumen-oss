use std::sync::Arc;

use crate::{
    error::LumenError,
    node::{
        InputPortDef, NodeEval, NodeInputs, OutputPortDef, PortKind, PortValue,
        pixel_utils::{make_skia_image, render_with_skia},
    },
    raster::RasterFrame,
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
        let (target_w, target_h) = (ctx.width, ctx.height);
        let (source_w, source_h) = source.dimensions();

        if source_w == target_w && source_h == target_h {
            return Ok(PortValue::RasterFrame(source.clone().to_bitmap()?));
        }

        if target_w == 0 || target_h == 0 {
            return Ok(PortValue::RasterFrame(RasterFrame::Bitmap(
                Arc::new(Vec::new()),
                0,
                0,
            )));
        }

        let (bytes, width, height) = source.clone().into_parts();
        let Some(image) = make_skia_image(&bytes, width, height) else {
            return Ok(PortValue::RasterFrame(RasterFrame::Bitmap(
                Arc::new(vec![0u8; (target_w as usize) * (target_h as usize) * 4]),
                target_w,
                target_h,
            )));
        };

        let output = render_with_skia(target_w, target_h, |canvas| {
            canvas.draw_image(&image, (0.0, 0.0), None);
        });

        Ok(PortValue::RasterFrame(RasterFrame::Bitmap(
            Arc::new(output),
            target_w,
            target_h,
        )))
    }
}
