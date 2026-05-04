//! Node construction from JSON for the renderer-agnostic node schema.

use std::{collections::HashMap, ops::Range};

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::{
    graph::Graph,
    node::{
        NodeId, NodeKind, NodeProperty, PortRef, PropertyDef,
        compositing::{
            boolean::Boolean, merge::Merge, raster_multimerge::RasterMultiMerge, switch::Switch,
        },
        media_output::MediaOutput,
        processing::{
            alpha_premultiply::AlphaPremultiply, blur::Blur, channel_shuffle::ChannelShuffle,
            color_grade::ColorGrade, crop::Crop, curves::Curves, exposure::Exposure,
            hue_saturation::HueSaturation, levels::Levels, memo::Memo, resize::Resize,
            shadow::Shadow, time_remap::TimeRemap, transform::Transform, wgsl_shader::WgslShader,
        },
        source::{media_in::MediaIn, solid_color::SolidColor, text::Text},
        vector::{path::Path, shape::Shape},
    },
};

use super::property::parse_property;

pub fn build_node(
    kind: &str,
    id: NodeId,
    obj: &serde_json::Map<String, Value>,
) -> Result<NodeKind> {
    let properties = obj.get("properties").and_then(Value::as_object);

    match kind {
        "media_in" => Ok(NodeKind::MediaIn(build_media_in(id, properties)?)),
        "solid_color" => Ok(NodeKind::SolidColor(build_solid_color(id, properties)?)),
        "text" => Ok(NodeKind::Text(build_text(id, properties)?)),
        "path" => Ok(NodeKind::Path(build_path(id, properties)?)),
        "shape" => Ok(NodeKind::Shape(build_shape(id, properties)?)),
        "boolean" => Ok(NodeKind::Boolean(build_boolean(id, properties)?)),
        "merge" => Ok(NodeKind::Merge(build_merge(id, properties)?)),
        "raster_multimerge" => Ok(NodeKind::RasterMultiMerge(build_raster_multimerge(
            id, properties,
        )?)),
        "alpha_premultiply" => Ok(NodeKind::AlphaPremultiply(build_alpha_premultiply(
            id, properties,
        )?)),
        "blur" => Ok(NodeKind::Blur(build_blur(id, properties)?)),
        "channel_shuffle" => Ok(NodeKind::ChannelShuffle(build_channel_shuffle(
            id, properties,
        )?)),
        "color_grade" => Ok(NodeKind::ColorGrade(build_color_grade(id, properties)?)),
        "curves" => Ok(NodeKind::Curves(build_curves(id, properties)?)),
        "exposure" => Ok(NodeKind::Exposure(build_exposure(id, properties)?)),
        "hue_saturation" => Ok(NodeKind::HueSaturation(build_hue_saturation(
            id, properties,
        )?)),
        "levels" => Ok(NodeKind::Levels(build_levels(id, properties)?)),
        "memo" => Ok(NodeKind::Memo(build_memo(id, properties)?)),
        "time_remap" => Ok(NodeKind::TimeRemap(build_time_remap(id, properties)?)),
        "transform" => Ok(NodeKind::Transform(build_transform(id, properties)?)),
        "crop" => Ok(NodeKind::Crop(build_crop(id, properties)?)),
        "resize" => Ok(NodeKind::Resize(build_resize(id, properties)?)),
        "shadow" => Ok(NodeKind::Shadow(build_shadow(id, properties)?)),
        "wgsl_shader" => Ok(NodeKind::WgslShader(build_wgsl_shader(id, properties)?)),
        "switch" => Ok(NodeKind::Switch(build_switch(id, obj)?)),
        "media_output" => Ok(NodeKind::MediaOutput(MediaOutput {
            id,
            ..MediaOutput::default()
        })),
        other => bail!("unknown or unsupported node type `{other}`"),
    }
}

