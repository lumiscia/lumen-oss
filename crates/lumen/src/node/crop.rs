use std::sync::Arc;

use skia_safe::{IRect, image::RequiredProperties};

use crate::{
    error::LumenError,
    node::{
        InputPortDef, NodeEval, NodeInputs, OutputPortDef, PortKind, PortValue,
        pixel_utils::make_skia_image,
    },
    raster::RasterFrame,
    render::RenderContext,
};

const INPUT_PORTS: [InputPortDef; 1] = [InputPortDef {
    name: "source",
    kind: PortKind::RasterFrame,
    optional: false,
}];

const OUTPUT_PORTS: [OutputPortDef; 1] = [OutputPortDef {
    name: "output",
    kind: PortKind::RasterFrame,
}];

#[derive(Debug, Clone)]
pub struct Crop {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl NodeEval for Crop {
    fn input_port_defs(&self) -> &'static [InputPortDef] {
        &INPUT_PORTS
    }

    fn output_port_defs(&self) -> &'static [OutputPortDef] {
        &OUTPUT_PORTS
    }

    fn evaluate(
        &self,
        inputs: &NodeInputs,
        _ctx: &mut RenderContext,
    ) -> Result<PortValue, LumenError> {
        let (bytes, src_w, src_h) = inputs.get_raster("source")?.clone().into_parts();

        let x0 = (self.x as i64).clamp(0, src_w as i64) as i32;
        let y0 = (self.y as i64).clamp(0, src_h as i64) as i32;
        let x1 = ((self.x as i64) + (self.width as i64)).clamp(0, src_w as i64) as i32;
        let y1 = ((self.y as i64) + (self.height as i64)).clamp(0, src_h as i64) as i32;

        if x1 <= x0 || y1 <= y0 {
            return Ok(PortValue::RasterFrame(RasterFrame::Bitmap(
                Arc::new(vec![0, 0, 0, 0]),
                1,
                1,
            )));
        }

        let Some(image) = make_skia_image(&bytes, src_w, src_h) else {
            return Ok(PortValue::RasterFrame(RasterFrame::Bitmap(
                Arc::new(vec![0, 0, 0, 0]),
                1,
                1,
            )));
        };

        let subset = IRect::from_ltrb(x0, y0, x1, y1);
        let out_w = (x1 - x0) as u32;
        let out_h = (y1 - y0) as u32;

        if let Some(cropped) = image.make_subset(None, &subset, RequiredProperties::default()) {
            if let Some(data) = cropped.peek_pixels() {
                let pixel_bytes = data.bytes();
                if let Some(pixel_bytes) = pixel_bytes {
                    return Ok(PortValue::RasterFrame(RasterFrame::Bitmap(
                        Arc::new(pixel_bytes.to_vec()),
                        out_w,
                        out_h,
                    )));
                }
            }
        }

        // Fallback: draw via canvas
        let output = crate::node::pixel_utils::render_with_skia(out_w, out_h, |canvas| {
            canvas.draw_image(&image, (-x0 as f32, -y0 as f32), None);
        });

        Ok(PortValue::RasterFrame(RasterFrame::Bitmap(
            Arc::new(output),
            out_w,
            out_h,
        )))
    }
}
