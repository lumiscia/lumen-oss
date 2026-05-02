//! Node construction from JSON: type dispatch, property application, and port wiring.

use std::collections::HashMap;

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::{
    graph::Graph,
    node::{
        NodeDef, NodeId, NodeKind, NodeProperty, PortRef,
        compositing::{
            boolean::Boolean, merge::Merge, raster_multimerge::RasterMultiMerge, switch::Switch,
        },
        media_output::MediaOutput,
        processing::{
            alpha_premultiply::AlphaPremultiply, blur::Blur, channel_shuffle::ChannelShuffle,
            color_grade::ColorGrade, crop::Crop, curves::Curves, exposure::Exposure,
            hue_saturation::HueSaturation, levels::Levels, matte_cleanup::MatteCleanup, memo::Memo,
            resize::Resize, shadow::Shadow, skia_shader::SkiaShader, time_remap::TimeRemap,
            transform::Transform,
        },
        source::{media_in::MediaIn, solid_color::SolidColor, text::Text},
        vector::{
            path::BezierPath, shape::Shape, shape_renderer::ShapeRenderer,
            vector_merge::VectorMerge, vector_multimerge::VectorMultiMerge,
            vector_stroke_style::VectorStrokeStyle, vector_text::VectorText,
            vector_transform::VectorTransform,
        },
    },
};

use super::property::parse_property;

/// Build a [`NodeKind`] from its JSON type name, id, and object data.
pub fn build_node(
    kind: &str,
    id: NodeId,
    obj: &serde_json::Map<String, Value>,
) -> Result<NodeKind> {
    let properties = obj.get("properties").and_then(|v| v.as_object());

    match kind {
        "boolean" => Ok(NodeKind::Boolean(build_typed::<Boolean>(id, properties)?)),
        "merge" => Ok(NodeKind::Merge(build_typed::<Merge>(id, properties)?)),
        "raster_multimerge" => Ok(NodeKind::RasterMultimerge(build_typed::<RasterMultiMerge>(
            id, properties,
        )?)),
        "switch" => {
            let mut node = Switch::default();
            node.id = id;
            if let Some(map_val) = obj.get("map") {
                node.map = parse_switch_map(map_val)?;
            }
            Ok(NodeKind::Switch(node))
        }
        "alpha_premultiply" => Ok(NodeKind::AlphaPremultiply(build_typed::<AlphaPremultiply>(
            id, properties,
        )?)),
        "blur" => Ok(NodeKind::Blur(build_typed::<Blur>(id, properties)?)),
        "channel_shuffle" => Ok(NodeKind::ChannelShuffle(build_typed::<ChannelShuffle>(
            id, properties,
        )?)),
        "color_grade" => Ok(NodeKind::ColorGrade(build_typed::<ColorGrade>(
            id, properties,
        )?)),
        "crop" => Ok(NodeKind::Crop(build_typed::<Crop>(id, properties)?)),
        "curves" => Ok(NodeKind::Curves(build_typed::<Curves>(id, properties)?)),
        "exposure" => Ok(NodeKind::Exposure(build_typed::<Exposure>(id, properties)?)),
        "hue_saturation" | "hsl" => Ok(NodeKind::HueSaturation(build_typed::<HueSaturation>(
            id, properties,
        )?)),
        "levels" => Ok(NodeKind::Levels(build_typed::<Levels>(id, properties)?)),
        "matte_cleanup" => Ok(NodeKind::MatteCleanup(build_typed::<MatteCleanup>(
            id, properties,
        )?)),
        "memo" => Ok(NodeKind::Memo(build_typed::<Memo>(id, properties)?)),
        "resize" => Ok(NodeKind::Resize(build_typed::<Resize>(id, properties)?)),
        "shadow" => Ok(NodeKind::Shadow(build_typed::<Shadow>(id, properties)?)),
        "skia_shader" => Ok(NodeKind::SkiaShader(build_skia_shader(id, properties)?)),
        "time_remap" => Ok(NodeKind::TimeRemap(build_typed::<TimeRemap>(
            id, properties,
        )?)),
        "transform" => Ok(NodeKind::Transform(build_typed::<Transform>(
            id, properties,
        )?)),
        "media_in" => Ok(NodeKind::MediaIn(build_typed::<MediaIn>(id, properties)?)),
        "solid_color" => Ok(NodeKind::SolidColor(build_typed::<SolidColor>(
            id, properties,
        )?)),
        "text" => Ok(NodeKind::Text(build_typed::<Text>(id, properties)?)),
        "bezier_path" | "path" => Ok(NodeKind::BezierPath(build_typed::<BezierPath>(
            id, properties,
        )?)),
        "shape" => Ok(NodeKind::Shape(build_typed::<Shape>(id, properties)?)),
        "shape_renderer" => Ok(NodeKind::ShapeRenderer(build_typed::<ShapeRenderer>(
            id, properties,
        )?)),
        "vector_stroke_style" => Ok(NodeKind::VectorStrokeStyle(
            build_typed::<VectorStrokeStyle>(id, properties)?,
        )),
        "vector_transform" => Ok(NodeKind::VectorTransform(build_typed::<VectorTransform>(
            id, properties,
        )?)),
        "vector_merge" => Ok(NodeKind::VectorMerge(build_typed::<VectorMerge>(
            id, properties,
        )?)),
        "vector_multimerge" => Ok(NodeKind::VectorMultimerge(build_typed::<VectorMultiMerge>(
            id, properties,
        )?)),
        "vector_text" => Ok(NodeKind::VectorText(build_typed::<VectorText>(
            id, properties,
        )?)),
        "media_output" => Ok(NodeKind::MediaOutput(build_typed::<MediaOutput>(
            id, properties,
        )?)),
        other => bail!("unknown node type `{other}`"),
    }
}