pub fn wire_port_ref(
    graph: &mut Graph,
    to_node: NodeId,
    to_port: &str,
    from_node: NodeId,
    from_port: &str,
) -> Result<()> {
    let port_ref = PortRef::new(from_node, from_port.to_string());
    let node = graph
        .nodes
        .get_mut(&to_node)
        .with_context(|| format!("node {to_node} not found for wiring"))?;

    let wired = match node {
        NodeKind::AlphaPremultiply(node) if to_port == "source" => {
            node.source = port_ref;
            true
        }
        NodeKind::Blur(node) if to_port == "source" => {
            node.source = port_ref;
            true
        }
        NodeKind::ChannelShuffle(node) if to_port == "source" => {
            node.source = port_ref;
            true
        }
        NodeKind::ColorGrade(node) if to_port == "source" => {
            node.source = port_ref;
            true
        }
        NodeKind::Curves(node) if to_port == "source" => {
            node.source = port_ref;
            true
        }
        NodeKind::Exposure(node) if to_port == "source" => {
            node.source = port_ref;
            true
        }
        NodeKind::HueSaturation(node) if to_port == "source" => {
            node.source = port_ref;
            true
        }
        NodeKind::Levels(node) if to_port == "source" => {
            node.source = port_ref;
            true
        }
        NodeKind::Memo(node) if to_port == "source" => {
            node.source = port_ref;
            true
        }
        NodeKind::TimeRemap(node) if to_port == "source" => {
            node.source = port_ref;
            true
        }
        NodeKind::Transform(node) if to_port == "source" => {
            node.source = port_ref;
            true
        }
        NodeKind::Crop(node) if to_port == "source" => {
            node.source = port_ref;
            true
        }
        NodeKind::Resize(node) if to_port == "source" => {
            node.source = port_ref;
            true
        }
        NodeKind::Shadow(node) if to_port == "source" => {
            node.source = port_ref;
            true
        }
        NodeKind::WgslShader(node) if to_port == "source" => {
            node.source = port_ref;
            true
        }
        NodeKind::Boolean(node) if to_port == "a" => {
            node.a = port_ref;
            true
        }
        NodeKind::Boolean(node) if to_port == "b" => {
            node.b = port_ref;
            true
        }
        NodeKind::Merge(node) if to_port == "base" => {
            node.base = port_ref;
            true
        }
        NodeKind::Merge(node) if to_port == "overlay" => {
            node.overlay = port_ref;
            true
        }
        NodeKind::Merge(node) if to_port == "mask" => {
            node.mask = port_ref;
            true
        }
        NodeKind::RasterMultiMerge(node) if to_port == "layers" => {
            node.layers.push(port_ref);
            true
        }
        NodeKind::Switch(node) if to_port == "layers" => {
            node.layers.push(port_ref);
            true
        }
        NodeKind::MediaOutput(node) if to_port == "source" => {
            node.source = port_ref;
            true
        }
        _ => false,
    };

    if !wired {
        bail!("unknown input port `{to_port}` on node {to_node}");
    }

    Ok(())
}

fn build_media_in(
    id: NodeId,
    properties: Option<&serde_json::Map<String, Value>>,
) -> Result<MediaIn> {
    let mut node = MediaIn {
        id,
        ..MediaIn::default()
    };

    set_property(properties, "kind", &mut node.kind, None)?;
    set_property(properties, "source", &mut node.source, None)?;
    set_property(properties, "range_start", &mut node.range_start, None)?;
    set_property(properties, "range_end", &mut node.range_end, None)?;
    set_property(properties, "speed", &mut node.speed, None)?;
    set_property(properties, "loop_mode", &mut node.loop_mode, None)?;
    reject_unknown(
        properties,
        &[
            "kind",
            "source",
            "asset",
            "range_start",
            "range_end",
            "speed",
            "loop_mode",
        ],
        id,
    )?;
    Ok(node)
}

fn build_solid_color(
    id: NodeId,
    properties: Option<&serde_json::Map<String, Value>>,
) -> Result<SolidColor> {
    let mut node = SolidColor {
        id,
        ..SolidColor::default()
    };

    set_property(
        properties,
        "color",
        &mut node.color,
        Some(PropertyDef {
            name: "color",
            expected: crate::node::PropertyKind::Color,
        }),
    )?;
    set_property(properties, "width", &mut node.width, None)?;
    set_property(properties, "height", &mut node.height, None)?;
    reject_unknown(properties, &["color", "width", "height"], id)?;
    Ok(node)
}

