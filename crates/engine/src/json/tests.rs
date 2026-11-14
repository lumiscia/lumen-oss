use super::*;
use crate::{expr::ExpressionValue, node::Node};

#[test]
fn parses_every_bundled_demo() {
    let demos = concat!(env!("CARGO_MANIFEST_DIR"), "/../cli/demo");
    for entry in std::fs::read_dir(demos).unwrap() {
        let path = entry.unwrap().path();
        let source = std::fs::read_to_string(&path).unwrap();
        parse(&source).unwrap_or_else(|error| panic!("{}: {error:#}", path.display()));
    }
}

#[test]
fn parse_composition_with_audio_timeline() {
    let json = r#"{
            "timeline": { "fps": 30, "duration_frames": 90 },
            "render_settings": { "width": 1920, "height": 1080 },
            "audio": {
                "tracks": [
                    { "id": "track-1", "name": "Voice", "muted": false, "solo": true, "gain": 0.5 }
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
                        "gain": 0.75
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
    let audio = comp.audio().expect("audio timeline");
    assert_eq!(audio.tracks[0].id, "track-1");
    assert!(audio.tracks[0].solo);
    assert_eq!(audio.clips[0].source_id, "voice");
    assert_eq!(audio.clips[0].start_ms, 100);
    assert_eq!(audio.clips[0].source_start_ms, 50);
}

fn audio_composition(audio: Value) -> Result<Composition, ParseError> {
    parse_value(&serde_json::json!({
        "timeline": { "fps": 30, "duration_frames": 90 },
        "render_settings": { "width": 16, "height": 16 },
        "audio": audio,
        "nodes": [
            { "id": 1, "type": "background", "params": { "width": 16, "height": 16 } },
            { "id": 2, "type": "media_output" }
        ],
        "connections": [
            { "from_node": 1, "from_port": "output", "to_node": 2, "to_port": "source" }
        ]
    }))
}

#[test]
fn audio_ids_and_track_references_are_checked() {
    let clip = |id: &str, track_id: &str, duration_ms: u64| {
        serde_json::json!({
            "id": id,
            "source_id": "voice",
            "track_id": track_id,
            "start_ms": 0,
            "duration_ms": duration_ms
        })
    };
    let error = |tracks: Value, clips: Value| {
        let audio = serde_json::json!({ "tracks": tracks, "clips": clips });
        format!("{:#}", audio_composition(audio).unwrap_err())
    };
    let main = serde_json::json!([{ "id": "main" }, { "id": "fx" }]);

    let unknown = error(main.clone(), serde_json::json!([clip("a", "typo", 100)]));
    assert!(
        unknown.contains("clip `a` names unknown track `typo`; declared tracks: `main`, `fx`"),
        "{unknown}"
    );
    let undeclared = error(
        serde_json::json!([]),
        serde_json::json!([clip("a", "x", 1)]),
    );
    assert!(undeclared.contains("declared tracks: none"), "{undeclared}");
    let tracks = error(
        serde_json::json!([{ "id": "main" }, { "id": "main" }]),
        serde_json::json!([]),
    );
    assert!(tracks.contains("duplicate track id `main`"), "{tracks}");
    let clips = error(
        main.clone(),
        serde_json::json!([clip("a", "main", 100), clip("a", "fx", 100)]),
    );
    assert!(clips.contains("duplicate clip id `a`"), "{clips}");
    let duration = error(main, serde_json::json!([clip("a", "main", 0)]));
    assert!(
        duration.contains("clip `a` duration must be greater than zero"),
        "{duration}"
    );
}

#[test]
fn builders_get_the_audio_checks_too() {
    let composition = audio_composition(serde_json::json!({
        "tracks": [{ "id": "main" }],
        "clips": []
    }))
    .unwrap();
    let mut builder = composition.into_builder();
    let audio = builder.audio.as_mut().unwrap();
    audio.tracks[0].gain = f32::NAN.into();
    let errors = builder.build().unwrap_err();
    assert!(
        errors[0].to_string().contains("track `main` gain"),
        "{}",
        errors[0]
    );
}

fn graph_error(nodes: Value, connections: Value) -> String {
    let error = parse_value(&serde_json::json!({
        "timeline": { "fps": 30, "duration_frames": 90 },
        "render_settings": { "width": 16, "height": 16 },
        "nodes": nodes,
        "connections": connections
    }))
    .unwrap_err();
    format!("{error:#}")
}

#[test]
fn unknown_variables_name_the_node_param_and_variable_at_load() {
    for (params, param) in [
        (serde_json::json!({ "opacity": "=foo + 1" }), "opacity"),
        (
            serde_json::json!({ "content": [{ "text": "a", "font_size": "=foo + 1" }] }),
            "font_size",
        ),
    ] {
        let error = graph_error(
            serde_json::json!([{ "id": 3, "type": "text", "params": params }]),
            serde_json::json!([]),
        );
        for expected in ["node 3", param, "unknown variable `foo`"] {
            assert!(error.contains(expected), "{expected}: {error}");
        }
    }
}

#[test]
fn duplicate_node_ids_are_rejected_within_a_graph() {
    let error = graph_error(
        serde_json::json!([
            { "id": 1, "type": "background" },
            { "id": 1, "type": "media_output" }
        ]),
        serde_json::json!([]),
    );
    assert!(error.contains("node 1: duplicate node id"), "{error}");
}

#[test]
fn malformed_params_and_unknown_keys_name_what_is_wrong() {
    for (params, got) in [
        (serde_json::json!(5), "a number"),
        (serde_json::json!("x"), "a string"),
        (serde_json::json!(true), "a boolean"),
        (serde_json::json!([]), "an array"),
        (Value::Null, "null"),
    ] {
        let error = graph_error(
            serde_json::json!([{ "id": 7, "type": "background", "params": params }]),
            serde_json::json!([]),
        );
        assert!(
            error.contains(&format!(
                "node 7 (`background`): `params` must be an object, got {got}"
            )),
            "{error}"
        );
    }

    let error = graph_error(
        serde_json::json!([{ "id": 7, "type": "background", "properties": {} }]),
        serde_json::json!([]),
    );
    assert!(
        error.contains("nodes[0].properties: unknown field `properties`, expected one of `id`, `type`, `params`"),
        "{error}"
    );

    let error = graph_error(
        serde_json::json!([
            { "id": 1, "type": "background" },
            { "id": 2, "type": "media_output" }
        ]),
        serde_json::json!([
            { "from_node": 1, "to_node": 2, "to_port": "source", "toPort": "source" }
        ]),
    );
    assert!(
        error.contains("connections[0].toPort: unknown field `toPort`"),
        "{error}"
    );
}

#[test]
fn a_connection_names_the_one_output_or_none() {
    let composition = |from_port: Value| {
        serde_json::json!({
            "timeline": { "fps": 30, "duration_frames": 90 },
            "render_settings": { "width": 16, "height": 16 },
            "nodes": [
                { "id": 1, "type": "background" },
                { "id": 2, "type": "media_output" }
            ],
            "connections": [
                { "from_node": 1, "from_port": from_port, "to_node": 2, "to_port": "source" }
            ]
        })
    };
    assert!(parse_value(&composition(serde_json::json!("output"))).is_ok());
    let mut omitted = composition(Value::Null);
    omitted["connections"][0]
        .as_object_mut()
        .unwrap()
        .remove("from_port");
    assert!(parse_value(&omitted).is_ok());
    let error = parse_value(&composition(serde_json::json!("mask"))).unwrap_err();
    assert!(
        format!("{error:#}").contains("connection 0: `from_port` must be `output`, got \"mask\""),
        "{error:#}"
    );
}

#[test]
fn positional_arrays_are_rejected_where_objects_belong() {
    let valid = serde_json::json!({
        "metadata": { "name": "card" },
        "timeline": { "fps": 30, "duration_frames": 90 },
        "render_settings": { "width": 16, "height": 16 },
        "audio": {
            "tracks": [{ "id": "track-1" }],
            "clips": [{
                "id": "clip-1",
                "source_id": "voice",
                "track_id": "track-1",
                "start_ms": 0,
                "duration_ms": 100
            }]
        },
        "compositions": {
            "card": {
                "props": { "index": { "type": "number" } },
                "nodes": [{ "id": 1, "type": "background" }, { "id": 2, "type": "media_output" }],
                "connections": [{ "from_node": 1, "to_node": 2, "to_port": "source" }]
            }
        },
        "nodes": [
            { "id": 1, "type": "background", "params": { "width": 16, "height": 16 } },
            { "id": 2, "type": "media_output" }
        ],
        "connections": [{ "from_node": 1, "to_node": 2, "to_port": "source" }]
    });
    parse_value(&valid).unwrap();

    for (pointer, positional, path) in [
        ("/timeline", serde_json::json!([30, 10]), "timeline"),
        (
            "/render_settings",
            serde_json::json!([16, 16]),
            "render_settings",
        ),
        ("/nodes/0", serde_json::json!([1, "background"]), "nodes[0]"),
        (
            "/connections/0",
            serde_json::json!([1, null, 2, "source"]),
            "connections[0]",
        ),
        ("/metadata", serde_json::json!(["card"]), "metadata"),
        ("/audio", serde_json::json!([[], []]), "audio"),
        (
            "/audio/tracks/0",
            serde_json::json!(["track-1"]),
            "audio.tracks[0]",
        ),
        (
            "/audio/clips/0",
            serde_json::json!(["clip-1"]),
            "audio.clips[0]",
        ),
        (
            "/compositions/card",
            serde_json::json!([]),
            "compositions.card",
        ),
        (
            "/compositions/card/nodes/0",
            serde_json::json!([1, "background"]),
            "compositions.card.nodes[0]",
        ),
        (
            "/compositions/card/props/index",
            serde_json::json!(["number"]),
            "compositions.card.props.index",
        ),
    ] {
        let mut document = valid.clone();
        *document.pointer_mut(pointer).unwrap() = positional;
        let error = format!("{:#}", parse_value(&document).expect_err(path));
        assert!(
            error.contains(path) && error.contains("an object"),
            "{error}"
        );
    }
    let error = format!("{:#}", parse_value(&serde_json::json!([])).err().unwrap());
    assert!(error.contains("an object"), "{error}");
}

#[test]
fn audio_gain_must_be_a_finite_non_negative_number() {
    let parse_audio = |track_gain: Value, clip_gain: Value| {
        parse_value(&serde_json::json!({
            "timeline": { "fps": 30, "duration_frames": 90 },
            "render_settings": { "width": 16, "height": 16 },
            "audio": {
                "tracks": [{ "id": "track-1", "gain": track_gain }],
                "clips": [{
                    "id": "clip-1",
                    "source_id": "voice",
                    "track_id": "track-1",
                    "start_ms": 0,
                    "duration_ms": 100,
                    "gain": clip_gain
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

    let comp = parse_audio(serde_json::json!(-0.0), serde_json::json!(2.5)).unwrap();
    let audio = comp.audio().unwrap();
    assert_eq!(audio.tracks[0].gain, 0.0.into());
    assert_eq!(audio.clips[0].gain, 2.5.into());

    for (invalid, track, clip) in [
        (
            serde_json::json!(-0.5),
            "audio.tracks[0].gain: must be at least 0",
            "audio.clips[0].gain: must be at least 0",
        ),
        (
            serde_json::json!(-1e-50),
            "audio.tracks[0].gain: must be at least 0",
            "audio.clips[0].gain: must be at least 0",
        ),
        (
            serde_json::json!(1e300),
            "track `track-1` gain",
            "clip `clip-1` gain",
        ),
        (
            serde_json::json!("loud"),
            "audio.tracks[0].gain",
            "audio.clips[0].gain",
        ),
    ] {
        let track_error = parse_audio(invalid.clone(), serde_json::json!(1)).unwrap_err();
        assert!(
            track_error.to_string().contains(track),
            "{invalid}: {track_error}"
        );
        let clip_error = parse_audio(serde_json::json!(1), invalid.clone()).unwrap_err();
        assert!(
            clip_error.to_string().contains(clip),
            "{invalid}: {clip_error}"
        );
    }
}

#[test]
fn audio_clip_times_are_whole_milliseconds() {
    let error = |clip: Value| {
        let audio = serde_json::json!({ "tracks": [{ "id": "main" }], "clips": [clip] });
        format!("{:#}", audio_composition(audio).unwrap_err())
    };
    let clip = |extra: Value| {
        let mut clip = serde_json::json!({
            "id": "a", "source_id": "voice", "track_id": "main", "start_ms": 0, "duration_ms": 100
        });
        clip.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        clip
    };
    for (key, value) in [
        ("start_ms", serde_json::json!(1.5)),
        ("start_ms", serde_json::json!(1.0)),
        ("duration_ms", serde_json::json!(1.5)),
        ("source_start_ms", serde_json::json!(1.5)),
        ("source_start_ms", serde_json::json!(-1)),
    ] {
        let message = error(clip(serde_json::json!({ key: value })));
        assert!(
            message.contains(&format!("audio.clips[0].{key}: invalid")),
            "{message}"
        );
    }

    let audio = serde_json::json!({
        "tracks": [{ "id": "main", "name": "Voice" }],
        "clips": [clip(serde_json::json!({ "name": "take 2" }))]
    });
    let composition = audio_composition(audio).unwrap();
    assert_eq!(composition.audio().unwrap().clips[0].source_start_ms, 0);
}

#[test]
fn audio_keys_the_format_does_not_name_fail() {
    let error = |audio: Value| format!("{:#}", audio_composition(audio).unwrap_err());
    let clip = |extra: Value| {
        let mut clip = serde_json::json!({
            "id": "a", "source_id": "voice", "track_id": "main", "start_ms": 0, "duration_ms": 100
        });
        clip.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        clip
    };
    let message = error(serde_json::json!({
        "tracks": [{ "id": "main" }],
        "clips": [clip(serde_json::json!({ "volume": 1 }))]
    }));
    assert!(
        message.contains("audio.clips[0].volume: unknown field `volume`"),
        "{message}"
    );
    let message = error(serde_json::json!({
        "tracks": [{ "id": "main", "volume": 1 }],
        "clips": []
    }));
    assert!(
        message.contains("audio.tracks[0].volume: unknown field `volume`"),
        "{message}"
    );
    let message = error(serde_json::json!({ "tracks": [], "clips": [], "mixer": {} }));
    assert!(
        message.contains("audio.mixer: unknown field `mixer`"),
        "{message}"
    );
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
        .graph()
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
            graph: Some(composition.graph()),
            media: None,
            glyph: None,
            instance: None,
            props: None,
        };

        assert_eq!(
            path.params
                .placement
                .position
                .eval(path.id, "position", &context)
                .unwrap(),
            expected
        );
    }
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
    let opacity = comp.graph().nodes.get(&NodeId::new(2)).expect("opacity");
    assert!(matches!(opacity, crate::node::NodeKind::Opacity(_)));
    assert_eq!(opacity.input_ports(), vec![Some(NodeId::new(1))]);
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
    let merge = comp.graph().nodes.get(&NodeId::new(3)).expect("merge");
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

fn param_error(node_type: &str, params: &str) -> String {
    let json = format!(
        r#"{{
                "timeline": {{ "fps": 30, "duration_frames": 1 }},
                "render_settings": {{ "width": 100, "height": 100 }},
                "nodes": [{{ "id": 1, "type": "{node_type}", "params": {params} }}],
                "connections": []
            }}"#
    );
    format!("{:#}", parse(&json).unwrap_err())
}

#[test]
fn fractional_int_param_says_whole_number() {
    let err = param_error("crop", r#"{ "height": 23.84 }"#);
    assert!(
        err.contains("`height` must be a whole number, got 23.84"),
        "{err}"
    );
}

#[test]
fn out_of_range_int_param_keeps_range_message() {
    let err = param_error("crop", r#"{ "height": 1e30 }"#);
    assert!(
        err.contains("`height` must be an integer within the i64 range"),
        "{err}"
    );
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

fn parse_with_schema_version(version: Value) -> Result<(), String> {
    let mut value: Value = serde_json::from_str(&composition_json(
        r#"{ "duration_frames": 1 }"#,
        r#"{ "width": 1, "height": 1 }"#,
    ))
    .unwrap();
    value["lumenSchemaVersion"] = version;
    parse_value(&value)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[test]
fn schema_version_is_absent_or_the_engines_line() {
    let own = SCHEMA_VERSION;
    assert_eq!(parse_with_schema_version(Value::from(own)), Ok(()));
    assert_eq!(
        parse_with_schema_version(Value::from(format!("{own}.0"))),
        Ok(())
    );
    assert_eq!(
        parse_with_schema_version(Value::from(format!("{own}.17"))),
        Ok(())
    );

    let absent: Value = serde_json::from_str(&composition_json(
        r#"{ "duration_frames": 1 }"#,
        r#"{ "width": 1, "height": 1 }"#,
    ))
    .unwrap();
    assert!(absent.get("lumenSchemaVersion").is_none());
    assert!(parse_value(&absent).is_ok());
}

#[test]
fn other_schema_versions_name_both_versions() {
    let own = SCHEMA_VERSION;
    let (major, minor) = own.split_once('.').unwrap();
    let minor: u64 = minor.parse().unwrap();
    let versions = [
        "0.1.0".to_string(),
        format!("{major}.{}", minor + 1),
        format!("1{own}"),
        String::new(),
        format!("{own}.x"),
        format!("v{own}"),
        format!(" {own} "),
        format!("{own}0"),
        format!("{own}."),
        format!("{own}.1.2"),
        format!("{own}.-1"),
    ];
    for version in &versions {
        let error = parse_with_schema_version(Value::from(version.as_str())).unwrap_err();
        assert_eq!(
            error,
            format!(
                "this composition declares schema version {version}; this engine reads \
                     {own}, so it was written for a different Lumen version"
            ),
            "{version:?}"
        );
    }
}

#[test]
fn non_string_schema_version_says_what_it_must_be() {
    let error = parse_with_schema_version(serde_json::json!(1)).unwrap_err();
    assert_eq!(
        error,
        format!(
            "`lumenSchemaVersion` must be a string such as \"{SCHEMA_VERSION}\", the Lumen \
                 version the composition was written for"
        )
    );
}

#[test]
fn schema_version_does_not_lag_the_crate_version() {
    let number = |value: &str| value.parse::<u64>().unwrap();
    let (major, minor) = SCHEMA_VERSION.split_once('.').unwrap();
    let crate_line = (
        number(env!("CARGO_PKG_VERSION_MAJOR")),
        number(env!("CARGO_PKG_VERSION_MINOR")),
    );
    assert!(
        crate_line <= (number(major), number(minor)),
        "lumen-engine is {} but SCHEMA_VERSION is {SCHEMA_VERSION}; run \
             `just set-schema-version {}.{}`",
        env!("CARGO_PKG_VERSION"),
        crate_line.0,
        crate_line.1,
    );
}

#[test]
fn root_keys_it_does_not_name_are_ignored_but_timeline_and_render_settings_keys_fail() {
    let timeline = r#"{ "duration_frames": 1 }"#;
    let settings = r#"{ "width": 1, "height": 1 }"#;
    let mut root: Value = serde_json::from_str(&composition_json(timeline, settings)).unwrap();
    root["schemaVersion"] = serde_json::json!("99.0.0");
    root["editor"] = serde_json::json!({ "zoom": 2 });
    assert!(parse_value(&root).is_ok());

    let error = parse(&composition_json(
        r#"{ "fpss": 60, "duration_frames": 1 }"#,
        settings,
    ))
    .unwrap_err()
    .to_string();
    assert!(
        error.contains("timeline.fpss: unknown field `fpss`"),
        "{error}"
    );

    let error = parse(&composition_json(
        timeline,
        r#"{ "width": 1, "height": 1, "scale": 2 }"#,
    ))
    .unwrap_err()
    .to_string();
    assert!(
        error.contains("render_settings.scale: unknown field"),
        "{error}"
    );
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
        assert!(err.contains("duration_frames"), "{timeline}: {err}");
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
    assert_eq!(comp.timeline().duration_frames, 10);
    assert_eq!(
        (comp.render_settings().width, comp.render_settings().height),
        (100, 50)
    );
}

/// A root that plays `card` with `clip` params, where `card` declares a string `title` and a
/// number `index` and draws `card_nodes` into its output.
fn clip_composition(clip: Value, card_nodes: Value) -> Value {
    serde_json::json!({
        "timeline": { "fps": 30, "duration_frames": 60 },
        "render_settings": { "width": 16, "height": 16 },
        "compositions": {
            "card": {
                "props": {
                    "title": { "type": "string", "default": "Untitled" },
                    "index": { "type": "number" }
                },
                "nodes": card_nodes,
                "connections": [{ "from_node": 1, "to_node": 2, "to_port": "source" }]
            }
        },
        "nodes": [
            { "id": 1, "type": "clip", "params": clip },
            { "id": 2, "type": "media_output" }
        ],
        "connections": [{ "from_node": 1, "to_node": 2, "to_port": "source" }]
    })
}

fn card_nodes(paint: &str) -> Value {
    serde_json::json!([
        { "id": 1, "type": "background", "params": { "paint": paint } },
        { "id": 2, "type": "media_output" }
    ])
}

fn clip_error(clip: Value, paint: &str) -> String {
    format!(
        "{:#}",
        parse_value(&clip_composition(clip, card_nodes(paint))).unwrap_err()
    )
}

#[test]
fn template_node_ids_are_scoped_to_the_template() {
    let comp = parse_value(&clip_composition(
        serde_json::json!({ "composition": "card", "props": { "title": "A", "index": 2 } }),
        card_nodes("=rgb(prop('index'), 0, 0)"),
    ))
    .unwrap();
    let card = &comp.compositions()["card"];
    assert!(matches!(
        card.scene.graph.nodes[&NodeId::new(1)],
        NodeKind::Background(_)
    ));
    assert!(matches!(
        comp.graph().nodes[&NodeId::new(1)],
        NodeKind::Clip(_)
    ));
    assert_eq!(card.props["index"].default, ExpressionValue::Number(0.0));
    assert_eq!(
        card.scene.timeline.duration_frames, 60,
        "a template without a timeline takes the root's"
    );
}

#[test]
fn clip_props_are_checked_against_the_template() {
    let paint = "#ff0000";
    for (clip, expected) in [
        (
            serde_json::json!({ "composition": "card", "props": { "titel": "x" } }),
            "node 1: composition `card` has no prop `titel`; the template declares `index`, `title`",
        ),
        (
            serde_json::json!({ "composition": "card", "props": { "index": "two" } }),
            "prop `index` of `card`: expected a number, got \"two\"",
        ),
        (
            serde_json::json!({ "composition": "missing" }),
            "composition `missing` is not defined; `compositions` has `card`",
        ),
        (
            serde_json::json!({ "props": { "title": "x" } }),
            "prop `title` is set, but the clip plays its `source`",
        ),
        (
            serde_json::json!({ "composition": "card", "props": "=1" }),
            "node 1: `props` does not support expressions",
        ),
    ] {
        let error = clip_error(clip.clone(), paint);
        assert!(error.contains(expected), "{clip}: {error}");
    }
}

#[test]
fn prop_reads_must_name_a_declared_prop_inside_a_template() {
    let clip = serde_json::json!({ "composition": "card" });
    for (paint, expected) in [
        (
            "=rgb(prop('size'), 0, 0)",
            "unknown prop `size` in `=rgb(prop('size'), 0, 0)`; the template declares `index`, `title`",
        ),
        (
            "=rgb(prop(uppercase('index')), 0, 0)",
            "`prop` takes the prop's name as a string literal",
        ),
    ] {
        let error = clip_error(clip.clone(), paint);
        assert!(error.contains("composition `card`"), "{error}");
        assert!(error.contains(expected), "{paint}: {error}");
    }

    let mut root_read = clip_composition(clip, card_nodes("#000000"));
    root_read["nodes"][0] = serde_json::json!({
        "id": 1, "type": "background", "params": { "paint": "=rgb(prop('index'), 0, 0)" }
    });
    let error = format!("{:#}", parse_value(&root_read).unwrap_err());
    assert!(
        error.contains("`prop('index')` is only available inside a composition template"),
        "{error}"
    );
}

#[test]
fn prop_reads_are_checked_however_deeply_a_param_holds_them() {
    let read = "=prop('index')";
    for (kind, params) in [
        (
            "text",
            serde_json::json!({ "content": [{ "text": "a", "color": "=rgb(prop('index'), 0, 0)" }] }),
        ),
        (
            "text",
            serde_json::json!({ "content": [{ "text": "a", "underline": { "thickness": read } }] }),
        ),
        (
            "text",
            serde_json::json!({ "font_features": { "tnum": read } }),
        ),
        (
            "wgsl_shader",
            serde_json::json!({ "uniforms": { "amount": [1, read] } }),
        ),
        ("clip", serde_json::json!({ "props": { "title": read } })),
    ] {
        let document = serde_json::json!({
            "timeline": { "fps": 30, "duration_frames": 1 },
            "render_settings": { "width": 8, "height": 8 },
            "nodes": [{ "id": 1, "type": kind, "params": params }],
            "connections": []
        });
        let error = format!("{:#}", parse_value(&document).unwrap_err());
        assert!(
            error.contains("`prop('index')` is only available inside a composition template"),
            "{kind} {params}: {error}"
        );
    }
}

#[test]
fn prop_declarations_need_a_type_and_a_default_of_it() {
    for (props, expected) in [
        (
            serde_json::json!({ "a": { "type": "bool" } }),
            "props.a.type: unknown variant `bool`, expected one of `string`, `number`, `color`, `vec2`, `vec3`",
        ),
        (
            serde_json::json!({ "a": { "type": "color", "default": "red" } }),
            "prop `a` default: expected a color, got \"red\"",
        ),
        (
            serde_json::json!({ "a": { "type": "color", "default": "#+f+f+f" } }),
            "prop `a` default: expected a color, got \"#+f+f+f\"",
        ),
        (
            serde_json::json!({ "a": { "type": "vec2", "default": [1] } }),
            "expected a vec2",
        ),
        (
            serde_json::json!({ "a": { "type": "vec3", "default": [1, 2] } }),
            "expected a vec3",
        ),
        (
            serde_json::json!({ "a": { "type": "number", "deflt": 1 } }),
            "props.a.deflt: unknown field `deflt`",
        ),
    ] {
        let mut comp = clip_composition(
            serde_json::json!({ "composition": "card" }),
            card_nodes("#000000"),
        );
        comp["compositions"]["card"]["props"] = props.clone();
        let error = format!("{:#}", parse_value(&comp).unwrap_err());
        assert!(error.contains(expected), "{props}: {error}");
    }
}

#[test]
fn templates_that_instantiate_themselves_are_rejected() {
    let template = |plays: &str| {
        serde_json::json!({
            "nodes": [
                { "id": 1, "type": "clip", "params": { "composition": plays } },
                { "id": 2, "type": "media_output" }
            ],
            "connections": [{ "from_node": 1, "to_node": 2, "to_port": "source" }]
        })
    };
    let mut comp = clip_composition(
        serde_json::json!({ "composition": "a" }),
        card_nodes("#000000"),
    );
    comp["compositions"] = serde_json::json!({ "a": template("b"), "b": template("a") });
    let error = format!("{:#}", parse_value(&comp).unwrap_err());
    assert!(
        error.contains("compositions instantiate themselves: `a` -> `b` -> `a`"),
        "{error}"
    );
    comp["compositions"] = serde_json::json!({ "a": template("a") });
    let error = format!("{:#}", parse_value(&comp).unwrap_err());
    assert!(error.contains("`a` -> `a`"), "{error}");
}

#[test]
fn build_rejects_templates_that_instantiate_themselves() {
    let mut comp = parse_value(&clip_composition(
        serde_json::json!({ "composition": "card" }),
        card_nodes("#000000"),
    ))
    .unwrap()
    .into_builder();
    let NodeKind::Clip(clip) = &comp.scene.graph.nodes[&NodeId::new(1)] else {
        unreachable!("node 1 is the clip");
    };
    let clip = NodeKind::Clip(clip.clone());
    let card = comp.compositions.get_mut("card").unwrap();
    card.scene.graph.nodes.insert(NodeId::new(1), clip);
    let errors = comp.build().unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.to_string().contains("`card` -> `card`")),
        "{errors:?}"
    );
}

#[test]
fn template_cycle_checks_visit_each_template_once() {
    // Template i plays i + 1 and i + 2, so following every path visits 2^(n/2) or more.
    let count = 40;
    let mut comp = clip_composition(
        serde_json::json!({ "composition": "t0" }),
        card_nodes("#000000"),
    );
    for index in 0..count {
        let plays = (index + 1..count.min(index + 3)).map(|next| format!("t{next}"));
        let mut nodes = vec![serde_json::json!({ "id": 1, "type": "background" })];
        nodes.extend(plays.enumerate().map(|(clip, name)| {
            serde_json::json!({ "id": clip + 3, "type": "clip", "params": { "composition": name } })
        }));
        nodes.push(serde_json::json!({ "id": 2, "type": "media_output" }));
        comp["compositions"][format!("t{index}")] = serde_json::json!({
            "nodes": nodes,
            "connections": [{ "from_node": 1, "to_node": 2, "to_port": "source" }]
        });
    }
    let started = std::time::Instant::now();
    parse_value(&comp).unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(1));
}

#[test]
fn explicit_zero_duration_hides_the_clip_and_negative_literals_are_rejected() {
    let shown_at = |duration: Value, frame: f64| {
        let comp = parse_value(&clip_composition(
            serde_json::json!({ "composition": "card", "duration": duration }),
            card_nodes("#000000"),
        ))
        .unwrap();
        let scope = crate::scope::Scope::root(&comp);
        let NodeKind::Clip(clip) = &comp.graph().nodes[&NodeId::new(1)] else {
            unreachable!("node 1 is the clip");
        };
        clip.route(&scope, frame.into(), None).unwrap().is_some()
    };
    assert!(!shown_at(serde_json::json!(0), 0.0));
    assert!(shown_at(serde_json::json!(-1), 59.0));
    assert!(shown_at(serde_json::json!("=20 - frame"), 9.0));
    assert!(!shown_at(serde_json::json!("=20 - frame"), 30.0));

    let error = clip_error(
        serde_json::json!({ "composition": "card", "duration": -2 }),
        "#000000",
    );
    assert!(
        error.contains("`duration` must be at least 0, or -1 to play to the end, got -2"),
        "{error}"
    );
}

#[test]
fn clip_rejects_a_composition_and_a_source_together() {
    let mut comp = clip_composition(
        serde_json::json!({ "composition": "card" }),
        card_nodes("#000000"),
    );
    comp["nodes"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({ "id": 3, "type": "background" }));
    comp["connections"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({ "from_node": 3, "to_node": 1, "to_port": "source" }));
    let error = format!("{:#}", parse_value(&comp).unwrap_err());
    assert!(
        error.contains("plays either a `composition` or its `source` input"),
        "{error}"
    );
}

#[test]
fn prop_expressions_of_the_wrong_type_fail_when_evaluated() {
    let comp = parse_value(&clip_composition(
        serde_json::json!({ "composition": "card", "props": { "index": "='two'" } }),
        card_nodes("#000000"),
    ))
    .unwrap();
    let scope = crate::scope::Scope::root(&comp);
    let error = crate::scope::route(
        &scope,
        &comp.graph().nodes[&NodeId::new(1)],
        0.0.into(),
        None,
        false,
    )
    .unwrap_err()
    .to_string();
    assert!(
        error.contains("node 1 `props.index`: expected a number, got string"),
        "{error}"
    );
}

fn codes_and_paths(diagnostics: &[Diagnostic]) -> Vec<(&'static str, &str, Option<u64>)> {
    diagnostics
        .iter()
        .map(|diagnostic| {
            (
                diagnostic.code,
                diagnostic.path.as_str(),
                diagnostic.node.map(|node| node.0),
            )
        })
        .collect()
}

#[test]
fn a_phase_reports_every_error_with_its_path_and_stops_the_check() {
    let document = serde_json::json!({
        "timeline": { "fps": 30, "duration_frames": 10 },
        "render_settings": { "width": 16, "height": 16 },
        "compositions": { "card/a": {
            "nodes": [
                { "id": 1, "type": "shape", "params": { "opacity": "=1 +" } },
                { "id": 2, "type": "media_output" }
            ],
            "connections": []
        } },
        "nodes": [
            { "id": 1, "type": "background", "params": { "paintt": "#fff", "width": "wide" } },
            { "id": 2, "type": "sparkle" },
            { "id": 2, "type": "media_output" },
            { "id": 3, "type": "color_grade", "params": { "lut": "rgb1d:0,0,0;255,128,0" } },
            { "id": 4, "type": "color_grade",
              "params": { "lut": "rgb1d:0,0,0;1,1,1", "lut_source": "grade.cube" } },
            { "id": 5, "type": "color_grade", "params": { "lut_source": "rgb1d:0,0,0;1,1,1" } }
        ],
        // Not reported: connections are checked once every node reads.
        "connections": [{ "from_node": 7, "to_node": 2, "to_port": "source" }]
    });
    let diagnostics = check_value(&document, None, None);
    assert_eq!(
        codes_and_paths(&diagnostics),
        [
            ("node.unknown_param", "/nodes/0/params/paintt", Some(1)),
            ("node.invalid_param", "/nodes/0/params/width", Some(1)),
            ("node.unknown_type", "/nodes/1/type", Some(2)),
            ("graph.duplicate_node_id", "/nodes/2", Some(2)),
            ("node.invalid_param", "/nodes/3/params/lut", Some(3)),
            ("node.invalid_param", "/nodes/4/params/lut", Some(4)),
            ("node.invalid_param", "/nodes/5/params/lut_source", Some(5)),
            (
                "node.invalid_param",
                "/compositions/card~1a/nodes/0/params/opacity",
                Some(1)
            ),
        ]
    );
    assert!(diagnostics.iter().all(Diagnostic::is_error));
    assert!(
        diagnostics[7]
            .message
            .starts_with("composition `card/a`: node 1: "),
        "{}",
        diagnostics[7].message
    );
    assert!(
        diagnostics[4].message.contains("`lut` value 255")
            && diagnostics[4].message.contains("0..1"),
        "{}",
        diagnostics[4].message
    );
    assert!(
        diagnostics[6].message.contains("is the `lut` param"),
        "{}",
        diagnostics[6].message
    );
    assert_eq!(
        parse_value(&document).unwrap_err().diagnostics(),
        diagnostics.as_slice()
    );
}

#[test]
fn settings_a_template_inherits_are_reported_at_the_root() {
    let template = |extra: serde_json::Value| {
        let mut card = serde_json::json!({
            "nodes": [{ "id": 5, "type": "media_output" }],
            "connections": []
        });
        card.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        card
    };
    let document = serde_json::json!({
        "timeline": { "fps": 0, "duration_frames": 10 },
        "render_settings": { "width": 0, "height": 16 },
        "compositions": {
            "inherits": template(serde_json::json!({})),
            "partial": template(serde_json::json!({ "timeline": { "duration_frames": 5 } })),
            "own": template(serde_json::json!({
                "timeline": { "fps": 30, "duration_frames": 0 },
                "render_settings": { "width": 8, "height": 8 }
            })),
        },
        "nodes": [{ "id": 2, "type": "media_output" }],
        "connections": []
    });
    let settings = check_value(&document, None, None)
        .into_iter()
        .filter(|diagnostic| diagnostic.code == "settings.invalid")
        .collect::<Vec<_>>();
    assert_eq!(
        codes_and_paths(&settings),
        [
            ("settings.invalid", "/timeline/fps", None),
            ("settings.invalid", "/render_settings/width", None),
            (
                "settings.invalid",
                "/compositions/own/timeline/duration_frames",
                None
            ),
        ]
    );
}

#[test]
fn graph_and_build_errors_point_at_what_they_name() {
    let document = serde_json::json!({
        "timeline": { "fps": 0, "duration_frames": 10 },
        "render_settings": { "width": 16, "height": 16 },
        "compositions": { "card": {
            "nodes": [{ "id": 4, "type": "opacity" }, { "id": 5, "type": "media_output" }],
            "connections": [{ "from_node": 4, "to_node": 5, "to_port": "source" }]
        } },
        "nodes": [
            { "id": 1, "type": "clip", "params": { "composition": "missing" } },
            { "id": 2, "type": "media_output" }
        ],
        "connections": [{ "from_node": 1, "to_node": 2, "to_port": "source" }]
    });
    assert_eq!(
        codes_and_paths(&check_value(&document, None, None)),
        [
            // The template takes the root's timeline, so the error is reported once.
            ("settings.invalid", "/timeline/fps", None),
            (
                "graph.missing_required_input",
                "/compositions/card/nodes/0",
                Some(4)
            ),
            ("clip.invalid", "/nodes/0/params", Some(1)),
        ]
    );

    let mut connections = document.clone();
    connections["timeline"]["fps"] = serde_json::json!(30);
    connections["connections"] = serde_json::json!([
        { "from_node": 1, "to_node": 2, "to_port": "source" },
        { "from_node": 9, "to_node": 2, "to_port": "source" },
        { "from_node": 1, "from_port": "mask", "to_node": 2, "to_port": "source" }
    ]);
    assert_eq!(
        codes_and_paths(&check_value(&connections, None, None)),
        [
            (
                "connection.missing_source_node",
                "/connections/1/from_node",
                Some(2)
            ),
            (
                "connection.invalid_from_port",
                "/connections/2/from_port",
                None
            ),
        ]
    );

    assert_eq!(
        codes_and_paths(&check("{ \"nodes\": ", None, None)),
        [("document.syntax", "", None)]
    );
    let mut shape = document;
    shape["timeline"]["fpss"] = serde_json::json!(30);
    assert_eq!(
        codes_and_paths(&check_value(&shape, None, None)),
        [("document.invalid", "/timeline/fpss", None)]
    );
}

/// A box drawn by a `render_3d` into the output, with `connections` added.
fn scene_document(nodes: Value, connections: Value) -> Value {
    let mut all = vec![
        serde_json::json!({ "id": 1, "type": "shape_3d" }),
        serde_json::json!({ "id": 2, "type": "render_3d" }),
        serde_json::json!({ "id": 3, "type": "media_output" }),
    ];
    all.extend(nodes.as_array().unwrap().iter().cloned());
    let mut wired = vec![
        serde_json::json!({ "from_node": 1, "to_node": 2, "to_port": "scene" }),
        serde_json::json!({ "from_node": 2, "to_node": 3, "to_port": "source" }),
    ];
    wired.extend(connections.as_array().unwrap().iter().cloned());
    serde_json::json!({
        "timeline": { "fps": 30, "duration_frames": 1 },
        "render_settings": { "width": 16, "height": 16 },
        "nodes": all,
        "connections": wired
    })
}

#[test]
fn scenes_and_rasters_only_wire_into_inputs_of_their_kind() {
    let document = scene_document(
        serde_json::json!([
            { "id": 4, "type": "opacity" },
            { "id": 5, "type": "background" },
            { "id": 6, "type": "merge_3d" }
        ]),
        serde_json::json!([
            { "from_node": 1, "to_node": 4, "to_port": "source" },
            { "from_node": 5, "to_node": 6, "to_port": "objects" }
        ]),
    );
    let diagnostics = check_value(&document, None, None);
    let wrong: Vec<(&str, &str)> = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == "connection.wrong_kind")
        .map(|diagnostic| (diagnostic.path.as_str(), diagnostic.message.as_str()))
        .collect();
    assert_eq!(
        wrong,
        [
            (
                "/connections/2",
                "node 4: input `source` takes a raster, but node 1 outputs a scene: draw the scene into a raster through a render_3d, and wire that in"
            ),
            (
                "/connections/3",
                "node 6: input `objects` takes a scene, but node 5 outputs a raster: show the raster in the scene through an image_plane_3d"
            ),
        ]
    );
}

#[test]
fn a_gradient_where_3d_needs_a_solid_color_is_reported_at_its_param() {
    let gradient = serde_json::json!({
        "type": "linear",
        "stops": [{ "offset": 0, "color": "#000000" }, { "offset": 1, "color": "#ffffff" }]
    });
    let diagnostics = check_value(
        &scene_document(
            serde_json::json!([
                { "id": 4, "type": "shape_3d", "params": { "color": gradient } },
                { "id": 5, "type": "light_3d", "params": { "color": gradient } },
                { "id": 6, "type": "render_3d", "params": { "fog_color": gradient } },
                { "id": 7, "type": "image_plane_3d", "params": { "color": gradient } },
                { "id": 8, "type": "shape_3d", "params": { "color": "=rgb(1, 2, 3)" } },
                { "id": 9, "type": "light_3d", "params": { "color": "#ff0000" } }
            ]),
            serde_json::json!([{ "from_node": 3, "to_node": 7, "to_port": "source" }]),
        ),
        None,
        None,
    );
    let invalid: Vec<_> = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == "node.invalid_param")
        .collect();
    assert_eq!(
        invalid
            .iter()
            .map(|diagnostic| (diagnostic.path.as_str(), diagnostic.node.map(|node| node.0)))
            .collect::<Vec<_>>(),
        [
            ("/nodes/3/params/color", Some(4)),
            ("/nodes/4/params/color", Some(5)),
            ("/nodes/5/params/fog_color", Some(6)),
            ("/nodes/6/params/color", Some(7)),
        ]
    );
    assert_eq!(
        invalid[0].message,
        "node 4: `color` expected a solid color, got a gradient: a gradient can't light a surface"
    );
}

#[test]
fn scene_nodes_need_an_engine_built_with_3d() {
    let diagnostics = check_value(
        &scene_document(
            serde_json::json!([
                { "id": 4, "type": "background" },
                { "id": 5, "type": "image_plane_3d" },
                { "id": 6, "type": "model_3d", "params": { "source": "robot.glb" } }
            ]),
            serde_json::json!([{ "from_node": 4, "to_node": 5, "to_port": "source" }]),
        ),
        None,
        None,
    );
    if cfg!(feature = "3d") {
        assert_eq!(diagnostics, []);
    } else {
        assert_eq!(
            codes_and_paths(&diagnostics),
            [
                ("node.unsupported", "/nodes/0/type", Some(1)),
                ("node.unsupported", "/nodes/1/type", Some(2)),
                ("node.unsupported", "/nodes/4/type", Some(5)),
                ("node.unsupported", "/nodes/5/type", Some(6)),
            ]
        );
    }
}

#[test]
fn diagnostics_are_capped() {
    let nodes = (0..80)
        .map(|id| serde_json::json!({ "id": id, "type": "sparkle" }))
        .collect::<Vec<_>>();
    let document = serde_json::json!({
        "timeline": { "duration_frames": 1 },
        "render_settings": { "width": 1, "height": 1 },
        "nodes": nodes,
        "connections": []
    });
    assert_eq!(check_value(&document, None, None).len(), MAX_DIAGNOSTICS);
}

#[test]
fn check_warns_of_literal_families_the_host_cannot_resolve() {
    let document = serde_json::json!({
        "timeline": { "duration_frames": 1 },
        "render_settings": { "width": 16, "height": 16 },
        "nodes": [
            { "id": 3, "type": "text", "params": {
                "font_family": "Brand",
                "content": [
                    { "text": "a", "font_family": "Known" },
                    { "text": "b", "font_family": "Display" },
                    { "text": "c" }
                ]
            } },
            { "id": 4, "type": "text", "params": {
                "font_family": "Unused",
                "content": [{ "text": "a", "font_family": "Roboto" }]
            } },
            // Only rendering knows what an expression names.
            { "id": 5, "type": "text", "params": { "font_family": "=uppercase('brand')" } },
            { "id": 6, "type": "media_output" }
        ],
        "connections": [{ "from_node": 3, "to_node": 6, "to_port": "source" }]
    });
    let known = |family: &str| family == "Known";
    let diagnostics = check_value(&document, Some(&known), None);
    assert_eq!(
        codes_and_paths(&diagnostics),
        [
            ("font.fallback", "/nodes/0/params/font_family", Some(3)),
            (
                "font.fallback",
                "/nodes/0/params/content/1/font_family",
                Some(3)
            ),
        ]
    );
    assert_eq!(
        diagnostics[1].severity,
        crate::diagnostic::Severity::Warning
    );
    assert!(
        diagnostics[1].message.contains("`Display`"),
        "{}",
        diagnostics[1].message
    );
    assert!(check_value(&document, None, None).is_empty());
    parse_value(&document).unwrap();
}

#[cfg(feature = "3d")]
#[test]
fn check_warns_of_a_text_3d_family_the_host_cannot_resolve() {
    let document = scene_document(
        serde_json::json!([
            { "id": 4, "type": "text_3d", "params": { "font_family": "Brand" } },
            { "id": 5, "type": "text_3d", "params": { "font_family": "Known" } }
        ]),
        serde_json::json!([]),
    );
    let known = |family: &str| family == "Known";
    let diagnostics = check_value(&document, Some(&known), None);
    assert_eq!(
        codes_and_paths(&diagnostics),
        [("font.fallback", "/nodes/3/params/font_family", Some(4))]
    );
}

#[cfg(feature = "3d")]
#[test]
fn check_rejects_text_3d_literals_that_no_text_has() {
    let document = scene_document(
        serde_json::json!([
            { "id": 4, "type": "text_3d", "params": {
                "depth": 1e40, "bevel_size": -1, "font_size": 2, "font_weight": 0,
                "letter_spacing": -0.1, "line_height": "=2 * 3"
            } },
            { "id": 5, "type": "text_3d", "params": { "max_width": 1e300 } }
        ]),
        serde_json::json!([]),
    );
    let diagnostics = check_value(&document, None, None);
    let mut found = codes_and_paths(&diagnostics);
    found.sort();
    assert_eq!(
        found,
        [
            ("node.invalid_param", "/nodes/3/params/bevel_size", Some(4)),
            ("node.invalid_param", "/nodes/3/params/depth", Some(4)),
            ("node.invalid_param", "/nodes/3/params/font_weight", Some(4)),
            ("node.invalid_param", "/nodes/4/params/max_width", Some(5)),
        ]
    );
}

#[derive(Debug)]
struct Assets;

impl AssetResolver for Assets {
    fn resolve(&self, id: &str) -> Option<crate::media::Asset> {
        use crate::media::Asset;
        let path = std::path::PathBuf::from(id);
        match id {
            "photo" => Some(Asset::Image(path)),
            "clip" => Some(Asset::Video(path)),
            "voice" => Some(Asset::Audio(path)),
            "brand" => Some(Asset::Font(vec![path])),
            "robot" => Some(Asset::Model(path)),
            _ => None,
        }
    }
}

// Without 3D, the node fails as unsupported before assets are checked.
#[cfg(feature = "3d")]
#[test]
fn model_sources_must_reference_model_assets() {
    let model = |id: u64, source: &str| serde_json::json!({ "id": id, "type": "model_3d", "params": { "source": source } });
    let diagnostics = check_value(
        &scene_document(
            serde_json::json!([
                model(4, "asset:robot"),
                model(5, "asset:photo"),
                model(6, "asset:gone")
            ]),
            serde_json::json!([]),
        ),
        None,
        Some(&Assets),
    );
    let assets: Vec<_> = codes_and_paths(&diagnostics)
        .into_iter()
        .filter(|(code, ..)| code.starts_with("media."))
        .collect();
    assert_eq!(
        assets,
        [
            ("media.wrong_kind", "/nodes/4/params/source", Some(5)),
            ("media.not_found", "/nodes/5/params/source", Some(6)),
        ]
    );
    let wrong_kind = diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == "media.wrong_kind")
        .unwrap();
    assert!(
        wrong_kind.message.contains("is an image, not a model"),
        "{}",
        wrong_kind.message
    );
}

#[test]
fn check_finds_asset_references_the_host_lacks_or_holds_as_another_kind() {
    let media = |id: u64, kind: &str, source: &str| serde_json::json!({ "id": id, "type": "media_in", "params": { "kind": kind, "source": source } });
    let document = serde_json::json!({
        "timeline": { "duration_frames": 1 },
        "render_settings": { "width": 16, "height": 16 },
        "compositions": {
            "card": {
                "nodes": [
                    media(1, "image", "asset:clip"),
                    { "id": 2, "type": "media_output" }
                ],
                "connections": [{ "from_node": 1, "to_node": 2, "to_port": "source" }],
                "audio": {
                    "tracks": [{ "id": "t", "name": "T", "muted": false, "solo": false, "gain": 1 }],
                    "clips": [
                        { "id": "c", "source_id": "asset:gone", "track_id": "t", "start_ms": 0, "duration_ms": 10 }
                    ]
                }
            }
        },
        "nodes": [
            media(1, "image", "asset:photo"),
            media(2, "video", "asset:clip"),
            media(3, "video", "asset:gone"),
            media(4, "image", "photo.png"),
            { "id": 5, "type": "text", "params": {
                "font_family": "asset:brand",
                "content": [{ "text": "a", "font_family": "asset:photo" }, { "text": "b" }]
            } },
            { "id": 6, "type": "media_output" },
            media(7, "=if(frame < 5, 0, 1)", "asset:clip"),
            media(8, "=if(frame < 5, 0, 1)", "asset:photo"),
            media(9, "=if(frame < 5, 0, 1)", "asset:brand"),
            media(10, "=if(frame < 5, 0, 1)", "asset:voice")
        ],
        "connections": [{ "from_node": 5, "to_node": 6, "to_port": "source" }],
        "audio": {
            "tracks": [{ "id": "t", "name": "T", "muted": false, "solo": false, "gain": 1 }],
            "clips": [
                { "id": "a", "source_id": "asset:voice", "track_id": "t", "start_ms": 0, "duration_ms": 10 },
                { "id": "b", "source_id": "asset:brand", "track_id": "t", "start_ms": 0, "duration_ms": 10 }
            ]
        }
    });
    let unknown = |_: &str| false;
    assert_eq!(
        codes_and_paths(&check_value(&document, Some(&unknown), Some(&Assets))),
        [
            ("media.not_found", "/nodes/2/params/source", Some(3)),
            (
                "media.wrong_kind",
                "/nodes/4/params/content/0/font_family",
                Some(5)
            ),
            ("media.wrong_kind", "/nodes/8/params/source", Some(9)),
            ("media.wrong_kind", "/nodes/9/params/source", Some(10)),
            (
                "media.wrong_kind",
                "/compositions/card/nodes/0/params/source",
                Some(1)
            ),
            ("media.wrong_kind", "/audio/clips/1/source_id", None),
            (
                "media.not_found",
                "/compositions/card/audio/clips/0/source_id",
                None
            ),
        ]
    );
    // Without the asset store, references aren't checked, nor taken for families.
    assert!(check_value(&document, Some(&unknown), None).is_empty());
    let no_assets = check_value(&document, None, Some(&crate::media::NoAssets));
    assert_eq!(no_assets.len(), 13);
    assert!(
        no_assets
            .iter()
            .all(|diagnostic| diagnostic.code == "media.not_found")
    );
}
