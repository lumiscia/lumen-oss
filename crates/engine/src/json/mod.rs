//! JSON deserialization for Lumen compositions.
//!
//! Parses a JSON object into a [`Composition`] containing a validated [`Graph`]
//! of renderer-agnostic nodes and connections.
//!
//! # Modules
//!
//! - [`property`] — JSON value → [`PropertyValue`](crate::node::PropertyValue) conversion
//! - [`node`] — per-node-type construction, property application, and port wiring

mod migrations;
mod node;
mod property;

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value};

use crate::{
    audio::{AudioClip, AudioTimeline, AudioTrack},
    composition::{Composition, CompositionMetadata, RenderSettings, TimelineSettings},
    graph::{Connection, Graph},
    node::NodeId,
};

pub use property::parse_color;
pub(crate) use property::parse_property;

/// Parse a JSON string into a [`Composition`].
pub fn parse(json: &str) -> Result<Composition> {
    let root: Value = serde_json::from_str(json).context("invalid JSON")?;
    parse_value(&root)
}

/// Parse a [`serde_json::Value`] into a [`Composition`].
pub fn parse_value(root: &Value) -> Result<Composition> {
    let migrated = migrations::migrate_to_current(root)?;
    parse_current_value(&migrated)
}

fn parse_current_value(root: &Value) -> Result<Composition> {
    let obj = root.as_object().context("root must be an object")?;
    let timeline = parse_timeline(obj.get("timeline").context("missing `timeline`")?)?;
    let render_settings = parse_render_settings(
        obj.get("render_settings")
            .context("missing `render_settings`")?,
    )?;

    let nodes_arr = obj
        .get("nodes")
        .and_then(|v| v.as_array())
        .context("`nodes` must be an array")?;
    let connections_arr = obj
        .get("connections")
        .and_then(|v| v.as_array())
        .context("`connections` must be an array")?;

    let mut graph = Graph::new();

    // First pass: create all nodes
    for node_val in nodes_arr {
        let node_obj = node_val.as_object().context("node must be an object")?;
        let id = parse_node_id(node_obj.get("id").context("node missing `id`")?)?;
        let kind_str = node_obj
            .get("type")
            .and_then(|v| v.as_str())
            .context("node missing `type` string")?;

        let node_kind = node::build_node(kind_str, id, node_obj)?;
        graph.nodes.insert(id, node_kind);
    }

    // Second pass: create connections and wire up PortRefs
    for conn_val in connections_arr {
        let conn_obj = conn_val
            .as_object()
            .context("connection must be an object")?;
        let from_node = parse_node_id(conn_obj.get("from_node").context("missing `from_node`")?)?;
        let from_port = conn_obj
            .get("from_port")
            .and_then(|v| v.as_str())
            .unwrap_or("output")
            .to_string();
        let to_node = parse_node_id(conn_obj.get("to_node").context("missing `to_node`")?)?;
        let to_port = conn_obj
            .get("to_port")
            .and_then(|v| v.as_str())
            .context("missing `to_port`")?
            .to_string();

        graph
            .connect(Connection {
                from_node,
                from_port: from_port.clone(),
                to_node,
                to_port: to_port.clone(),
            })
            .with_context(|| {
                format!("connecting {from_node}:{from_port} -> {to_node}:{to_port}")
            })?;
    }

    let metadata = obj.get("metadata").map(|v| CompositionMetadata {
        name: v.get("name").and_then(|n| n.as_str()).map(String::from),
    });

    let mut comp = Composition::new(graph, timeline, render_settings);
    comp.metadata = metadata;
    comp.audio = root.get("audio").map(parse_audio_timeline).transpose()?;
    if let Err(errors) = comp.validate_structure() {
        let details = errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ");
        bail!("composition structure validation failed: {details}");
    }
    Ok(comp)
}

// ---------------------------------------------------------------------------
// Section parsers
// ---------------------------------------------------------------------------

fn parse_timeline(val: &Value) -> Result<TimelineSettings> {
    let obj = val.as_object().context("`timeline` must be an object")?;
    Ok(TimelineSettings {
        fps: obj.get("fps").and_then(|v| v.as_f64()).unwrap_or(30.0) as f32,
        duration_frames: obj
            .get("duration_frames")
            .and_then(|v| v.as_u64())
            .unwrap_or(1) as u32,
    })
}