fn build_text(id: NodeId, properties: Option<&serde_json::Map<String, Value>>) -> Result<Text> {
    let mut node = Text {
        id,
        ..Text::default()
    };

    set_property(properties, "content", &mut node.content, None)?;
    set_property(properties, "font_family", &mut node.font_family, None)?;
    set_property(properties, "font_size", &mut node.font_size, None)?;
    set_property(properties, "font_weight", &mut node.font_weight, None)?;
    set_property(properties, "font_style", &mut node.font_style, None)?;
    set_property(properties, "max_width", &mut node.max_width, None)?;
    set_property(
        properties,
        "position",
        &mut node.position,
        Some(PropertyDef {
            name: "position",
            expected: crate::node::PropertyKind::Vec2,
        }),
    )?;
    set_property(
        properties,
        "color",
        &mut node.color,
        Some(PropertyDef {
            name: "color",
            expected: crate::node::PropertyKind::Color,
        }),
    )?;
    set_property(
        properties,
        "alignment_horizontal",
        &mut node.alignment_horizontal,
        None,
    )?;
    set_property(
        properties,
        "alignment_vertical",
        &mut node.alignment_vertical,
        None,
    )?;
    reject_unknown(
        properties,
        &[
            "content",
            "font_family",
            "font_size",
            "font_weight",
            "font_style",
            "max_width",
            "position",
            "color",
            "alignment_horizontal",
            "alignment_vertical",
        ],
        id,
    )?;
    Ok(node)
}

fn build_shape(id: NodeId, properties: Option<&serde_json::Map<String, Value>>) -> Result<Shape> {
    let mut node = Shape {
        id,
        ..Shape::default()
    };

    set_property(properties, "geometry_kind", &mut node.geometry_kind, None)?;
    set_property(properties, "width", &mut node.width, None)?;
    set_property(properties, "height", &mut node.height, None)?;
    set_property(properties, "border_radius", &mut node.border_radius, None)?;
    set_property(properties, "polygon_points", &mut node.polygon_points, None)?;
    set_property(
        properties,
        "position",
        &mut node.position,
        Some(PropertyDef {
            name: "position",
            expected: crate::node::PropertyKind::Vec2,
        }),
    )?;
    set_property(properties, "fill_enabled", &mut node.fill_enabled, None)?;
    set_property(
        properties,
        "fill_color",
        &mut node.fill_color,
        Some(PropertyDef {
            name: "fill_color",
            expected: crate::node::PropertyKind::Color,
        }),
    )?;
    set_property(properties, "stroke_enabled", &mut node.stroke_enabled, None)?;
    set_property(
        properties,
        "stroke_color",
        &mut node.stroke_color,
        Some(PropertyDef {
            name: "stroke_color",
            expected: crate::node::PropertyKind::Color,
        }),
    )?;
    set_property(properties, "stroke_width", &mut node.stroke_width, None)?;
    reject_unknown(
        properties,
        &[
            "geometry_kind",
            "width",
            "height",
            "border_radius",
            "polygon_points",
            "position",
            "fill_enabled",
            "fill_color",
            "stroke_enabled",
            "stroke_color",
            "stroke_width",
        ],
        id,
    )?;
    Ok(node)
}

fn build_path(id: NodeId, properties: Option<&serde_json::Map<String, Value>>) -> Result<Path> {
    let mut node = Path {
        id,
        ..Path::default()
    };

    set_property(properties, "data", &mut node.data, None)?;
    set_property(
        properties,
        "position",
        &mut node.position,
        Some(PropertyDef {
            name: "position",
            expected: crate::node::PropertyKind::Vec2,
        }),
    )?;
    set_property(properties, "fill_enabled", &mut node.fill_enabled, None)?;
    set_property(
        properties,
        "fill_color",
        &mut node.fill_color,
        Some(PropertyDef {
            name: "fill_color",
            expected: crate::node::PropertyKind::Color,
        }),
    )?;
    set_property(properties, "stroke_enabled", &mut node.stroke_enabled, None)?;
    set_property(
        properties,
        "stroke_color",
        &mut node.stroke_color,
        Some(PropertyDef {
            name: "stroke_color",
            expected: crate::node::PropertyKind::Color,
        }),
    )?;
    set_property(properties, "stroke_width", &mut node.stroke_width, None)?;
    reject_unknown(
        properties,
        &[
            "data",
            "position",
            "fill_enabled",
            "fill_color",
            "stroke_enabled",
            "stroke_color",
            "stroke_width",
        ],
        id,
    )?;
    Ok(node)
}

