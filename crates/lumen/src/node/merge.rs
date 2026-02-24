use std::sync::Arc;

use crate::{
    error::LumenError,
    node::{BlendMode, InputPortDef, NodeEval, NodeInputs, OutputPortDef, PortKind, PortValue},
    raster::RasterFrame,
    render::RenderContext,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Merge {
    pub blend_mode: BlendMode,
    pub opacity: f32,
}

impl Default for Merge {
    fn default() -> Self {
        Self {
            blend_mode: BlendMode::Normal,
            opacity: 1.0,
        }
    }
}

const INPUT_PORT_DEFS: &[InputPortDef] = &[
    InputPortDef {
        name: "base",
        kind: PortKind::RasterFrame,
        optional: false,
    },
    InputPortDef {
        name: "overlay",
        kind: PortKind::RasterFrame,
        optional: false,
    },
    InputPortDef {
        name: "mask",
        kind: PortKind::RasterFrame,
        optional: true,
    },
];

const OUTPUT_PORT_DEFS: &[OutputPortDef] = &[OutputPortDef {
    name: "output",
    kind: PortKind::RasterFrame,
}];

impl NodeEval for Merge {
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
        let (base_bytes, base_w, base_h) =
            into_bitmap_parts(inputs.get_raster("base")?.clone().to_bitmap()?);
        let (overlay_bytes, overlay_w, overlay_h) =
            into_bitmap_parts(inputs.get_raster("overlay")?.clone().to_bitmap()?);
        let mask = match inputs.get_raster_optional("mask")? {
            Some(raster) => Some(into_bitmap_parts(raster.clone().to_bitmap()?)),
            None => None,
        };

        let out_w = base_w.min(overlay_w);
        let out_h = base_h.min(overlay_h);
        let Some(byte_len) = rgba_len(out_w, out_h) else {
            return Ok(PortValue::RasterFrame(RasterFrame::Bitmap(
                Arc::new(Vec::new()),
                0,
                0,
            )));
        };

        let opacity = self.opacity.clamp(0.0, 1.0);
        let mut out = vec![0_u8; byte_len];

        for y in 0..out_h {
            for x in 0..out_w {
                let Some(out_idx) = pixel_index(out_w, x, y) else {
                    continue;
                };
                let Some(base_idx) = pixel_index(base_w, x, y) else {
                    continue;
                };
                let Some(overlay_idx) = pixel_index(overlay_w, x, y) else {
                    continue;
                };

                let base_px = read_rgba(&base_bytes, base_idx);
                let overlay_px = read_rgba(&overlay_bytes, overlay_idx);

                let mask_alpha = match &mask {
                    Some((mask_bytes, mask_w, mask_h)) if x < *mask_w && y < *mask_h => {
                        match pixel_index(*mask_w, x, y) {
                            Some(mask_idx) => f32::from(read_rgba(mask_bytes, mask_idx)[3]) / 255.0,
                            None => 0.0,
                        }
                    }
                    Some(_) => 0.0,
                    None => 1.0,
                };

                let merged =
                    merge_pixel(base_px, overlay_px, self.blend_mode, opacity * mask_alpha);
                write_rgba(&mut out, out_idx, merged);
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

fn merge_pixel(base: [u8; 4], overlay: [u8; 4], blend_mode: BlendMode, factor: f32) -> [u8; 4] {
    let src = rgba_to_premul(overlay);
    let dst = rgba_to_premul(base);

    let src_a = (src[3] * factor).clamp(0.0, 1.0);
    let src_rgb_premul = [src[0] * factor, src[1] * factor, src[2] * factor];

    let src_rgb = unpremul_rgb(src_rgb_premul, src_a);
    let dst_rgb = unpremul_rgb([dst[0], dst[1], dst[2]], dst[3]);

    let blended_rgb = [
        blend_channel(dst_rgb[0], src_rgb[0], blend_mode),
        blend_channel(dst_rgb[1], src_rgb[1], blend_mode),
        blend_channel(dst_rgb[2], src_rgb[2], blend_mode),
    ];
    let blended_rgb_premul = [
        blended_rgb[0] * src_a,
        blended_rgb[1] * src_a,
        blended_rgb[2] * src_a,
    ];

    let out_a = src_a + dst[3] * (1.0 - src_a);
    let out_rgb = [
        blended_rgb_premul[0] + dst[0] * (1.0 - src_a),
        blended_rgb_premul[1] + dst[1] * (1.0 - src_a),
        blended_rgb_premul[2] + dst[2] * (1.0 - src_a),
    ];

    premul_to_rgba([out_rgb[0], out_rgb[1], out_rgb[2], out_a])
}

fn blend_channel(base: f32, overlay: f32, mode: BlendMode) -> f32 {
    let value = match mode {
        BlendMode::Normal => overlay,
        BlendMode::Multiply => base * overlay,
        BlendMode::Screen => base + overlay - (base * overlay),
        BlendMode::Overlay => {
            if base <= 0.5 {
                2.0 * base * overlay
            } else {
                1.0 - 2.0 * (1.0 - base) * (1.0 - overlay)
            }
        }
        BlendMode::Darken => base.min(overlay),
        BlendMode::Lighten => base.max(overlay),
    };
    value.clamp(0.0, 1.0)
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

fn rgba_to_premul(rgba: [u8; 4]) -> [f32; 4] {
    [
        f32::from(rgba[0]) / 255.0,
        f32::from(rgba[1]) / 255.0,
        f32::from(rgba[2]) / 255.0,
        f32::from(rgba[3]) / 255.0,
    ]
}

fn unpremul_rgb(rgb_premul: [f32; 3], alpha: f32) -> [f32; 3] {
    if alpha <= f32::EPSILON {
        return [0.0, 0.0, 0.0];
    }
    [
        (rgb_premul[0] / alpha).clamp(0.0, 1.0),
        (rgb_premul[1] / alpha).clamp(0.0, 1.0),
        (rgb_premul[2] / alpha).clamp(0.0, 1.0),
    ]
}

fn premul_to_rgba(premul: [f32; 4]) -> [u8; 4] {
    [
        float_unit_to_u8(premul[0]),
        float_unit_to_u8(premul[1]),
        float_unit_to_u8(premul[2]),
        float_unit_to_u8(premul[3]),
    ]
}

fn float_unit_to_u8(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}
