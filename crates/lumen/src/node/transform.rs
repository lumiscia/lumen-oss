use std::sync::Arc;

use crate::{
    error::LumenError,
    node::{InputPortDef, NodeEval, NodeInputs, OutputPortDef, PortKind, PortValue},
    raster::RasterFrame,
    render::RenderContext,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    pub scale: (f32, f32),
    pub translate: (f32, f32),
    pub rotate_degrees: f32,
    pub pivot: (f32, f32),
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            scale: (1.0, 1.0),
            translate: (0.0, 0.0),
            rotate_degrees: 0.0,
            pivot: (0.0, 0.0),
        }
    }
}

const INPUT_PORT_DEFS: &[InputPortDef] = &[InputPortDef {
    name: "source",
    kind: PortKind::RasterFrame,
    optional: false,
}];

const OUTPUT_PORT_DEFS: &[OutputPortDef] = &[OutputPortDef {
    name: "output",
    kind: PortKind::RasterFrame,
}];

impl NodeEval for Transform {
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
        let source = inputs.get_raster("source")?.clone().to_bitmap()?;
        let (source_bytes, width, height) = match source {
            RasterFrame::Bitmap(bytes, width, height) => (bytes, width, height),
            RasterFrame::Surface(surface) => {
                let width = surface.width();
                let height = surface.height();
                let bytes = rgba_len(width, height).map_or_else(Vec::new, |len| vec![0; len]);
                (Arc::new(bytes), width, height)
            }
        };

        if self.has_non_translation_transform() {
            return Ok(PortValue::RasterFrame(RasterFrame::Bitmap(
                Arc::clone(&source_bytes),
                width,
                height,
            )));
        }

        let Some(pixel_len) = rgba_len(width, height) else {
            return Ok(PortValue::RasterFrame(RasterFrame::Bitmap(
                Arc::new(Vec::new()),
                0,
                0,
            )));
        };
        let mut output = vec![0_u8; pixel_len];

        let tx = self.translate.0.round() as i32;
        let ty = self.translate.1.round() as i32;

        for y in 0..height {
            for x in 0..width {
                let dst_x = i64::from(x) + i64::from(tx);
                let dst_y = i64::from(y) + i64::from(ty);

                if dst_x < 0 || dst_y < 0 || dst_x >= i64::from(width) || dst_y >= i64::from(height)
                {
                    continue;
                }

                let Some(src_idx) = pixel_index(width, x, y) else {
                    continue;
                };
                let Some(dst_idx) = pixel_index(width, dst_x as u32, dst_y as u32) else {
                    continue;
                };
                copy_pixel(&source_bytes, &mut output, src_idx, dst_idx);
            }
        }

        Ok(PortValue::RasterFrame(RasterFrame::Bitmap(
            Arc::new(output),
            width,
            height,
        )))
    }
}

impl Transform {
    fn has_non_translation_transform(&self) -> bool {
        let scale_is_identity = (self.scale.0 - 1.0).abs() <= f32::EPSILON
            && (self.scale.1 - 1.0).abs() <= f32::EPSILON;
        let rotation_is_identity = self.rotate_degrees.abs() <= f32::EPSILON;
        let pivot_is_origin =
            self.pivot.0.abs() <= f32::EPSILON && self.pivot.1.abs() <= f32::EPSILON;

        !(scale_is_identity && rotation_is_identity && pivot_is_origin)
    }
}

fn rgba_len(width: u32, height: u32) -> Option<usize> {
    let pixel_count = u64::from(width).checked_mul(u64::from(height))?;
    let byte_count = pixel_count.checked_mul(4)?;
    usize::try_from(byte_count).ok()
}

fn pixel_index(width: u32, x: u32, y: u32) -> Option<usize> {
    let row = u64::from(y).checked_mul(u64::from(width))?;
    let offset = row.checked_add(u64::from(x))?;
    let byte_offset = offset.checked_mul(4)?;
    usize::try_from(byte_offset).ok()
}

fn copy_pixel(src: &[u8], dst: &mut [u8], src_idx: usize, dst_idx: usize) {
    let src_end = src_idx.saturating_add(4);
    let dst_end = dst_idx.saturating_add(4);
    if src_end <= src.len() && dst_end <= dst.len() {
        dst[dst_idx..dst_end].copy_from_slice(&src[src_idx..src_end]);
    }
}
