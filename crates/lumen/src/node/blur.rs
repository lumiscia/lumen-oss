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
pub struct Blur {
    pub radius: f32,
}

impl Blur {
    pub fn is_noop(&self) -> bool {
        self.radius <= 0.0
    }
}

impl NodeEval for Blur {
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
        let RasterFrame::Bitmap(bytes, width, height) = source else {
            return Ok(PortValue::RasterFrame(RasterFrame::Bitmap(
                Arc::new(Vec::new()),
                0,
                0,
            )));
        };

        if self.is_noop() || width == 0 || height == 0 {
            return Ok(PortValue::RasterFrame(RasterFrame::Bitmap(
                Arc::clone(&bytes),
                width,
                height,
            )));
        }

        let sigma = self.radius.max(0.5);
        let kernel = gaussian_kernel(sigma);
        let blurred = separable_blur(&bytes, width, height, &kernel);
        Ok(PortValue::RasterFrame(RasterFrame::Bitmap(
            Arc::new(blurred),
            width,
            height,
        )))
    }
}

fn gaussian_kernel(sigma: f32) -> Vec<f32> {
    let radius = (sigma * 3.0).ceil().max(1.0) as i32;
    let mut weights = Vec::with_capacity((radius * 2 + 1) as usize);
    let mut sum = 0.0_f32;
    for i in -radius..=radius {
        let distance = i as f32;
        let weight = (-distance * distance / (2.0 * sigma * sigma)).exp();
        weights.push(weight);
        sum += weight;
    }

    if sum > f32::EPSILON {
        for weight in &mut weights {
            *weight /= sum;
        }
    }

    weights
}

fn separable_blur(source: &[u8], width: u32, height: u32, kernel: &[f32]) -> Vec<u8> {
    let radius = (kernel.len() / 2) as i32;
    let pixel_len = rgba_len(width, height).unwrap_or_default();
    if pixel_len == 0 {
        return Vec::new();
    }

    let mut horizontal = vec![0_u8; pixel_len];
    for y in 0..height as i32 {
        for x in 0..width as i32 {
            let mut accum = [0.0_f32; 4];
            for (index, weight) in kernel.iter().enumerate() {
                let offset = index as i32 - radius;
                let sample_x = (x + offset).clamp(0, width as i32 - 1) as u32;
                let sample = pixel_at(source, width, sample_x, y as u32);
                for channel in 0..4 {
                    accum[channel] += f32::from(sample[channel]) * *weight;
                }
            }
            write_pixel(
                &mut horizontal,
                width,
                x as u32,
                y as u32,
                [
                    accum[0].round().clamp(0.0, 255.0) as u8,
                    accum[1].round().clamp(0.0, 255.0) as u8,
                    accum[2].round().clamp(0.0, 255.0) as u8,
                    accum[3].round().clamp(0.0, 255.0) as u8,
                ],
            );
        }
    }

    let mut vertical = vec![0_u8; pixel_len];
    for y in 0..height as i32 {
        for x in 0..width as i32 {
            let mut accum = [0.0_f32; 4];
            for (index, weight) in kernel.iter().enumerate() {
                let offset = index as i32 - radius;
                let sample_y = (y + offset).clamp(0, height as i32 - 1) as u32;
                let sample = pixel_at(&horizontal, width, x as u32, sample_y);
                for channel in 0..4 {
                    accum[channel] += f32::from(sample[channel]) * *weight;
                }
            }
            write_pixel(
                &mut vertical,
                width,
                x as u32,
                y as u32,
                [
                    accum[0].round().clamp(0.0, 255.0) as u8,
                    accum[1].round().clamp(0.0, 255.0) as u8,
                    accum[2].round().clamp(0.0, 255.0) as u8,
                    accum[3].round().clamp(0.0, 255.0) as u8,
                ],
            );
        }
    }

    vertical
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

fn write_pixel(bytes: &mut [u8], width: u32, x: u32, y: u32, pixel: [u8; 4]) {
    let Some(index) = pixel_index(width, x, y) else {
        return;
    };
    if index + 4 <= bytes.len() {
        bytes[index..index + 4].copy_from_slice(&pixel);
    }
}
