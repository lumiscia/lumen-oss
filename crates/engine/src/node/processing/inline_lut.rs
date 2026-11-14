use bytemuck::{Pod, Zeroable};

use crate::{error::RenderError, node::NodeId};

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable, lumen_macros::ShaderLayout)]
#[shader_layout(wgsl = "ColorGradeLut", source = super::color_grade::SHADER)]
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

const LUT_TABLE_SIZE: usize = 256;

fn parse_lut_stops(node_id: NodeId, source: &str) -> crate::Result<Vec<[f32; 3]>> {
    lut_stops(source).map_err(|details| lut_error(node_id, details))
}

/// Whether `lut` leaves the colour as it is: empty, or `identity`.
pub(crate) fn is_identity_lut(lut: &str) -> bool {
    let lut = lut.trim();
    lut.is_empty() || lut.eq_ignore_ascii_case(super::color_grade::IDENTITY_LUT)
}

/// Whether `source`, meant to name a `.cube` file, is written as an inline LUT, as `lut_source`
/// was before `lut` took that role.
pub(crate) fn looks_like_inline_lut(source: &str) -> bool {
    let source = source.trim();
    source.starts_with("rgb1d:")
        || source.eq_ignore_ascii_case(super::color_grade::IDENTITY_LUT)
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
    RenderError::evaluating::<super::color_grade::ColorGrade>(node_id, details.into()).into()
}