fn parse_render_settings(val: &Value) -> Result<RenderSettings> {
    let obj = val
        .as_object()
        .context("`render_settings` must be an object")?;
    Ok(RenderSettings {
        width: obj
            .get("width")
            .and_then(|v| v.as_u64())
            .context("render_settings.width")? as u32,
        height: obj
            .get("height")
            .and_then(|v| v.as_u64())
            .context("render_settings.height")? as u32,
        background_color: parse_color(obj.get("background_color").unwrap_or(&Value::Null))
            .unwrap_or([0, 0, 0, 255]),
    })
}

fn parse_audio_timeline(val: &Value) -> Result<AudioTimeline> {
    let obj = val.as_object().context("`audio` must be an object")?;
    let tracks = obj
        .get("tracks")
        .and_then(Value::as_array)
        .context("audio.tracks must be an array")?
        .iter()
        .map(parse_audio_track)
        .collect::<Result<Vec<_>>>()?;
    let clips = obj
        .get("clips")
        .and_then(Value::as_array)
        .context("audio.clips must be an array")?
        .iter()
        .map(parse_audio_clip)
        .collect::<Result<Vec<_>>>()?;

    Ok(AudioTimeline { tracks, clips })
}

fn parse_audio_track(val: &Value) -> Result<AudioTrack> {
    let obj = val.as_object().context("audio track must be an object")?;
    Ok(AudioTrack {
        id: required_string(obj, "id")?,
        name: obj
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("Track")
            .to_string(),
        muted: obj.get("muted").and_then(Value::as_bool).unwrap_or(false),
        solo: obj.get("solo").and_then(Value::as_bool).unwrap_or(false),
        volume: obj.get("volume").and_then(Value::as_f64).unwrap_or(1.0) as f32,
    })
}

fn parse_audio_clip(val: &Value) -> Result<AudioClip> {
    let obj = val.as_object().context("audio clip must be an object")?;
    let start_ms = audio_time_ms(obj, "start_ms", "start_frame")?;
    let duration_ms = audio_time_ms(obj, "duration_ms", "duration_frames")?;
    if duration_ms == 0 {
        anyhow::bail!("audio clip duration_ms must be greater than zero");
    }

    Ok(AudioClip {
        id: required_string(obj, "id")?,
        source_id: required_string(obj, "source_id")?,
        track_id: required_string(obj, "track_id")?,
        name: obj
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("Audio clip")
            .to_string(),
        start_ms,
        duration_ms,
        source_start_ms: obj
            .get("source_start_ms")
            .and_then(Value::as_u64)
            .or_else(|| {
                obj.get("source_start_seconds")
                    .and_then(Value::as_f64)
                    .map(|seconds| (seconds.max(0.0) * 1_000.0).round() as u64)
            })
            .unwrap_or(0),
        volume: obj.get("volume").and_then(Value::as_f64).unwrap_or(1.0) as f32,
    })
}

fn audio_time_ms(obj: &Map<String, Value>, ms_key: &str, frame_key: &str) -> Result<u64> {
    if let Some(ms) = obj.get(ms_key).and_then(Value::as_u64) {
        return Ok(ms);
    }
    if let Some(frames) = obj.get(frame_key).and_then(Value::as_u64) {
        // Frame compatibility for current editor payloads. Canonical output should emit *_ms.
        return Ok(((frames as f64 / 30.0) * 1_000.0).round() as u64);
    }
    anyhow::bail!("audio clip missing `{ms_key}`")
}

fn required_string(obj: &Map<String, Value>, key: &str) -> Result<String> {
    obj.get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .with_context(|| format!("missing `{key}` string"))
}