fn build_merge(id: NodeId, properties: Option<&serde_json::Map<String, Value>>) -> Result<Merge> {
    let mut node = Merge {
        id,
        ..Merge::default()
    };

    set_property(properties, "opacity", &mut node.opacity, None)?;
    set_property(properties, "blend_mode", &mut node.blend_mode, None)?;
    reject_unknown(properties, &["opacity", "blend_mode"], id)?;
    Ok(node)
}

fn build_boolean(
    id: NodeId,
    properties: Option<&serde_json::Map<String, Value>>,
) -> Result<Boolean> {
    let mut node = Boolean {
        id,
        ..Boolean::default()
    };

    set_property(properties, "operation", &mut node.operation, None)?;
    set_property(properties, "threshold", &mut node.threshold, None)?;
    reject_unknown(properties, &["operation", "threshold"], id)?;
    Ok(node)
}

fn build_raster_multimerge(
    id: NodeId,
    properties: Option<&serde_json::Map<String, Value>>,
) -> Result<RasterMultiMerge> {
    let mut node = RasterMultiMerge {
        id,
        ..RasterMultiMerge::default()
    };

    set_property(properties, "opacity", &mut node.opacity, None)?;
    set_property(properties, "blend_mode", &mut node.blend_mode, None)?;
    reject_unknown(properties, &["opacity", "blend_mode"], id)?;
    Ok(node)
}

fn build_alpha_premultiply(
    id: NodeId,
    properties: Option<&serde_json::Map<String, Value>>,
) -> Result<AlphaPremultiply> {
    let mut node = AlphaPremultiply {
        id,
        ..AlphaPremultiply::default()
    };

    set_property(properties, "mode", &mut node.mode, None)?;
    reject_unknown(properties, &["mode"], id)?;
    Ok(node)
}

fn build_blur(id: NodeId, properties: Option<&serde_json::Map<String, Value>>) -> Result<Blur> {
    let mut node = Blur {
        id,
        ..Blur::default()
    };

    set_property(properties, "radius", &mut node.radius, None)?;
    reject_unknown(properties, &["radius"], id)?;
    Ok(node)
}

fn build_channel_shuffle(
    id: NodeId,
    properties: Option<&serde_json::Map<String, Value>>,
) -> Result<ChannelShuffle> {
    let mut node = ChannelShuffle {
        id,
        ..ChannelShuffle::default()
    };

    set_property(properties, "red", &mut node.red, None)?;
    set_property(properties, "green", &mut node.green, None)?;
    set_property(properties, "blue", &mut node.blue, None)?;
    set_property(properties, "alpha", &mut node.alpha, None)?;
    reject_unknown(properties, &["red", "green", "blue", "alpha"], id)?;
    Ok(node)
}

fn build_color_grade(
    id: NodeId,
    properties: Option<&serde_json::Map<String, Value>>,
) -> Result<ColorGrade> {
    let mut node = ColorGrade {
        id,
        ..ColorGrade::default()
    };

    set_property(properties, "lut_source", &mut node.lut_source, None)?;
    set_property(properties, "strength", &mut node.strength, None)?;
    set_property(properties, "interpolation", &mut node.interpolation, None)?;
    reject_unknown(properties, &["lut_source", "strength", "interpolation"], id)?;
    Ok(node)
}

fn build_curves(id: NodeId, properties: Option<&serde_json::Map<String, Value>>) -> Result<Curves> {
    let mut node = Curves {
        id,
        ..Curves::default()
    };

    set_property(properties, "curve_source", &mut node.curve_source, None)?;
    set_property(properties, "strength", &mut node.strength, None)?;
    reject_unknown(properties, &["curve_source", "strength"], id)?;
    Ok(node)
}

