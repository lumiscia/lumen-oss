use std::sync::Arc;

use crate::{
    error::LumenError,
    node::{
        InputPortDef, NodeEval, NodeInputs, OutputPortDef, PortKind, PortValue, ShapeGeometry,
        VectorData,
    },
    raster::RasterFrame,
    render::RenderContext,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShapeRenderer {
    pub color: [u8; 4],
}

impl Default for ShapeRenderer {
    fn default() -> Self {
        Self {
            color: [255, 255, 255, 255],
        }
    }
}

impl NodeEval for ShapeRenderer {
    fn input_port_defs(&self) -> &'static [InputPortDef] {
        &[InputPortDef {
            name: "vector",
            kind: PortKind::Vector,
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
        _ctx: &mut RenderContext,
    ) -> Result<PortValue, LumenError> {
        let vector = inputs.get_vector("vector")?;
        let raster = match vector {
            VectorData::Shape(geometry) => rasterize_geometry(geometry, self.color),
        };
        Ok(PortValue::RasterFrame(raster))
    }
}

fn rasterize_geometry(geometry: &ShapeGeometry, color: [u8; 4]) -> RasterFrame {
    match geometry {
        ShapeGeometry::Rectangle { width, height } => {
            let width = (*width).max(1);
            let height = (*height).max(1);
            filled_bitmap(width, height, color)
        }
        ShapeGeometry::Ellipse { width, height } => {
            let width = (*width).max(1);
            let height = (*height).max(1);
            ellipse_bitmap(width, height, color)
        }
        ShapeGeometry::Polygon { points } => polygon_bitmap(points, color),
    }
}

fn filled_bitmap(width: u32, height: u32, color: [u8; 4]) -> RasterFrame {
    let (width, height, mut bytes) = allocate_bitmap(width, height);
    for pixel in bytes.chunks_exact_mut(4) {
        pixel.copy_from_slice(&color);
    }
    RasterFrame::Bitmap(Arc::new(bytes), width, height)
}

fn ellipse_bitmap(width: u32, height: u32, color: [u8; 4]) -> RasterFrame {
    let (width, height, mut bytes) = allocate_bitmap(width, height);
    let rx = (width as f32) * 0.5;
    let ry = (height as f32) * 0.5;
    let cx = rx;
    let cy = ry;

    for y in 0..height {
        for x in 0..width {
            let dx = ((x as f32) + 0.5 - cx) / rx.max(f32::EPSILON);
            let dy = ((y as f32) + 0.5 - cy) / ry.max(f32::EPSILON);
            if dx * dx + dy * dy <= 1.0 {
                let pixel_offset = (u64::from(y) * u64::from(width) + u64::from(x)) * 4;
                if let Ok(index) = usize::try_from(pixel_offset) {
                    if index + 4 <= bytes.len() {
                        bytes[index..index + 4].copy_from_slice(&color);
                    }
                }
            }
        }
    }

    RasterFrame::Bitmap(Arc::new(bytes), width, height)
}

fn polygon_bitmap(points: &[(f32, f32)], color: [u8; 4]) -> RasterFrame {
    if points.is_empty() {
        return filled_bitmap(1, 1, color);
    }

    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;

    for &(x, y) in points {
        if x.is_finite() && y.is_finite() {
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        }
    }

    if !min_x.is_finite() || !min_y.is_finite() || !max_x.is_finite() || !max_y.is_finite() {
        return filled_bitmap(1, 1, color);
    }

    let width = (max_x - min_x).ceil().max(1.0) as u32;
    let height = (max_y - min_y).ceil().max(1.0) as u32;
    filled_bitmap(width, height, color)
}

fn allocate_bitmap(width: u32, height: u32) -> (u32, u32, Vec<u8>) {
    let width = width.max(1);
    let height = height.max(1);
    match rgba_byte_len(width, height) {
        Some(len) => (width, height, vec![0_u8; len]),
        None => (1, 1, vec![0_u8; 4]),
    }
}

fn rgba_byte_len(width: u32, height: u32) -> Option<usize> {
    let pixels = u64::from(width).checked_mul(u64::from(height))?;
    let bytes = pixels.checked_mul(4)?;
    usize::try_from(bytes).ok()
}
