use std::sync::Arc;

use crate::{
    error::LumenError,
    node::{InputPortDef, NodeEval, NodeInputs, OutputPortDef, PortKind, PortValue},
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
        let source = inputs.get_raster("source")?.clone().to_bitmap()?;
        let (bytes, src_width, src_height) = match source {
            RasterFrame::Bitmap(bytes, width, height) => (bytes, width, height),
            RasterFrame::Surface(_) => (Arc::new(vec![0_u8; 4]), 1, 1),
        };

        let x0 = i64::from(self.x).clamp(0, i64::from(src_width));
        let y0 = i64::from(self.y).clamp(0, i64::from(src_height));
        let x1 = (i64::from(self.x) + i64::from(self.width)).clamp(0, i64::from(src_width));
        let y1 = (i64::from(self.y) + i64::from(self.height)).clamp(0, i64::from(src_height));

        if x1 <= x0 || y1 <= y0 {
            return Ok(PortValue::RasterFrame(RasterFrame::Bitmap(
                Arc::new(vec![0, 0, 0, 0]),
                1,
                1,
            )));
        }

        let out_width = u32::try_from(x1 - x0).unwrap_or(1);
        let out_height = u32::try_from(y1 - y0).unwrap_or(1);
        let out_len = (out_width as usize)
            .checked_mul(out_height as usize)
            .and_then(|px| px.checked_mul(4))
            .unwrap_or(4);
        let mut out = vec![0_u8; out_len];

        for y in 0..out_height as usize {
            for x in 0..out_width as usize {
                let src_x = x0 as usize + x;
                let src_y = y0 as usize + y;
                let src_idx = (src_y * src_width as usize + src_x).saturating_mul(4);
                let dst_idx = (y * out_width as usize + x).saturating_mul(4);

                if let (Some(src_px), Some(dst_px)) = (
                    bytes.get(src_idx..src_idx.saturating_add(4)),
                    out.get_mut(dst_idx..dst_idx.saturating_add(4)),
                ) {
                    dst_px.copy_from_slice(src_px);
                }
            }
        }

        Ok(PortValue::RasterFrame(RasterFrame::Bitmap(
            Arc::new(out),
            out_width,
            out_height,
        )))
    }
}
