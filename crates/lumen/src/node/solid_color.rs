use std::sync::Arc;

use crate::{
    error::LumenError,
    node::{InputPortDef, NodeEval, NodeInputs, OutputPortDef, PortKind, PortValue},
    raster::RasterFrame,
    render::RenderContext,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SolidColor {
    pub color: [u8; 4],
    pub width: Option<u32>,
    pub height: Option<u32>,
}

impl Default for SolidColor {
    fn default() -> Self {
        Self {
            color: [0, 0, 0, 255],
            width: None,
            height: None,
        }
    }
}

impl NodeEval for SolidColor {
    fn input_port_defs(&self) -> &'static [InputPortDef] {
        &[]
    }

    fn output_port_defs(&self) -> &'static [OutputPortDef] {
        &[OutputPortDef {
            name: "output",
            kind: PortKind::RasterFrame,
        }]
    }

    fn evaluate(
        &self,
        _inputs: &NodeInputs,
        ctx: &mut RenderContext,
    ) -> Result<PortValue, LumenError> {
        let mut width = self.width.unwrap_or(ctx.width).max(1);
        let mut height = self.height.unwrap_or(ctx.height).max(1);
        let byte_len = match rgba_byte_len(width, height) {
            Some(len) => len,
            None => {
                width = 1;
                height = 1;
                4
            }
        };

        let mut bytes = vec![0_u8; byte_len];
        for pixel in bytes.chunks_exact_mut(4) {
            pixel.copy_from_slice(&self.color);
        }

        Ok(PortValue::RasterFrame(RasterFrame::Bitmap(
            Arc::new(bytes),
            width,
            height,
        )))
    }
}

fn rgba_byte_len(width: u32, height: u32) -> Option<usize> {
    let pixels = u64::from(width).checked_mul(u64::from(height))?;
    let bytes = pixels.checked_mul(4)?;
    usize::try_from(bytes).ok()
}
