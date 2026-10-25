use bytemuck::{Pod, Zeroable};

use crate::{error::RenderError, node::NodeId};

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct ColorParams {
    pub(crate) color: [f32; 4],
}

impl ColorParams {
    pub(crate) fn from_rgba8(color: [u8; 4]) -> Self {
        Self {
            color: [
                f32::from(color[0]) / 255.0,
                f32::from(color[1]) / 255.0,
                f32::from(color[2]) / 255.0,
                f32::from(color[3]) / 255.0,
            ],
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct AlphaPremultiplyParams {
    pub(crate) values: [f32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct ChannelShuffleParams {
    pub(crate) selector_indices: [f32; 4],
    pub(crate) selector_values: [f32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct ColorGradeParams {
    pub(crate) strength: f32,
    pub(crate) interpolation: u32,
    pub(crate) _pad: [u32; 2],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct ColorGradeLut {
    stops: [[f32; 4]; LUT_TABLE_SIZE],
}

impl ColorGradeLut {
    pub(crate) fn parse(node_id: NodeId, frame: u32, source: &str) -> crate::Result<Self> {
        let stops = parse_lut_stops(node_id, frame, source)?;
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
    pub(crate) _pad: f32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct HueSaturationParams {
    pub(crate) hue_offset: f32,
    pub(crate) saturation: f32,
    pub(crate) lightness: f32,
    pub(crate) _pad: f32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct LevelsParams {
    pub(crate) black_point: f32,
    pub(crate) white_point: f32,
    pub(crate) gamma: f32,
    pub(crate) output_black: f32,
    pub(crate) output_white: f32,
    pub(crate) _pad: [f32; 3],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct BlurParams {
    pub(crate) values: [u32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct CurvesParams {
    pub(crate) values: [f32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct CurvesTable {
    entries: [[f32; 4]; LUT_TABLE_SIZE],
}

impl CurvesTable {
    pub(crate) fn parse(node_id: NodeId, frame: u32, source: &str) -> crate::Result<Self> {
        let stops = parse_lut_stops(node_id, frame, source)?;
        let mut entries = [[0.0; 4]; LUT_TABLE_SIZE];
        for (index, entry) in entries.iter_mut().enumerate() {
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
        Ok(Self { entries })
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct ShadowParams {
    pub(crate) color: [f32; 4],
    pub(crate) values: [f32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct WgslShaderParams {
    pub(crate) values: [f32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct MergeParams {
    pub(crate) opacity: f32,
    pub(crate) blend_mode: u32,
    pub(crate) has_mask: u32,
    pub(crate) _pad: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct BooleanParams {
    pub(crate) values: [f32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct RasterMultiMergeParams {
    pub(crate) values: [f32; 4],
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ChannelSelector {
    pub(crate) index: f32,
    pub(crate) value: f32,
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
    pub(crate) _pad: [u32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct OpacityParams {
    pub(crate) values: [f32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct CropParams {
    pub(crate) origin: [i32; 2],
    pub(crate) size: [u32; 2],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct ResizeParams {
    pub(crate) size: [u32; 2],
    pub(crate) mode: u32,
    pub(crate) sampling: u32,
}

pub(crate) fn dispatch_for(size: lumen_gpu::Size) -> lumen_gpu::Dispatch {
    lumen_gpu::Dispatch {
        x: size.width.div_ceil(8),
        y: size.height.div_ceil(8),
        z: 1,
    }
}

pub(crate) fn spatial_bindings(
    input: lumen_gpu::TextureId,
    params: lumen_gpu::BufferId,
    output: lumen_gpu::TextureId,
) -> Vec<lumen_gpu::Binding> {
    vec![
        lumen_gpu::Binding::sampled_texture(0, 0, input),
        lumen_gpu::Binding::uniform(0, 1, params),
        lumen_gpu::Binding::storage_texture(0, 2, output),
    ]
}

pub(crate) fn alpha_operation(node_id: NodeId, mode: &str) -> crate::Result<f32> {
    match mode.trim().to_ascii_lowercase().as_str() {
        "premultiply" | "premul" | "multiply" => Ok(0.0),
        "unpremultiply" | "unpremul" | "straight" | "unmultiply" => Ok(1.0),
        _ => Err(crate::error::PropertyError::InvalidType {
            node_id,
            property_path: "mode".to_string(),
            expected: "`premultiply` or `unpremultiply`",
            actual: "String",
        }
        .into()),
    }
}

pub(crate) fn channel_selector(
    node_id: NodeId,
    property_path: &str,
    spec: &str,
) -> crate::Result<ChannelSelector> {
    let normalized = spec.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "r" | "red" => Ok(ChannelSelector {
            index: 0.0,
            value: 0.0,
        }),
        "g" | "green" => Ok(ChannelSelector {
            index: 1.0,
            value: 0.0,
        }),
        "b" | "blue" => Ok(ChannelSelector {
            index: 2.0,
            value: 0.0,
        }),
        "a" | "alpha" => Ok(ChannelSelector {
            index: 3.0,
            value: 0.0,
        }),
        "zero" => Ok(ChannelSelector {
            index: 4.0,
            value: 0.0,
        }),
        "one" => Ok(ChannelSelector {
            index: 4.0,
            value: 1.0,
        }),
        _ => {
            let value = normalized.parse::<f32>().map_err(|_| {
                crate::error::PropertyError::InvalidType {
                    node_id,
                    property_path: property_path.to_string(),
                    expected: "channel name or numeric constant",
                    actual: "String",
                }
            })?;
            Ok(ChannelSelector {
                index: 4.0,
                value: if value <= 1.0 {
                    value.clamp(0.0, 1.0)
                } else {
                    (value / 255.0).clamp(0.0, 1.0)
                },
            })
        }
    }
}

fn parse_lut_stops(node_id: NodeId, frame: u32, source: &str) -> crate::Result<Vec<[f32; 3]>> {
    let source = source.trim();
    if source.is_empty()
        || source.eq_ignore_ascii_case(crate::node::processing::color_grade::IDENTITY_LUT)
    {
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
            .map(str::parse::<f32>)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| lut_error(node_id, frame, "LUT contains a non-numeric component"))?;
        if components.len() != 3 {
            return Err(lut_error(
                node_id,
                frame,
                format!("LUT triplet `{triplet}` must contain exactly three RGB components"),
            ));
        }
        stops.push([
            normalize_lut_component(components[0]),
            normalize_lut_component(components[1]),
            normalize_lut_component(components[2]),
        ]);
    }
    if stops.len() < 2 {
        return Err(lut_error(
            node_id,
            frame,
            "LUT must contain at least two RGB triplets",
        ));
    }
    Ok(stops)
}

fn normalize_lut_component(value: f32) -> f32 {
    if value > 1.0 {
        (value / 255.0).clamp(0.0, 1.0)
    } else {
        value.clamp(0.0, 1.0)
    }
}

fn lut_error(node_id: NodeId, frame: u32, details: impl Into<String>) -> crate::error::LumenError {
    RenderError::NodeEvaluation {
        frame,
        node_id,
        node_kind: "ColorGrade",
        details: details.into(),
    }
    .into()
}

pub(crate) fn copyable_texture_desc(size: lumen_gpu::Size) -> lumen_gpu::TextureDesc {
    lumen_gpu::TextureDesc {
        domain: lumen_gpu::TextureDomain::full_frame(size),
        format: lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
        usage: lumen_gpu::wgpu::TextureUsages::COPY_DST
            | lumen_gpu::wgpu::TextureUsages::COPY_SRC
            | lumen_gpu::wgpu::TextureUsages::TEXTURE_BINDING
            | lumen_gpu::wgpu::TextureUsages::STORAGE_BINDING
            | lumen_gpu::wgpu::TextureUsages::RENDER_ATTACHMENT,
    }
}
