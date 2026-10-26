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

pub(crate) use property::{StringParam, classify_string, integral_i64, parse_property};
pub use property::{parse_color, try_parse_color};

const DEFAULT_FPS: f32 = 30.0;

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
    comp.audio = root
        .get("audio")
        .map(|audio| parse_audio_timeline(audio, comp.timeline.fps))
        .transpose()?;
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
    let fps = match obj.get("fps") {
        None => DEFAULT_FPS,
        Some(value) => value
            .as_f64()
            .map(|fps| fps as f32)
            .filter(|fps| fps.is_finite() && *fps > 0.0)
            .context("`timeline.fps` must be a positive, finite number")?,
    };
    let duration_frames = obj
        .get("duration_frames")
        .context("missing `timeline.duration_frames`")?;
    let duration_frames = integral_u32(duration_frames)
        .filter(|frames| *frames >= 1)
        .context("`timeline.duration_frames` must be an integer of at least 1")?;
    Ok(TimelineSettings {
        fps,
        duration_frames,
    })
}

fn parse_render_settings(val: &Value) -> Result<RenderSettings> {
    let obj = val
        .as_object()
        .context("`render_settings` must be an object")?;
    Ok(RenderSettings {
        width: render_dimension(obj, "width")?,
        height: render_dimension(obj, "height")?,
        background_color: obj
            .get("background_color")
            .map(|value| {
                try_parse_color(value).map_err(|error| {
                    anyhow::anyhow!("`render_settings.background_color` expected a color: {error}")
                })
            })
            .transpose()?
            .unwrap_or([0, 0, 0, 255]),
    })
}

fn render_dimension(obj: &Map<String, Value>, key: &str) -> Result<u32> {
    obj.get(key)
        .and_then(integral_u32)
        .filter(|size| *size >= 1)
        .with_context(|| format!("`render_settings.{key}` must be an integer of at least 1"))
}

/// Reads a JSON number as a `u32`, accepting integral floats such as `1.0` like JSON Schema does.
fn integral_u32(value: &Value) -> Option<u32> {
    if let Some(integer) = value.as_u64() {
        return u32::try_from(integer).ok();
    }
    let float = value.as_f64()?;
    (float.fract() == 0.0 && (0.0..=f64::from(u32::MAX)).contains(&float)).then_some(float as u32)
}

fn parse_audio_timeline(val: &Value, fps: f32) -> Result<AudioTimeline> {
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
        .map(|clip| parse_audio_clip(clip, fps))
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
        volume: audio_volume(obj, "audio track")?,
    })
}

fn parse_audio_clip(val: &Value, fps: f32) -> Result<AudioClip> {
    let obj = val.as_object().context("audio clip must be an object")?;
    let start_ms = audio_time_ms(obj, "start_ms", "start_frame", fps)?;
    let duration_ms = audio_time_ms(obj, "duration_ms", "duration_frames", fps)?;
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
        volume: audio_volume(obj, "audio clip")?,
    })
}

/// Reads an optional `volume` gain, which defaults to 1.
fn audio_volume(obj: &Map<String, Value>, owner: &str) -> Result<f32> {
    let Some(value) = obj.get("volume") else {
        return Ok(1.0);
    };
    value
        .as_f64()
        .filter(|volume| *volume >= 0.0)
        .map(|volume| volume as f32)
        .filter(|volume| volume.is_finite())
        .with_context(|| format!("{owner} `volume` must be a finite number of at least 0"))
}

