//! Node construction from JSON for the renderer-agnostic node schema.

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::{
    graph::Graph,
    node::{
        JsonNode, NodeId, NodeKind, PortRef,
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

pub fn build_node(
    kind: &str,
    id: NodeId,
    obj: &serde_json::Map<String, Value>,
) -> Result<NodeKind> {
    let properties = obj.get("properties").and_then(Value::as_object);

    match kind {
        "media_in" => Ok(NodeKind::MediaIn(MediaIn::from_json(id, properties)?)),
        "solid_color" => Ok(NodeKind::SolidColor(SolidColor::from_json(id, properties)?)),
        "text" => Ok(NodeKind::Text(Text::from_json(id, properties)?)),
        "path" => Ok(NodeKind::Path(Path::from_json(id, properties)?)),
        "shape" => Ok(NodeKind::Shape(Shape::from_json(id, properties)?)),
        "boolean" => Ok(NodeKind::Boolean(Boolean::from_json(id, properties)?)),
        "merge" => Ok(NodeKind::Merge(Merge::from_json(id, properties)?)),
        "raster_multimerge" => Ok(NodeKind::RasterMultiMerge(RasterMultiMerge::from_json(
            id, properties,
        )?)),
        "switch" => Ok(NodeKind::Switch(Switch::from_json(id, properties)?)),
        "memo" => Ok(NodeKind::Memo(Memo::from_json(id, properties)?)),
        "alpha_premultiply" => Ok(NodeKind::AlphaPremultiply(AlphaPremultiply::from_json(
            id, properties,
        )?)),
        "blur" => Ok(NodeKind::Blur(Blur::from_json(id, properties)?)),
        "channel_shuffle" => Ok(NodeKind::ChannelShuffle(ChannelShuffle::from_json(
            id, properties,
        )?)),
        "color_grade" => Ok(NodeKind::ColorGrade(ColorGrade::from_json(id, properties)?)),
        "curves" => Ok(NodeKind::Curves(Curves::from_json(id, properties)?)),
        "exposure" => Ok(NodeKind::Exposure(Exposure::from_json(id, properties)?)),
        "hue_saturation" => Ok(NodeKind::HueSaturation(HueSaturation::from_json(
            id, properties,
        )?)),
        "levels" => Ok(NodeKind::Levels(Levels::from_json(id, properties)?)),
        "time_remap" => Ok(NodeKind::TimeRemap(TimeRemap::from_json(id, properties)?)),
        "transform" => Ok(NodeKind::Transform(Transform::from_json(id, properties)?)),
        "crop" => Ok(NodeKind::Crop(Crop::from_json(id, properties)?)),
        "resize" => Ok(NodeKind::Resize(Resize::from_json(id, properties)?)),
        "shadow" => Ok(NodeKind::Shadow(Shadow::from_json(id, properties)?)),
        "wgsl_shader" => Ok(NodeKind::WgslShader(WgslShader::from_json(id, properties)?)),
        "media_output" => Ok(NodeKind::MediaOutput(MediaOutput::from_json(
            id, properties,
        )?)),
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

    match node {
        NodeKind::MediaIn(node) => node.set_input_json(to_port, port_ref)?,
        NodeKind::SolidColor(node) => node.set_input_json(to_port, port_ref)?,
        NodeKind::Text(node) => node.set_input_json(to_port, port_ref)?,
        NodeKind::Path(node) => node.set_input_json(to_port, port_ref)?,
        NodeKind::Shape(node) => node.set_input_json(to_port, port_ref)?,
        NodeKind::Boolean(node) => node.set_input_json(to_port, port_ref)?,
        NodeKind::Merge(node) => node.set_input_json(to_port, port_ref)?,
        NodeKind::RasterMultiMerge(node) => node.set_input_json(to_port, port_ref)?,
        NodeKind::AlphaPremultiply(node) => node.set_input_json(to_port, port_ref)?,
        NodeKind::Blur(node) => node.set_input_json(to_port, port_ref)?,
        NodeKind::ChannelShuffle(node) => node.set_input_json(to_port, port_ref)?,
        NodeKind::ColorGrade(node) => node.set_input_json(to_port, port_ref)?,
        NodeKind::Curves(node) => node.set_input_json(to_port, port_ref)?,
        NodeKind::Exposure(node) => node.set_input_json(to_port, port_ref)?,
        NodeKind::HueSaturation(node) => node.set_input_json(to_port, port_ref)?,
        NodeKind::Levels(node) => node.set_input_json(to_port, port_ref)?,
        NodeKind::Memo(node) => node.set_input_json(to_port, port_ref)?,
        NodeKind::TimeRemap(node) => node.set_input_json(to_port, port_ref)?,
        NodeKind::Transform(node) => node.set_input_json(to_port, port_ref)?,
        NodeKind::Crop(node) => node.set_input_json(to_port, port_ref)?,
        NodeKind::Resize(node) => node.set_input_json(to_port, port_ref)?,
        NodeKind::Shadow(node) => node.set_input_json(to_port, port_ref)?,
        NodeKind::WgslShader(node) => node.set_input_json(to_port, port_ref)?,
        NodeKind::Switch(node) => node.set_input_json(to_port, port_ref)?,
        NodeKind::MediaOutput(node) => node.set_input_json(to_port, port_ref)?,
    }

    Ok(())
}