/// Wire a connection's target port on the destination node.
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

    let wired = wire_node_kind(node, to_port, port_ref);
    if !wired {
        bail!("unknown input port `{to_port}` on node {to_node}");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Trait bound for types that can be built from JSON via macro-generated methods.
///
/// Every node struct gets `__set_property` and `__wire_input` from `#[derive(Node)]`,
/// plus `NodeDef` for static property/port definitions. We only need `Default` + `NodeDef`
/// and the generated setters — no manual per-type boilerplate.
trait JsonBuildable: Default + NodeDef {
    fn set_id(&mut self, id: NodeId);
    fn set_property(&mut self, name: &str, value: NodeProperty) -> bool;
}

/// Blanket-style macro: every node has `pub id: NodeId` and `__set_property`.
macro_rules! impl_json_buildable {
    ($($ty:ty),* $(,)?) => {
        $(
            impl JsonBuildable for $ty {
                fn set_id(&mut self, id: NodeId) { self.id = id; }
                fn set_property(&mut self, name: &str, value: NodeProperty) -> bool {
                    self.__set_property(name, value)
                }
            }
        )*
    };
}

impl_json_buildable!(
    AlphaPremultiply,
    Boolean,
    BezierPath,
    Merge,
    RasterMultiMerge,
    Switch,
    Blur,
    ChannelShuffle,
    ColorGrade,
    Crop,
    Curves,
    Exposure,
    HueSaturation,
    Levels,
    MatteCleanup,
    Memo,
    Resize,
    Shadow,
    SkiaShader,
    TimeRemap,
    Transform,
    MediaIn,
    SolidColor,
    Text,
    Shape,
    ShapeRenderer,
    VectorMerge,
    VectorMultiMerge,
    VectorStrokeStyle,
    VectorTransform,
    VectorText,
    MediaOutput,
);

fn build_typed<T: JsonBuildable>(
    id: NodeId,
    properties: Option<&serde_json::Map<String, Value>>,
) -> Result<T> {
    let mut node = T::default();
    node.set_id(id);

    if let Some(props) = properties {
        let defs = T::property_defs();
        for (key, val) in props {
            let def = defs.iter().find(|d| d.name == key.as_str());
            let prop = parse_property(val, def, key)?;
            if !node.set_property(key, prop) {
                bail!("unknown property `{key}` on node {id}");
            }
        }
    }

    Ok(node)
}

fn build_skia_shader(
    id: NodeId,
    properties: Option<&serde_json::Map<String, Value>>,
) -> Result<SkiaShader> {
    let mut node = SkiaShader::default();
    node.id = id;

    let Some(props) = properties else {
        return Ok(node);
    };

    let defs = SkiaShader::property_defs();
    let mut legacy_uniforms: Vec<(&str, String)> = Vec::new();
    let mut has_uniforms_payload = false;

    for (key, val) in props {
        if matches!(
            key.as_str(),
            "uniform0" | "uniform1" | "uniform2" | "uniform3"
        ) {
            legacy_uniforms.push((key.as_str(), legacy_uniform_value(val, key)?));
            continue;
        }

        if key == "uniforms" {
            has_uniforms_payload = true;
        }

        let def = defs.iter().find(|d| d.name == key.as_str());
        let prop = parse_property(val, def, key)?;
        if !node.__set_property(key, prop) {
            bail!("unknown property `{key}` on node {id}");
        }
    }

    if !has_uniforms_payload && !legacy_uniforms.is_empty() {
        node.uniforms = NodeProperty::String(
            legacy_uniforms
                .into_iter()
                .map(|(name, value)| format!("{name} = {value}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }

    Ok(node)
}

fn legacy_uniform_value(value: &Value, name: &str) -> Result<String> {
    if let Some(number) = value.as_f64() {
        return Ok(number.to_string());
    }
    if let Some(number) = value.as_i64() {
        return Ok(number.to_string());
    }
    if let Some(expression) = value.as_str().filter(|value| value.starts_with('=')) {
        return Ok(expression.to_string());
    }

    bail!("legacy skia shader `{name}` expected number or expression")
}

/// Dispatch `__wire_input` through `NodeKind` to the inner node struct.
fn wire_node_kind(node: &mut NodeKind, port: &str, port_ref: PortRef) -> bool {
    match node {
        NodeKind::Boolean(n) => n.__wire_input(port, port_ref),
        NodeKind::Merge(n) => n.__wire_input(port, port_ref),
        NodeKind::RasterMultimerge(n) => n.__wire_input(port, port_ref),
        NodeKind::Switch(n) => n.__wire_input(port, port_ref),
        NodeKind::AlphaPremultiply(n) => n.__wire_input(port, port_ref),
        NodeKind::Blur(n) => n.__wire_input(port, port_ref),
        NodeKind::ChannelShuffle(n) => n.__wire_input(port, port_ref),
        NodeKind::ColorGrade(n) => n.__wire_input(port, port_ref),
        NodeKind::Crop(n) => n.__wire_input(port, port_ref),
        NodeKind::Curves(n) => n.__wire_input(port, port_ref),
        NodeKind::Exposure(n) => n.__wire_input(port, port_ref),
        NodeKind::HueSaturation(n) => n.__wire_input(port, port_ref),
        NodeKind::Levels(n) => n.__wire_input(port, port_ref),
        NodeKind::MatteCleanup(n) => n.__wire_input(port, port_ref),
        NodeKind::Memo(n) => n.__wire_input(port, port_ref),
        NodeKind::Resize(n) => n.__wire_input(port, port_ref),
        NodeKind::Shadow(n) => n.__wire_input(port, port_ref),
        NodeKind::SkiaShader(n) => n.__wire_input(port, port_ref),
        NodeKind::TimeRemap(n) => n.__wire_input(port, port_ref),
        NodeKind::Transform(n) => n.__wire_input(port, port_ref),
        NodeKind::MediaIn(n) => n.__wire_input(port, port_ref),
        NodeKind::SolidColor(n) => n.__wire_input(port, port_ref),
        NodeKind::Text(n) => n.__wire_input(port, port_ref),
        NodeKind::BezierPath(n) => n.__wire_input(port, port_ref),
        NodeKind::Shape(n) => n.__wire_input(port, port_ref),
        NodeKind::ShapeRenderer(n) => n.__wire_input(port, port_ref),
        NodeKind::VectorMerge(n) => n.__wire_input(port, port_ref),
        NodeKind::VectorMultimerge(n) => n.__wire_input(port, port_ref),
        NodeKind::VectorStrokeStyle(n) => n.__wire_input(port, port_ref),
        NodeKind::VectorTransform(n) => n.__wire_input(port, port_ref),
        NodeKind::VectorText(n) => n.__wire_input(port, port_ref),
        NodeKind::MediaOutput(n) => n.__wire_input(port, port_ref),
    }
}

fn parse_switch_map(val: &Value) -> Result<HashMap<u16, std::ops::Range<u32>>> {
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
