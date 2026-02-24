use std::sync::Arc;

use crate::{
    error::LumenError,
    node::{
        InputPortDef, NodeEval, NodeInputs, OutputPortDef, PortKind, PortValue,
        pixel_utils::{render_with_skia, rgba_byte_len, to_skia_color},
    },
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
        if rgba_byte_len(width, height).is_none() {
            width = 1;
            height = 1;
        }

        let color = to_skia_color(self.color);
        let bytes = render_with_skia(width, height, |canvas| {
            canvas.clear(color);
        });

        Ok(PortValue::RasterFrame(RasterFrame::bitmap(
            Arc::new(bytes),
            width,
            height,
        )))
    }
}