fn build_exposure(
    id: NodeId,
    properties: Option<&serde_json::Map<String, Value>>,
) -> Result<Exposure> {
    let mut node = Exposure {
        id,
        ..Exposure::default()
    };

    set_property(properties, "exposure", &mut node.exposure, None)?;
    set_property(properties, "contrast", &mut node.contrast, None)?;
    set_property(properties, "offset", &mut node.offset, None)?;
    reject_unknown(properties, &["exposure", "contrast", "offset"], id)?;
    Ok(node)
}

fn build_hue_saturation(
    id: NodeId,
    properties: Option<&serde_json::Map<String, Value>>,
) -> Result<HueSaturation> {
    let mut node = HueSaturation {
        id,
        ..HueSaturation::default()
    };

    set_property(properties, "hue_degrees", &mut node.hue_degrees, None)?;
    set_property(properties, "saturation", &mut node.saturation, None)?;
    set_property(properties, "lightness", &mut node.lightness, None)?;
    reject_unknown(properties, &["hue_degrees", "saturation", "lightness"], id)?;
    Ok(node)
}

fn build_levels(id: NodeId, properties: Option<&serde_json::Map<String, Value>>) -> Result<Levels> {
    let mut node = Levels {
        id,
        ..Levels::default()
    };

    set_property(properties, "black_point", &mut node.black_point, None)?;
    set_property(properties, "white_point", &mut node.white_point, None)?;
    set_property(properties, "gamma", &mut node.gamma, None)?;
    set_property(properties, "output_black", &mut node.output_black, None)?;
    set_property(properties, "output_white", &mut node.output_white, None)?;
    reject_unknown(
        properties,
        &[
            "black_point",
            "white_point",
            "gamma",
            "output_black",
            "output_white",
        ],
        id,
    )?;
    Ok(node)
}

fn build_memo(id: NodeId, properties: Option<&serde_json::Map<String, Value>>) -> Result<Memo> {
    let mut node = Memo {
        id,
        ..Memo::default()
    };

    set_property(properties, "cache_id", &mut node.cache_id, None)?;
    set_property(
        properties,
        "allow_expressions",
        &mut node.allow_expressions,
        None,
    )?;
    reject_unknown(properties, &["cache_id", "allow_expressions"], id)?;
    Ok(node)
}

fn build_time_remap(
    id: NodeId,
    properties: Option<&serde_json::Map<String, Value>>,
) -> Result<TimeRemap> {
    let mut node = TimeRemap {
        id,
        ..TimeRemap::default()
    };

    set_property(properties, "frame", &mut node.frame, None)?;
    set_property(properties, "loop_enabled", &mut node.loop_enabled, None)?;
    set_property(properties, "loop_start", &mut node.loop_start, None)?;
    set_property(properties, "loop_end", &mut node.loop_end, None)?;
    reject_unknown(
        properties,
        &["frame", "loop_enabled", "loop_start", "loop_end"],
        id,
    )?;
    Ok(node)
}

fn build_transform(
    id: NodeId,
    properties: Option<&serde_json::Map<String, Value>>,
) -> Result<Transform> {
    let mut node = Transform {
        id,
        ..Transform::default()
    };

    set_property(properties, "scale_x", &mut node.scale_x, None)?;
    set_property(properties, "scale_y", &mut node.scale_y, None)?;
    set_property(properties, "translate_x", &mut node.translate_x, None)?;
    set_property(properties, "translate_y", &mut node.translate_y, None)?;
    set_property(properties, "rotate", &mut node.rotate, None)?;
    set_property(properties, "pivot_x", &mut node.pivot_x, None)?;
    set_property(properties, "pivot_y", &mut node.pivot_y, None)?;
    set_property(properties, "sampling", &mut node.sampling, None)?;
    reject_unknown(
        properties,
        &[
            "scale_x",
            "scale_y",
            "translate_x",
            "translate_y",
            "rotate",
            "pivot_x",
            "pivot_y",
            "sampling",
        ],
        id,
    )?;
    Ok(node)
}

