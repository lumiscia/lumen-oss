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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeMode {
    Stretch,
    Fit,
    Fill,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeSampling {
    Nearest,
    Linear,
}

#[derive(Debug, Clone)]
pub struct Resize {
    pub width: u32,
    pub height: u32,
    pub mode: ResizeMode,
    pub sampling: ResizeSampling,
}

impl NodeEval for Resize {
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

        let target_width = self.width.max(1);
        let target_height = self.height.max(1);
        let out_len = (target_width as usize)
            .checked_mul(target_height as usize)
            .and_then(|px| px.checked_mul(4))
            .unwrap_or(4);
        let mut out = vec![0_u8; out_len];

        if src_width == 0 || src_height == 0 {
            return Ok(PortValue::RasterFrame(RasterFrame::Bitmap(
                Arc::new(out),
                target_width,
                target_height,
            )));
        }

        match self.mode {
            ResizeMode::Stretch | ResizeMode::Fit | ResizeMode::Fill => {
                for y in 0..target_height as usize {
                    let src_y = ((y as u64 * src_height as u64) / target_height as u64)
                        .min(src_height.saturating_sub(1) as u64)
                        as usize;
                    for x in 0..target_width as usize {
                        let src_x = ((x as u64 * src_width as u64) / target_width as u64)
                            .min(src_width.saturating_sub(1) as u64)
                            as usize;

                        let src_idx = (src_y * src_width as usize + src_x).saturating_mul(4);
                        let dst_idx = (y * target_width as usize + x).saturating_mul(4);

                        if let (Some(src_px), Some(dst_px)) = (
                            bytes.get(src_idx..src_idx.saturating_add(4)),
                            out.get_mut(dst_idx..dst_idx.saturating_add(4)),
                        ) {
                            dst_px.copy_from_slice(src_px);
                        }
                    }
                }
            }
        }

        Ok(PortValue::RasterFrame(RasterFrame::Bitmap(
            Arc::new(out),
            target_width,
            target_height,
        )))
    }
}
