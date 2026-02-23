use std::sync::Arc;

use crate::{
    error::LumenError,
    node::{InputPortDef, NodeEval, NodeInputs, OutputPortDef, PortKind, PortValue},
    raster::RasterFrame,
    render::RenderContext,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MaskKind {
    Alpha,
    Luma,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Boolean {
    pub mask_kind: MaskKind,
    pub invert: bool,
}

impl Default for Boolean {
    fn default() -> Self {
        Self {
            mask_kind: MaskKind::Alpha,
            invert: false,
        }
    }
}

const INPUT_PORT_DEFS: &[InputPortDef] = &[
    InputPortDef {
        name: "source",
        kind: PortKind::RasterFrame,
        optional: false,
    },
    InputPortDef {
        name: "mask",
        kind: PortKind::RasterFrame,
        optional: true,
    },
    InputPortDef {
        name: "vector",
        kind: PortKind::Vector,
        optional: true,
    },
];

const OUTPUT_PORT_DEFS: &[OutputPortDef] = &[OutputPortDef {
    name: "output",
    kind: PortKind::RasterFrame,
}];

impl NodeEval for Boolean {
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
        let (source_bytes, source_w, source_h) =
            into_bitmap_parts(inputs.get_raster("source")?.clone().to_bitmap()?);
        let mask = match inputs.get_raster_optional("mask")? {
            Some(frame) => Some(into_bitmap_parts(frame.clone().to_bitmap()?)),
            None => None,
        };

        let Some((mask_bytes, mask_w, mask_h)) = mask else {
            return Ok(PortValue::RasterFrame(RasterFrame::Bitmap(
                Arc::clone(&source_bytes),
                source_w,
                source_h,
            )));
        };

        let out_w = source_w.min(mask_w);
        let out_h = source_h.min(mask_h);
        let Some(byte_len) = rgba_len(out_w, out_h) else {
            return Ok(PortValue::RasterFrame(RasterFrame::Bitmap(
                Arc::new(Vec::new()),
                0,
                0,
            )));
        };

        let mut out = vec![0_u8; byte_len];
        for y in 0..out_h {
            for x in 0..out_w {
                let Some(out_idx) = pixel_index(out_w, x, y) else {
                    continue;
                };
                let Some(source_idx) = pixel_index(source_w, x, y) else {
                    continue;
                };
                let Some(mask_idx) = pixel_index(mask_w, x, y) else {
                    continue;
                };

                let source_px = read_rgba(&source_bytes, source_idx);
                let mask_px = read_rgba(&mask_bytes, mask_idx);
                let mut coverage = mask_coverage(mask_px, self.mask_kind);
                if self.invert {
                    coverage = 1.0 - coverage;
                }

                let alpha_scale = coverage.clamp(0.0, 1.0);
                let out_px = [
                    scale_u8(source_px[0], alpha_scale),
                    scale_u8(source_px[1], alpha_scale),
                    scale_u8(source_px[2], alpha_scale),
                    scale_u8(source_px[3], alpha_scale),
                ];
                write_rgba(&mut out, out_idx, out_px);
            }
        }

        Ok(PortValue::RasterFrame(RasterFrame::Bitmap(
            Arc::new(out),
            out_w,
            out_h,
        )))
    }
}

fn into_bitmap_parts(raster: RasterFrame) -> (Arc<Vec<u8>>, u32, u32) {
    match raster {
        RasterFrame::Bitmap(bytes, width, height) => (bytes, width, height),
        RasterFrame::Surface(surface) => {
            let width = surface.width();
            let height = surface.height();
            let bytes = rgba_len(width, height).map_or_else(Vec::new, |len| vec![0; len]);
            (Arc::new(bytes), width, height)
        }
    }
}

fn mask_coverage(mask: [u8; 4], kind: MaskKind) -> f32 {
    match kind {
        MaskKind::Alpha => f32::from(mask[3]) / 255.0,
        MaskKind::Luma => {
            let r = f32::from(mask[0]) / 255.0;
            let g = f32::from(mask[1]) / 255.0;
            let b = f32::from(mask[2]) / 255.0;
            (0.2126 * r + 0.7152 * g + 0.0722 * b).clamp(0.0, 1.0)
        }
    }
}

fn rgba_len(width: u32, height: u32) -> Option<usize> {
    let pixels = u64::from(width).checked_mul(u64::from(height))?;
    let bytes = pixels.checked_mul(4)?;
    usize::try_from(bytes).ok()
}

fn pixel_index(width: u32, x: u32, y: u32) -> Option<usize> {
    let row = u64::from(y).checked_mul(u64::from(width))?;
    let offset = row.checked_add(u64::from(x))?;
    let bytes = offset.checked_mul(4)?;
    usize::try_from(bytes).ok()
}

fn read_rgba(bytes: &[u8], index: usize) -> [u8; 4] {
    let i1 = index.checked_add(1);
    let i2 = index.checked_add(2);
    let i3 = index.checked_add(3);
    [
        bytes.get(index).copied().unwrap_or(0),
        i1.and_then(|idx| bytes.get(idx).copied()).unwrap_or(0),
        i2.and_then(|idx| bytes.get(idx).copied()).unwrap_or(0),
        i3.and_then(|idx| bytes.get(idx).copied()).unwrap_or(0),
    ]
}

fn write_rgba(bytes: &mut [u8], index: usize, rgba: [u8; 4]) {
    let Some(last) = index.checked_add(3) else {
        return;
    };
    if last >= bytes.len() {
        return;
    }
    bytes[index] = rgba[0];
    bytes[index + 1] = rgba[1];
    bytes[index + 2] = rgba[2];
    bytes[index + 3] = rgba[3];
}

fn scale_u8(value: u8, scalar: f32) -> u8 {
    (f32::from(value) * scalar.clamp(0.0, 1.0)).round() as u8
}