fn audio_time_ms(obj: &Map<String, Value>, ms_key: &str, frame_key: &str, fps: f32) -> Result<u64> {
    if let Some(ms) = obj.get(ms_key).and_then(Value::as_u64) {
        return Ok(ms);
    }
    if let Some(frames) = obj.get(frame_key).and_then(Value::as_u64) {
        // Frame compatibility for current editor payloads. Canonical output should emit *_ms.
        return Ok(((frames as f64 / f64::from(fps)) * 1_000.0).round() as u64);
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
    fn audio_volume_must_be_a_finite_non_negative_number() {
        let parse_audio = |track_volume: Value, clip_volume: Value| {
            parse_value(&serde_json::json!({
                "timeline": { "fps": 30, "duration_frames": 90 },
                "render_settings": { "width": 16, "height": 16 },
                "audio": {
                    "tracks": [{ "id": "track-1", "volume": track_volume }],
                    "clips": [{
                        "id": "clip-1",
                        "source_id": "voice",
                        "track_id": "track-1",
                        "start_ms": 0,
                        "duration_ms": 100,
                        "volume": clip_volume
                    }]
                },
                "nodes": [
                    { "id": 1, "type": "background", "params": { "width": 16, "height": 16 } },
                    { "id": 2, "type": "media_output" }
                ],
                "connections": [
                    { "from_node": 1, "from_port": "output", "to_node": 2, "to_port": "source" }
                ]
            }))
        };

        let audio = parse_audio(serde_json::json!(0), serde_json::json!(2.5))
            .unwrap()
            .audio
            .unwrap();
        assert_eq!(audio.tracks[0].volume, 0.0);
        assert_eq!(audio.clips[0].volume, 2.5);

        for invalid in [
            serde_json::json!(-0.5),
            serde_json::json!(-1e-50),
            serde_json::json!(1e300),
            serde_json::json!("loud"),
        ] {
            let track_error = parse_audio(invalid.clone(), serde_json::json!(1)).unwrap_err();
            assert!(
                track_error.to_string().contains("audio track `volume`"),
                "{invalid}: {track_error}"
            );
            let clip_error = parse_audio(serde_json::json!(1), invalid.clone()).unwrap_err();
            assert!(
                clip_error.to_string().contains("audio clip `volume`"),
                "{invalid}: {clip_error}"
            );
        }
    }

    #[test]
    fn audio_frame_fields_convert_at_the_timeline_fps() {
        let json = r#"{
            "timeline": { "fps": 60, "duration_frames": 120 },
            "render_settings": { "width": 1920, "height": 1080 },
            "audio": {
                "tracks": [{ "id": "track-1" }],
                "clips": [
                    {
                        "id": "clip-1",
                        "source_id": "voice",
                        "track_id": "track-1",
                        "start_frame": 60,
                        "duration_frames": 30
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
        let clip = &comp.audio.expect("audio timeline").clips[0];
        assert_eq!(clip.start_ms, 1_000);
        assert_eq!(clip.duration_ms, 500);
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
        for (frame, expected) in [(0.0, (0.0, 3.0)), (24.0, (48.0, 4.0)), (48.0, (96.0, 5.0))] {
            let context = crate::expr::ExpressionContext {
                frame,
                fps: 24.0,
                width: 800,
                height: 600,
                duration_frames: 48,
                path: Some("1.position".to_string()),
                graph: Some(&composition.graph),
                media: None,
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

    fn composition_json(timeline: &str, render_settings: &str) -> String {
        format!(
            r#"{{
                "timeline": {timeline},
                "render_settings": {render_settings},
                "nodes": [
                    {{ "id": 1, "type": "background", "params": {{ "paint": [255, 0, 0], "width": 100, "height": 100 }} }},
                    {{ "id": 2, "type": "media_output" }}
                ],
                "connections": [
                    {{ "from_node": 1, "from_port": "output", "to_node": 2, "to_port": "source" }}
                ]
            }}"#
        )
    }

    #[test]
    fn parse_rejects_invalid_fps() {
        for fps in ["0", "-30", "\"30\"", "null", "1e300"] {
            let json = composition_json(
                &format!(r#"{{ "fps": {fps}, "duration_frames": 10 }}"#),
                r#"{ "width": 100, "height": 100 }"#,
            );
            let err = parse(&json).unwrap_err().to_string();
            assert!(err.contains("timeline.fps"), "{fps}: {err}");
        }
    }

    #[test]
    fn parse_rejects_missing_or_invalid_duration_frames() {
        for timeline in [
            r#"{ "fps": 30 }"#,
            r#"{ "fps": 30, "duration_frames": 0 }"#,
            r#"{ "fps": 30, "duration_frames": -5 }"#,
            r#"{ "fps": 30, "duration_frames": 1.5 }"#,
            r#"{ "fps": 30, "duration_frames": 4294967296 }"#,
        ] {
            let json = composition_json(timeline, r#"{ "width": 100, "height": 100 }"#);
            let err = parse(&json).unwrap_err().to_string();
            assert!(
                err.contains("timeline.duration_frames"),
                "{timeline}: {err}"
            );
        }
    }

    #[test]
    fn background_color_defaults_to_black_but_rejects_malformed_values() {
        let timeline = r#"{ "fps": 30, "duration_frames": 10 }"#;
        let json = composition_json(timeline, r#"{ "width": 100, "height": 100 }"#);
        assert_eq!(
            parse(&json).unwrap().render_settings.background_color,
            [0, 0, 0, 255]
        );

        for color in [r#""red""#, "[256, 0, 0]", "null"] {
            let json = composition_json(
                timeline,
                &format!(r#"{{ "width": 100, "height": 100, "background_color": {color} }}"#),
            );
            let err = parse(&json).unwrap_err().to_string();
            assert!(
                err.contains("render_settings.background_color"),
                "{color}: {err}"
            );
        }
    }

    #[test]
    fn dimensions_and_frames_reject_wrapping_or_zero_and_accept_integral_floats() {
        let timeline = r#"{ "fps": 30, "duration_frames": 10 }"#;
        for (settings, field) in [
            (r#"{ "width": 4294967297, "height": 100 }"#, "width"),
            (r#"{ "width": 100, "height": 4294967296 }"#, "height"),
            (r#"{ "width": 0, "height": 100 }"#, "width"),
            (r#"{ "width": 100, "height": 1.5 }"#, "height"),
        ] {
            let err = parse(&composition_json(timeline, settings))
                .unwrap_err()
                .to_string();
            assert!(err.contains(&format!("render_settings.{field}")), "{err}");
        }

        let comp = parse(&composition_json(
            r#"{ "fps": 30, "duration_frames": 10.0 }"#,
            r#"{ "width": 100.0, "height": 50.0 }"#,
        ))
        .unwrap();
        assert_eq!(comp.timeline.duration_frames, 10);
        assert_eq!(
            (comp.render_settings.width, comp.render_settings.height),
            (100, 50)
        );
    }
}