fn build_crop(id: NodeId, properties: Option<&serde_json::Map<String, Value>>) -> Result<Crop> {
    let mut node = Crop {
        id,
        ..Crop::default()
    };

    set_property(properties, "x", &mut node.x, None)?;
    set_property(properties, "y", &mut node.y, None)?;
    set_property(properties, "width", &mut node.width, None)?;
    set_property(properties, "height", &mut node.height, None)?;
    reject_unknown(properties, &["x", "y", "width", "height"], id)?;
    Ok(node)
}

fn build_resize(id: NodeId, properties: Option<&serde_json::Map<String, Value>>) -> Result<Resize> {
    let mut node = Resize {
        id,
        ..Resize::default()
    };

    set_property(properties, "width", &mut node.width, None)?;
    set_property(properties, "height", &mut node.height, None)?;
    set_property(properties, "mode", &mut node.mode, None)?;
    set_property(properties, "sampling", &mut node.sampling, None)?;
    reject_unknown(properties, &["width", "height", "mode", "sampling"], id)?;
    Ok(node)
}

fn build_shadow(id: NodeId, properties: Option<&serde_json::Map<String, Value>>) -> Result<Shadow> {
    let mut node = Shadow {
        id,
        ..Shadow::default()
    };

    set_property(properties, "offset_x", &mut node.offset_x, None)?;
    set_property(properties, "offset_y", &mut node.offset_y, None)?;
    set_property(properties, "radius", &mut node.radius, None)?;
    set_property(
        properties,
        "color",
        &mut node.color,
        Some(PropertyDef {
            name: "color",
            expected: crate::node::PropertyKind::Color,
        }),
    )?;
    set_property(properties, "opacity", &mut node.opacity, None)?;
    reject_unknown(
        properties,
        &["offset_x", "offset_y", "radius", "color", "opacity"],
        id,
    )?;
    Ok(node)
}

fn build_wgsl_shader(
    id: NodeId,
    properties: Option<&serde_json::Map<String, Value>>,
) -> Result<WgslShader> {
    let mut node = WgslShader {
        id,
        ..WgslShader::default()
    };

    set_property(properties, "shader", &mut node.shader, None)?;
    set_property(properties, "value0", &mut node.value0, None)?;
    set_property(properties, "value1", &mut node.value1, None)?;
    set_property(properties, "value2", &mut node.value2, None)?;
    set_property(properties, "value3", &mut node.value3, None)?;
    reject_unknown(
        properties,
        &["shader", "value0", "value1", "value2", "value3"],
        id,
    )?;
    Ok(node)
}

fn build_switch(id: NodeId, obj: &serde_json::Map<String, Value>) -> Result<Switch> {
    let mut node = Switch {
        id,
        ..Switch::default()
    };
    if let Some(map) = obj.get("map") {
        node.map = parse_switch_map(map)?;
    }
    Ok(node)
}

fn set_property(
    properties: Option<&serde_json::Map<String, Value>>,
    name: &str,
    target: &mut NodeProperty,
    def: Option<PropertyDef>,
) -> Result<()> {
    let Some(value) = properties.and_then(|properties| properties.get(name)) else {
        return Ok(());
    };
    *target = parse_property(value, def.as_ref(), name)?;
    Ok(())
}

fn reject_unknown(
    properties: Option<&serde_json::Map<String, Value>>,
    known: &[&str],
    id: NodeId,
) -> Result<()> {
    let Some(properties) = properties else {
        return Ok(());
    };

    for key in properties.keys() {
        if !known.iter().any(|known| known == &key.as_str()) {
            bail!("unknown property `{key}` on node {id}");
        }
    }

    Ok(())
}

fn parse_switch_map(val: &Value) -> Result<HashMap<u16, Range<u32>>> {
    let obj = val.as_object().context("switch `map` must be an object")?;
    let mut result = HashMap::new();
    for (key, range_val) in obj {
        let index: u16 = key
            .parse()
            .with_context(|| format!("switch map key `{key}`"))?;
        let arr = range_val
            .as_array()
            .with_context(|| format!("switch map value for `{key}` must be [start, end]"))?;
        if arr.len() != 2 {
            bail!("switch map range must be [start, end]");
        }
        let start = arr[0].as_u64().context("range start")? as u32;
        let end = arr[1].as_u64().context("range end")? as u32;
        result.insert(index, start..end);
    }
    Ok(result)
}