fn parse_node_id(val: &Value) -> Result<NodeId> {
    let n = val.as_u64().context("node id must be a u64")?;
    Ok(NodeId::new(n))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_composition() {
        let json = r#"{
            "lumenSchemaVersion": "0.1.0",
            "timeline": { "fps": 30, "duration_frames": 90 },
            "render_settings": { "width": 1920, "height": 1080 },
            "nodes": [
                { "id": 1, "type": "background", "params": { "paint": [255, 0, 0, 255], "width": 1920, "height": 1080 } },
                { "id": 2, "type": "media_output" }
            ],
            "connections": [
                { "from_node": 1, "from_port": "output", "to_node": 2, "to_port": "source" }
            ]
        }"#;

        let comp = parse(json).expect("should parse");
        assert_eq!(comp.timeline.fps, 30.0);
        assert_eq!(comp.timeline.duration_frames, 90);
        assert_eq!(comp.render_settings.width, 1920);
        assert_eq!(comp.render_settings.height, 1080);
        assert_eq!(comp.graph.nodes.len(), 2);
        assert_eq!(comp.graph.connections.len(), 1);
    }

    #[test]
    fn parse_composition_with_audio_timeline() {
        let json = r#"{
            "timeline": { "fps": 30, "duration_frames": 90 },
            "render_settings": { "width": 1920, "height": 1080 },
            "audio": {
                "tracks": [
                    { "id": "track-1", "name": "Voice", "muted": false, "solo": true, "volume": 0.5 }
                ],
                "clips": [
                    {
                        "id": "clip-1",
                        "source_id": "voice",
                        "track_id": "track-1",
                        "name": "voice.wav",
                        "start_ms": 100,
                        "duration_ms": 900,
                        "source_start_ms": 50,
                        "volume": 0.75
                    }
                ]
            },
            "nodes": [
                { "id": 1, "type": "background", "params": { "paint": [255, 0, 0, 255], "width": 1920, "height": 1080 } },
                { "id": 2, "type": "media_output" }
            ],
            "connections": [
                { "from_node": 1, "from_port": "output", "to_node": 2, "to_port": "source" }
            ]
        }"#;

        let comp = parse(json).expect("should parse");
        let audio = comp.audio.expect("audio timeline");
        assert_eq!(audio.tracks[0].id, "track-1");
        assert!(audio.tracks[0].solo);
        assert_eq!(audio.clips[0].source_id, "voice");
        assert_eq!(audio.clips[0].start_ms, 100);
        assert_eq!(audio.clips[0].source_start_ms, 50);
    }

    #[test]
    fn parse_with_expressions() {
        let json = r##"{
            "timeline": { "fps": 24, "duration_frames": 48 },
            "render_settings": { "width": 800, "height": 600 },
            "nodes": [
                { "id": 1, "type": "background", "params": { "paint": "#FF0000", "width": 800, "height": 600 } },
                { "id": 2, "type": "exposure", "params": {
                    "exposure": "=frame * 0.1",
                    "contrast": 1.0,
                    "offset": 0.0
                }},
                { "id": 3, "type": "media_output" }
            ],
            "connections": [
                { "from_node": 1, "from_port": "output", "to_node": 2, "to_port": "source" },
                { "from_node": 2, "from_port": "output", "to_node": 3, "to_port": "source" }
            ]
        }"##;

        let comp = parse(json).expect("should parse");
        assert_eq!(comp.graph.nodes.len(), 3);
    }

    #[test]
    fn parses_and_evaluates_vec2_expression() {
        let json = r##"{
            "timeline": { "fps": 24, "duration_frames": 48 },
            "render_settings": { "width": 800, "height": 600 },
            "nodes": [
                { "id": 1, "type": "path", "params": {
                    "data": "M 0 0 L 100 100",
                    "position": "=vec2(frame * 2, time + 3)"
                }},
                { "id": 2, "type": "media_output" }
            ],
            "connections": [
                { "from_node": 1, "from_port": "output", "to_node": 2, "to_port": "source" }
            ]
        }"##;

        let composition = parse(json).expect("should parse vec2 expression");
        let crate::node::NodeKind::Path(path) = composition
            .graph
            .nodes
            .get(&NodeId::new(1))
            .expect("path node")
        else {
            panic!("expected path node");
        };
        for (frame, expected) in [(0, (0.0, 3.0)), (24, (48.0, 4.0)), (48, (96.0, 5.0))] {
            let context = crate::expr::ExpressionContext {
                frame,
                fps: 24.0,
                width: 800,
                height: 600,
                duration_frames: 48,
                path: Some("1.position".to_string()),
                graph: Some(&composition.graph),
            };

            assert_eq!(
                path.params
                    .position
                    .eval(path.id, "position", &context)
                    .unwrap(),
                expected
            );
        }
    }

    #[test]
    fn parse_switch_node() {
        let json = r#"{
            "timeline": { "fps": 30, "duration_frames": 60 },
            "render_settings": { "width": 100, "height": 100 },
            "nodes": [
                { "id": 1, "type": "background", "params": { "paint": [255, 0, 0], "width": 100, "height": 100 } },
                { "id": 2, "type": "background", "params": { "paint": [0, 255, 0], "width": 100, "height": 100 } },
                { "id": 3, "type": "switch", "params": { "selected_layer": "=if(frame < 30, 0, 1)" } },
                { "id": 4, "type": "media_output" }
            ],
            "connections": [
                { "from_node": 1, "from_port": "output", "to_node": 3, "to_port": "layers" },
                { "from_node": 2, "from_port": "output", "to_node": 3, "to_port": "layers" },
                { "from_node": 3, "from_port": "output", "to_node": 4, "to_port": "source" }
            ]
        }"#;

        let comp = parse(json).expect("should parse");
        assert_eq!(comp.graph.nodes.len(), 4);
    }

    #[test]
    fn parse_opacity_node() {
        let json = r#"{
            "timeline": { "fps": 30, "duration_frames": 60 },
            "render_settings": { "width": 100, "height": 100 },
            "nodes": [
                { "id": 1, "type": "background", "params": { "paint": [255, 0, 0], "width": 100, "height": 100 } },
                { "id": 2, "type": "opacity", "params": { "opacity": "=frame / 60" } },
                { "id": 3, "type": "media_output" }
            ],
            "connections": [
                { "from_node": 1, "from_port": "output", "to_node": 2, "to_port": "source" },
                { "from_node": 2, "from_port": "output", "to_node": 3, "to_port": "source" }
            ]
        }"#;

        let comp = parse(json).expect("should parse");
        let opacity = comp.graph.nodes.get(&NodeId::new(2)).expect("opacity");
        assert!(matches!(opacity, crate::node::NodeKind::Opacity(_)));
        assert_eq!(
            opacity.input_ports(),
            vec![crate::node::PortRef::new(
                NodeId::new(1),
                "output".to_string()
            )]
        );
    }

    #[test]
    fn parse_enum_property_names() {
        let json = r#"{
            "timeline": { "fps": 30, "duration_frames": 60 },
            "render_settings": { "width": 100, "height": 100 },
            "nodes": [
                { "id": 1, "type": "background", "params": { "paint": [255, 0, 0], "width": 100, "height": 100 } },
                { "id": 2, "type": "background", "params": { "paint": [0, 255, 0], "width": 100, "height": 100 } },
                { "id": 3, "type": "merge", "params": { "blend_mode": "multiply", "opacity": 0.5 } },
                { "id": 4, "type": "media_output" }
            ],
            "connections": [
                { "from_node": 1, "from_port": "output", "to_node": 3, "to_port": "base" },
                { "from_node": 2, "from_port": "output", "to_node": 3, "to_port": "overlay" },
                { "from_node": 3, "from_port": "output", "to_node": 4, "to_port": "source" }
            ]
        }"#;

        let comp = parse(json).expect("should parse");
        let merge = comp.graph.nodes.get(&NodeId::new(3)).expect("merge");
        assert!(matches!(
            merge,
            crate::node::NodeKind::Merge(node)
                if matches!(node.params.blend_mode, crate::node::compositing::BlendModeDelegate::Multiply)
        ));
    }

    #[test]
    fn parse_rejects_invalid_enum_property_name() {
        let json = r#"{
            "timeline": { "fps": 30, "duration_frames": 60 },
            "render_settings": { "width": 100, "height": 100 },
            "nodes": [
                { "id": 1, "type": "merge", "params": { "blend_mode": "not_real" } }
            ],
            "connections": []
        }"#;

        let err = parse(json).unwrap_err().to_string();
        assert!(err.contains("unknown enum value `not_real`"));
    }

    #[test]
    fn unknown_node_type_errors() {
        let json = r#"{
            "timeline": { "fps": 30, "duration_frames": 1 },
            "render_settings": { "width": 100, "height": 100 },
            "nodes": [{ "id": 1, "type": "nonexistent" }],
            "connections": []
        }"#;

        assert!(parse(json).is_err());
    }

    #[test]
    fn parse_rejects_unknown_schema_version() {
        let json = r#"{
            "lumenSchemaVersion": "99.0.0",
            "timeline": { "fps": 30, "duration_frames": 1 },
            "render_settings": { "width": 100, "height": 100 },
            "nodes": [],
            "connections": []
        }"#;

        let err = parse(json).unwrap_err().to_string();
        assert!(err.contains("unsupported Lumen schema version `99.0.0`"));
    }

    #[test]
    fn parse_rejects_cyclic_composition() {
        let json = r#"{
            "timeline": { "fps": 30, "duration_frames": 1 },
            "render_settings": { "width": 100, "height": 100 },
            "nodes": [
                { "id": 1, "type": "exposure" },
                { "id": 2, "type": "exposure" },
                { "id": 3, "type": "media_output" }
            ],
            "connections": [
                { "from_node": 1, "from_port": "output", "to_node": 2, "to_port": "source" },
                { "from_node": 2, "from_port": "output", "to_node": 1, "to_port": "source" },
                { "from_node": 1, "from_port": "output", "to_node": 3, "to_port": "source" }
            ]
        }"#;

        let error = parse(json).expect_err("cyclic graph must be rejected");
        assert!(error.to_string().contains("graph contains a cycle"));
    }
}
