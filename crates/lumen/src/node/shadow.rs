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
pub struct Shadow {
    pub offset_x: i32,
    pub offset_y: i32,
    pub color: [u8; 4],
}

impl NodeEval for Shadow {
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
        let (bytes, width, height) = match source {
            RasterFrame::Bitmap(bytes, width, height) => (bytes, width, height),
            RasterFrame::Surface(_) => (Arc::new(vec![0_u8; 4]), 1, 1),
        };

        let out_len = (width as usize)
            .checked_mul(height as usize)
            .and_then(|px| px.checked_mul(4))
            .unwrap_or(4);
        let mut out = vec![0_u8; out_len];

        for y in 0..height as usize {
            for x in 0..width as usize {
                let src_idx = (y * width as usize + x).saturating_mul(4);
                let src_alpha = bytes.get(src_idx + 3).copied().map(u16::from).unwrap_or(0);
                if src_alpha == 0 {
                    continue;
                }

                let dst_x = x as i64 + i64::from(self.offset_x);
                let dst_y = y as i64 + i64::from(self.offset_y);
                if dst_x < 0 || dst_y < 0 || dst_x >= i64::from(width) || dst_y >= i64::from(height)
                {
                    continue;
                }

                let shadow_idx =
                    ((dst_y as usize) * width as usize + dst_x as usize).saturating_mul(4);
                if let Some(dst_px) = out.get_mut(shadow_idx..shadow_idx.saturating_add(4)) {
                    dst_px[0] = self.color[0];
                    dst_px[1] = self.color[1];
                    dst_px[2] = self.color[2];
                    dst_px[3] = ((u16::from(self.color[3]) * src_alpha) / 255) as u8;
                }
            }
        }

        for y in 0..height as usize {
            for x in 0..width as usize {
                let idx = (y * width as usize + x).saturating_mul(4);
                let src = [
                    bytes.get(idx).copied().unwrap_or(0),
                    bytes.get(idx + 1).copied().unwrap_or(0),
                    bytes.get(idx + 2).copied().unwrap_or(0),
                    bytes.get(idx + 3).copied().unwrap_or(0),
                ];
                let dst = [
                    out.get(idx).copied().unwrap_or(0),
                    out.get(idx + 1).copied().unwrap_or(0),
                    out.get(idx + 2).copied().unwrap_or(0),
                    out.get(idx + 3).copied().unwrap_or(0),
                ];
                let blended = alpha_over(src, dst);
                if let Some(dst_px) = out.get_mut(idx..idx.saturating_add(4)) {
                    dst_px.copy_from_slice(&blended);
                }
            }
        }

        Ok(PortValue::RasterFrame(RasterFrame::Bitmap(
            Arc::new(out),
            width,
            height,
        )))
    }
}

fn alpha_over(src: [u8; 4], dst: [u8; 4]) -> [u8; 4] {
    let src_a = f32::from(src[3]) / 255.0;
    let dst_a = f32::from(dst[3]) / 255.0;
    let out_a = src_a + dst_a * (1.0 - src_a);

    if out_a <= f32::EPSILON {
        return [0, 0, 0, 0];
    }

    let mut out = [0_u8; 4];
    for i in 0..3 {
        let src_c = f32::from(src[i]) / 255.0;
        let dst_c = f32::from(dst[i]) / 255.0;
        let out_c = (src_c * src_a + dst_c * dst_a * (1.0 - src_a)) / out_a;
        out[i] = (out_c * 255.0).round().clamp(0.0, 255.0) as u8;
    }

    out[3] = (out_a * 255.0).round().clamp(0.0, 255.0) as u8;
    out
}
