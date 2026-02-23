use std::sync::Arc;

use crate::{
    error::LumenError,
    node::{InputPortDef, NodeEval, NodeInputs, OutputPortDef, PortKind, PortValue},
    raster::RasterFrame,
    render::RenderContext,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    pub scale_x: f32,
    pub scale_y: f32,
    pub translate_x: f32,
    pub translate_y: f32,
    pub rotate: f32,
    pub pivot_x: f32,
    pub pivot_y: f32,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            scale_x: 1.0,
            scale_y: 1.0,
            translate_x: 0.0,
            translate_y: 0.0,
            rotate: 0.0,
            pivot_x: 0.0,
            pivot_y: 0.0,
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
            RasterFrame::Surface(_) => (Arc::new(Vec::new()), 0, 0),
        };

        if width == 0 || height == 0 {
            return Ok(PortValue::RasterFrame(RasterFrame::Bitmap(
                Arc::new(Vec::new()),
                width,
                height,
            )));
        }

        if self.is_identity() {
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
        let (pivot_x, pivot_y) = self.resolved_pivot(width, height);
        let radians = self.rotate.to_radians();
        let cos_t = radians.cos();
        let sin_t = radians.sin();
        let scale_x = if self.scale_x.abs() <= f32::EPSILON {
            f32::INFINITY
        } else {
            self.scale_x
        };
        let scale_y = if self.scale_y.abs() <= f32::EPSILON {
            f32::INFINITY
        } else {
            self.scale_y
        };

        for y in 0..height {
            for x in 0..width {
                let dst_cx = x as f32 + 0.5;
                let dst_cy = y as f32 + 0.5;
                let translated_x = dst_cx - (pivot_x + self.translate_x);
                let translated_y = dst_cy - (pivot_y + self.translate_y);

                let rotated_x = translated_x * cos_t + translated_y * sin_t;
                let rotated_y = -translated_x * sin_t + translated_y * cos_t;

                let scaled_x = rotated_x / scale_x;
                let scaled_y = rotated_y / scale_y;

                let src_cx = scaled_x + pivot_x;
                let src_cy = scaled_y + pivot_y;
                let sample =
                    sample_bilinear(&source_bytes, width, height, src_cx - 0.5, src_cy - 0.5);

                if let Some(dst_idx) = pixel_index(width, x, y) {
                    output[dst_idx..dst_idx + 4].copy_from_slice(&sample);
                }
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
    pub fn is_identity(&self) -> bool {
        (self.scale_x - 1.0).abs() <= f32::EPSILON
            && (self.scale_y - 1.0).abs() <= f32::EPSILON
            && self.translate_x.abs() <= f32::EPSILON
            && self.translate_y.abs() <= f32::EPSILON
            && self.rotate.abs() <= f32::EPSILON
    }

    fn resolved_pivot(&self, width: u32, height: u32) -> (f32, f32) {
        if self.pivot_x.abs() <= f32::EPSILON && self.pivot_y.abs() <= f32::EPSILON {
            (width as f32 * 0.5, height as f32 * 0.5)
        } else {
            (self.pivot_x, self.pivot_y)
        }
    }
}

fn sample_bilinear(bytes: &[u8], width: u32, height: u32, x: f32, y: f32) -> [u8; 4] {
    let max_x = width as f32 - 1.0;
    let max_y = height as f32 - 1.0;
    if !x.is_finite() || !y.is_finite() || x < 0.0 || y < 0.0 || x > max_x || y > max_y {
        return [0, 0, 0, 0];
    }

    let x0 = x.floor();
    let y0 = y.floor();
    let x1 = (x0 + 1.0).min(max_x);
    let y1 = (y0 + 1.0).min(max_y);

    let tx = x - x0;
    let ty = y - y0;

    let p00 = pixel_at(bytes, width, x0 as u32, y0 as u32);
    let p10 = pixel_at(bytes, width, x1 as u32, y0 as u32);
    let p01 = pixel_at(bytes, width, x0 as u32, y1 as u32);
    let p11 = pixel_at(bytes, width, x1 as u32, y1 as u32);

    let mut out = [0_u8; 4];
    for channel in 0..4 {
        let top = f32::from(p00[channel]) * (1.0 - tx) + f32::from(p10[channel]) * tx;
        let bottom = f32::from(p01[channel]) * (1.0 - tx) + f32::from(p11[channel]) * tx;
        let value = top * (1.0 - ty) + bottom * ty;
        out[channel] = value.round().clamp(0.0, 255.0) as u8;
    }
    out
}

fn pixel_at(bytes: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let Some(index) = pixel_index(width, x, y) else {
        return [0, 0, 0, 0];
    };
    if index + 4 > bytes.len() {
        return [0, 0, 0, 0];
    }
    [
        bytes[index],
        bytes[index + 1],
        bytes[index + 2],
        bytes[index + 3],
    ]
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
