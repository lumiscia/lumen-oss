//! Node construction from JSON for the renderer-agnostic node schema.

use anyhow::{Result, bail};
use serde_json::Value;

use crate::node::{
    JsonNode, NodeId, NodeKind,
    compositing::{
        boolean::Boolean, merge::Merge, raster_multimerge::RasterMultiMerge, switch::Switch,
    },
    media_output::MediaOutput,
    processing::{
        alpha_premultiply::AlphaPremultiply, blur::Blur, channel_shuffle::ChannelShuffle,
        color_grade::ColorGrade, crop::Crop, curves::Curves, exposure::Exposure,
        hue_saturation::HueSaturation, levels::Levels, memo::Memo, resize::Resize, shadow::Shadow,
        time_remap::TimeRemap, transform::Transform, wgsl_shader::WgslShader,
    },
    source::{background::Background, media_in::MediaIn, text::Text},
    vector::{path::Path, shape::Shape},
};

pub fn build_node(
    kind: &str,
    id: NodeId,
    obj: &serde_json::Map<String, Value>,
) -> Result<NodeKind> {
    let params = obj.get("params").and_then(Value::as_object);

    match kind {
        "media_in" => Ok(NodeKind::MediaIn(MediaIn::from_json(id, params)?)),
        "background" => Ok(NodeKind::Background(Background::from_json(id, params)?)),
        "text" => Ok(NodeKind::Text(Text::from_json(id, params)?)),
        "path" => Ok(NodeKind::Path(Path::from_json(id, params)?)),
        "shape" => Ok(NodeKind::Shape(Shape::from_json(id, params)?)),
        "boolean" => Ok(NodeKind::Boolean(Boolean::from_json(id, params)?)),
        "merge" => Ok(NodeKind::Merge(Merge::from_json(id, params)?)),
        "raster_multimerge" => Ok(NodeKind::RasterMultiMerge(RasterMultiMerge::from_json(
            id, params,
        )?)),
        "switch" => Ok(NodeKind::Switch(Switch::from_json(id, params)?)),
        "memo" => Ok(NodeKind::Memo(Memo::from_json(id, params)?)),
        "alpha_premultiply" => Ok(NodeKind::AlphaPremultiply(AlphaPremultiply::from_json(
            id, params,
        )?)),
        "blur" => Ok(NodeKind::Blur(Blur::from_json(id, params)?)),
        "channel_shuffle" => Ok(NodeKind::ChannelShuffle(ChannelShuffle::from_json(
            id, params,
        )?)),
        "color_grade" => Ok(NodeKind::ColorGrade(ColorGrade::from_json(id, params)?)),
        "curves" => Ok(NodeKind::Curves(Curves::from_json(id, params)?)),
        "exposure" => Ok(NodeKind::Exposure(Exposure::from_json(id, params)?)),
        "hue_saturation" => Ok(NodeKind::HueSaturation(HueSaturation::from_json(
            id, params,
        )?)),
        "levels" => Ok(NodeKind::Levels(Levels::from_json(id, params)?)),
        "time_remap" => Ok(NodeKind::TimeRemap(TimeRemap::from_json(id, params)?)),
        "transform" => Ok(NodeKind::Transform(Transform::from_json(id, params)?)),
        "crop" => Ok(NodeKind::Crop(Crop::from_json(id, params)?)),
        "resize" => Ok(NodeKind::Resize(Resize::from_json(id, params)?)),
        "shadow" => Ok(NodeKind::Shadow(Shadow::from_json(id, params)?)),
        "wgsl_shader" => Ok(NodeKind::WgslShader(WgslShader::from_json(id, params)?)),
        "media_output" => Ok(NodeKind::MediaOutput(MediaOutput::from_json(id, params)?)),
        other => bail!("unknown or unsupported node type `{other}`"),
    }
}
