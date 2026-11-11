use bytemuck::{Pod, Zeroable};

use crate::{
    error::RenderError,
    node::{JsonNode, NodeId},
};

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct ColorGradeParams {
    pub(crate) strength: f32,
    pub(crate) interpolation: u32,
    pub(crate) _pad: [u32; 2],
    /// The `.cube` table's `DOMAIN_MIN` and `DOMAIN_MAX`, padded to four floats.
    pub(crate) domain_min: [f32; 4],
    pub(crate) domain_max: [f32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct ColorGradeLut {
    stops: [[f32; 4]; LUT_TABLE_SIZE],
}

impl ColorGradeLut {
    pub(crate) fn parse(node_id: NodeId, source: &str) -> crate::Result<Self> {
        let stops = parse_lut_stops(node_id, source)?;
        let mut table = [[0.0; 4]; LUT_TABLE_SIZE];
        for (index, entry) in table.iter_mut().enumerate() {
            let value = index as f32 / (LUT_TABLE_SIZE - 1) as f32;
            let scaled = value * (stops.len() - 1) as f32;
            let low = scaled.floor() as usize;
            let high = (low + 1).min(stops.len() - 1);
            let t = scaled - low as f32;
            *entry = [
                stops[low][0] + (stops[high][0] - stops[low][0]) * t,
                stops[low][1] + (stops[high][1] - stops[low][1]) * t,
                stops[low][2] + (stops[high][2] - stops[low][2]) * t,
                1.0,
            ];
        }
        Ok(Self { stops: table })
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct ExposureParams {
    pub(crate) exposure: f32,
    pub(crate) contrast: f32,
    pub(crate) offset: f32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct HueSaturationParams {
    pub(crate) hue_offset: f32,
    pub(crate) saturation: f32,
    pub(crate) lightness: f32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct LevelsParams {
    pub(crate) black_point: f32,
    pub(crate) white_point: f32,
    pub(crate) gamma: f32,
    pub(crate) output_black: f32,
    pub(crate) output_white: f32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct BlurParams {
    /// Gaussian sigma, transparent-edge flag.
    pub(crate) values: [f32; 4],
    /// Whole-pixel offset applied to the source in `xy`.
    pub(crate) offset: [f32; 4],
    /// Canvas pixel of the output's first texel in `xy`.
    pub(crate) region: [i32; 4],
    /// Canvas pixel of the source's first texel in `xy`, and the source's display size in `zw`.
    pub(crate) source: [i32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct ShadowParams {
    /// The shadow colour, with its opacity as alpha.
    pub(crate) color: [f32; 4],
    /// Canvas pixel of the output's first texel in `xy`, and of the blurred shadow's in `zw`.
    pub(crate) origins: [i32; 4],
    /// Canvas pixel of the source's first texel in `xy`.
    pub(crate) source_origin: [i32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct MergeParams {
    pub(crate) opacity: f32,
    pub(crate) blend_mode: u32,
    pub(crate) mask_source: u32,
    pub(crate) mask_mode: u32,
    pub(crate) invert: u32,
    pub(crate) _pad0: u32,
    /// Canvas pixels of the first texels of the output, base, overlay and raw mask.
    pub(crate) origins: [[i32; 2]; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct BooleanParams {
    pub(crate) values: [f32; 2],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct RasterMultiMergeParams {
    pub(crate) opacity: f32,
    pub(crate) blend_mode: u32,
    /// Output texel of the pass's first invocation, then canvas pixels of the first texels
    /// of the output, base and overlay.
    pub(crate) origins: [[i32; 2]; 4],
}

const LUT_TABLE_SIZE: usize = 256;

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct TransformParams {
    pub(crate) scale: [f32; 2],
    pub(crate) translate: [f32; 2],
    pub(crate) pivot: [f32; 2],
    pub(crate) rotate_radians: f32,
    pub(crate) sampling: u32,
    /// Taps to average over the shutter, or 0 to sample with the fields above alone.
    pub(crate) tap_count: u32,
    /// The mip level that a filtered minification reads.
    pub(crate) lod: f32,
    /// Canvas pixel of the output's first texel.
    pub(crate) output_origin: [i32; 2],
    /// Source pixel of the source's first texel.
    pub(crate) source_origin: [i32; 2],
    /// Texels of the source a pixel covers along its axes when the source is minified, else 0.
    pub(crate) footprint: [f32; 2],
    pub(crate) taps: [TransformTap; crate::node::motion::MAX_MOTION_TAPS],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct TransformTap {
    pub(crate) scale: [f32; 2],
    pub(crate) translate: [f32; 2],
    pub(crate) pivot: [f32; 2],
    pub(crate) rotate_radians: f32,
    /// The mip level that a filtered minification reads.
    pub(crate) lod: f32,
    /// Texels of the source a pixel covers along its axes when the source is minified, else 0.
    pub(crate) footprint: [f32; 2],
    pub(crate) _pad: [u32; 2],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct OpacityParams {
    pub(crate) opacity: f32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub(crate) struct CropParams {
    pub(crate) origin: [i32; 2],
    pub(crate) size: [u32; 2],
}

/// A crop over a source stored from `source_origin`, which is where its first texel sits in the
/// display.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct CropPassParams {
    pub(crate) origin: [i32; 2],
    pub(crate) size: [u32; 2],
    pub(crate) source_origin: [i32; 2],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct ResizeParams {
    pub(crate) origin: [i32; 2],
    pub(crate) size: [u32; 2],
    pub(crate) mode: u32,
    pub(crate) sampling: u32,
    /// Texels of the source a pixel covers along its axes when the source is minified, else 0.
    pub(crate) footprint: [f32; 2],
    /// The mip level that a filtered minification reads.
    pub(crate) lod: f32,
    pub(crate) _pad: u32,
}

/// Converts a signed pixel coordinate property to the `i32` the shaders use.
pub(crate) fn pixel_origin(node_id: NodeId, property_path: &str, value: i64) -> crate::Result<i32> {
    i32::try_from(value).map_err(|_| {
        crate::error::PropertyError::InvalidType {
            node_id,
            property_path: property_path.to_string(),
            expected: "an integer between -2147483648 and 2147483647",
            actual: "out-of-range integer",
        }
        .into()
    })
}

/// Converts a pixel extent property to the `u32` the shaders use; values below `min` clamp to it.
pub(crate) fn pixel_extent(
    node_id: NodeId,
    property_path: &str,
    value: i64,
    min: u32,
) -> crate::Result<u32> {
    u32::try_from(value.max(i64::from(min))).map_err(|_| {
        crate::error::PropertyError::InvalidType {
            node_id,
            property_path: property_path.to_string(),
            expected: "an integer between 0 and 4294967295",
            actual: "out-of-range integer",
        }
        .into()
    })
}

pub(crate) fn dispatch_for(size: lumen_gpu::Size) -> lumen_gpu::Dispatch {
    lumen_gpu::Dispatch {
        x: size.width.div_ceil(8),
        y: size.height.div_ceil(8),
        z: 1,
    }
}

fn parse_lut_stops(node_id: NodeId, source: &str) -> crate::Result<Vec<[f32; 3]>> {
    lut_stops(source).map_err(|details| lut_error(node_id, details))
}

/// Whether `lut` leaves the colour as it is: empty, or `identity`.
pub(crate) fn is_identity_lut(lut: &str) -> bool {
    let lut = lut.trim();
    lut.is_empty() || lut.eq_ignore_ascii_case(crate::node::processing::color_grade::IDENTITY_LUT)
}

/// Whether `source`, meant to name a `.cube` file, is written as an inline LUT, as `lut_source`
/// was before `lut` took that role.
pub(crate) fn looks_like_inline_lut(source: &str) -> bool {
    let source = source.trim();
    source.starts_with("rgb1d:")
        || source.eq_ignore_ascii_case(crate::node::processing::color_grade::IDENTITY_LUT)
        || (source.contains(';')
            && source.starts_with(|c: char| c.is_ascii_digit() || "+-.".contains(c)))
}

pub(crate) const INLINE_LUT_IN_LUT_SOURCE: &str = "`lut_source` names a .cube file; an inline LUT is the `lut` param, on a 0..1 scale (divide 0..255 values by 255)";

/// The stops of the inline LUT `source`, or what is wrong with it.
pub(crate) fn lut_stops(source: &str) -> Result<Vec<[f32; 3]>, String> {
    let source = source.trim();
    if is_identity_lut(source) {
        return Ok(vec![[0.0, 0.0, 0.0], [1.0, 1.0, 1.0]]);
    }

    let source = source
        .strip_prefix("rgb1d")
        .and_then(|rest| rest.strip_prefix(':'))
        .unwrap_or(source);
    let mut stops = Vec::new();
    for triplet in source.split(';') {
        let triplet = triplet.trim();
        if triplet.is_empty() {
            continue;
        }
        let components = triplet
            .split([',', ' ', '\t'])
            .filter(|part| !part.is_empty())
            // `str::parse` also accepts `nan` and `inf`, which no clamp below would remove.
            .map(|part| part.parse::<f32>().ok().filter(|value| value.is_finite()))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| "`lut` components must be finite numbers".to_string())?;
        if components.len() != 3 {
            return Err(format!(
                "`lut` triplet `{triplet}` must contain exactly three RGB components"
            ));
        }
        if let Some(value) = components
            .iter()
            .find(|value| !(0.0..=1.0).contains(*value))
        {
            return Err(format!(
                "`lut` value {value} in `{triplet}` is outside 0..1; the scale is 0..1, so divide 0..255 values by 255"
            ));
        }
        stops.push([components[0], components[1], components[2]]);
    }
    if stops.len() < 2 {
        return Err("`lut` must contain at least two RGB triplets".to_string());
    }
    Ok(stops)
}

pub(crate) fn lut_error(node_id: NodeId, details: impl Into<String>) -> crate::error::LumenError {
    RenderError::NodeEvaluation {
        node_id,
        node_kind: <crate::node::processing::color_grade::ColorGrade as JsonNode>::KIND,
        details: details.into(),
    }
    .into()
}

pub(crate) fn copyable_texture_desc(size: lumen_gpu::Size) -> lumen_gpu::TextureDesc {
    lumen_gpu::TextureDesc {
        domain: lumen_gpu::TextureDomain::full_frame(size),
        format: super::WORKING_COLOR_FORMAT,
        usage: lumen_gpu::wgpu::TextureUsages::COPY_DST
            | lumen_gpu::wgpu::TextureUsages::COPY_SRC
            | lumen_gpu::wgpu::TextureUsages::TEXTURE_BINDING
            | lumen_gpu::wgpu::TextureUsages::STORAGE_BINDING
            | lumen_gpu::wgpu::TextureUsages::RENDER_ATTACHMENT,
    }
}
