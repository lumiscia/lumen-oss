use lumen_engine::node::{NodeKind, NodeSchemaDef};
use serde_json::{Value, json};

const SOURCE_ID: i64 = 100;
const OUTPUT_ID: i64 = 101;

fn schema() -> Value {
    let schema_json = lumen_generators::render_composition_schema_json().unwrap();
    serde_json::from_str(&schema_json).unwrap()
}

fn schema_validator() -> jsonschema::Validator {
    jsonschema::validator_for(&schema()).unwrap()
}

fn node_schema(kind: &str) -> NodeSchemaDef {
    NodeKind::schemas()
        .into_iter()
        .find(|schema| schema.kind == kind)
        .unwrap()
}

/// The default the schema gives a node param, absent for optional params.
fn default_param<'a>(schema: &'a Value, kind: &str, id: &str) -> Option<&'a Value> {
    schema["properties"]["nodes"]["items"]["oneOf"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["properties"]["type"]["const"] == kind)?
        .pointer(&format!("/properties/params/properties/{id}/default"))
}

fn composition(node: Value, connections: Vec<Value>) -> Value {
    json!({
        "timeline": { "fps": 30, "duration_frames": 10 },
        "render_settings": { "width": 16, "height": 16 },
        "nodes": [
            node,
            { "id": SOURCE_ID, "type": "background" },
            { "id": OUTPUT_ID, "type": "media_output" }
        ],
        "connections": connections
    })
}

/// A renderable composition containing `node` with `params`, its required inputs fed by a
/// background node, and its output wired to the media output.
fn composition_with_node(node: &NodeSchemaDef, params: Value) -> Value {
    let mut connections = node
        .inputs
        .iter()
        .filter(|input| !input.optional)
        .map(|input| json!({ "from_node": SOURCE_ID, "to_node": 1, "to_port": input.name }))
        .collect::<Vec<_>>();
    connections.push(json!({ "from_node": 1, "to_node": OUTPUT_ID, "to_port": "source" }));
    composition(
        json!({ "id": 1, "type": node.kind, "params": params }),
        connections,
    )
}

#[test]
fn expression_support_matches_engine_parsing() {
    let schema = schema();
    let validator = jsonschema::validator_for(&schema).unwrap();
    for node in NodeKind::schemas() {
        for param in &node.properties {
            let label = format!("{}.{}", node.kind, param.id);
            if let Some(default) = default_param(&schema, node.kind, param.id) {
                let literal = composition_with_node(&node, json!({ param.id: default }));
                lumen_engine::json::parse_value(&literal)
                    .unwrap_or_else(|error| panic!("{label} literal should parse: {error:#}"));
                assert!(
                    validator.is_valid(&literal),
                    "{label} literal should validate"
                );
            }

            let expression = composition_with_node(&node, json!({ param.id: "=1" }));
            let engine_accepts = lumen_engine::json::parse_value(&expression).is_ok();
            assert_eq!(engine_accepts, param.accepts_expressions, "{label} engine");
            assert_eq!(
                validator.is_valid(&expression),
                param.accepts_expressions,
                "{label} schema"
            );
        }
    }
}

#[test]
fn text_kind_schemas_match_engine_parsing() {
    use lumen_engine::node::{
        DelegateEvaluable, Delegated,
        source::text_content::{
            TextContent, TextContentDelegate, TextHighlightDelegate, TextLineDelegate,
            TextSpanDelegate, TextStrokeDelegate,
        },
    };

    let schema = schema();
    let validator = |def: &str| {
        jsonschema::validator_for(&json!({
            "$defs": schema["$defs"].clone(),
            "$ref": format!("#/$defs/{def}")
        }))
        .unwrap()
    };
    let content = validator("textContent");

    for (def, value) in [
        (
            "textContent",
            TextContent::default()
                .into_delegate()
                .to_json_value()
                .unwrap(),
        ),
        (
            "textSpan",
            TextSpanDelegate::default().to_json_value().unwrap(),
        ),
        (
            "textLine",
            TextLineDelegate::default().to_json_value().unwrap(),
        ),
        (
            "textHighlight",
            TextHighlightDelegate::default().to_json_value().unwrap(),
        ),
        (
            "textStroke",
            TextStrokeDelegate::default().to_json_value().unwrap(),
        ),
    ] {
        assert!(validator(def).is_valid(&value), "{def} default: {value}");
    }

    let example = json!([
        { "text": "Hello, " },
        {
            "text": "=prop('name')",
            "font_family": "Inter",
            "font_size": 48.0,
            "font_weight": "=frame * 10",
            "font_style": "italic",
            "letter_spacing": -0.5,
            "font_features": { "liga": "=frame % 2", "tnum": 1 },
            "color": [255, 0, 0, 255],
            "underline": {
                "style": "wavy",
                "paint": "=rgb(0, 0, 255)",
                "thickness": 2.0,
                "offset": 3.0,
                "extent": "=vec2(0, frame / 24)"
            },
            "strikethrough": null,
            "highlight": {
                "paint": [255, 255, 0, 128],
                "padding": [4.0, 2.0],
                "radius": 3.0,
                "extent": [0.0, 1.0]
            },
            "stroke": { "paint": [0, 0, 0, 255], "width": "=frame / 4", "align": "center" }
        }
    ]);
    let parsed = TextContentDelegate::from_json_value(&example, "content").unwrap();
    assert_eq!(parsed.to_json_value(), Some(example.clone()));
    assert!(content.is_valid(&example));

    let enum_expressions = json!([{
        "text": "a",
        "font_style": "=frame % 3",
        "underline": { "style": "=1" }
    }]);
    assert!(content.is_valid(&enum_expressions));
    assert!(TextContentDelegate::from_json_value(&enum_expressions, "content").is_ok());

    for invalid in [
        json!(5),
        json!([{ "text": "a", "font_wieght": 700 }]),
        json!([{ "text": "a", "font_size": null }]),
        json!([{ "font_size": 12 }]),
        json!([{ "text": "a", "font_style": "bold" }]),
        json!([{ "text": "a", "underline": "=1" }]),
        json!([{ "text": "a", "highlight": { "radius": "big" } }]),
        json!([{ "text": "a", "font_features": "=1" }]),
        json!([{ "text": "a", "font_features": { "tnu": 1 } }]),
        json!([{ "text": "a", "font_features": { "tnum": -1 } }]),
        json!([{ "text": "a", "font_features": { "tnum": 1.5 } }]),
        json!([{ "text": "a", "font_features": { "tnum": 4294967296u64 } }]),
        json!([{ "text": "a", "font_features": { "liga\n": 1 } }]),
        json!([{ "text": "a", "stroke": { "align": "inside" } }]),
    ] {
        assert!(!content.is_valid(&invalid), "{invalid}");
        assert!(
            TextContentDelegate::from_json_value(&invalid, "content").is_err(),
            "{invalid}"
        );
    }
}

#[test]
fn paint_shader_uniform_and_clip_props_schemas_match_engine_parsing() {
    let validator = schema_validator();
    let check = |label: &str, composition: Value, valid: bool| {
        assert_eq!(validator.is_valid(&composition), valid, "{label} schema");
        assert_eq!(
            lumen_engine::json::parse_value(&composition).is_ok(),
            valid,
            "{label} engine"
        );
    };

    let stops = json!([[0.0, [255, 0, 0]], { "offset": 1.0, "color": "#0000ff" }]);
    let gradient = |field: &str, value: Value| {
        let mut paint = json!({ "type": "linear", "stops": stops });
        paint[field] = value;
        paint
    };
    let mut paints = vec![
        (json!([255, 0, 0]), true),
        (json!([255, 0, 0, 128]), true),
        (json!("#ff0000"), true),
        (json!("#ff000080"), true),
        (json!("=rgb(255, 0, 0)"), true),
        (
            json!({
                "type": "radial_gradient",
                "units": "user_space",
                "spread": "reflect",
                "interpolation": "linear_srgb",
                "start": [0, 0],
                "end": [1, 0],
                "center": [0.5, 0.5],
                "radius": [0.25, 0.5],
                "angle": 90,
                "stops": [{ "offset": -0.5, "color": [0, 0, 0, 255] }, [1.5, "#ffffff"]]
            }),
            true,
        ),
        (gradient("radius", json!(0.25)), true),
        (json!("red"), false),
        (json!("#ff00"), false),
        (json!([255, 0]), false),
        (json!([256, 0, 0]), false),
        (json!({ "stops": stops }), false),
        (json!({ "type": "linear" }), false),
        (gradient("stops", json!([])), false),
        (
            gradient("stops", json!(vec![json!([0, [0, 0, 0]]); 8])),
            true,
        ),
        (
            gradient("stops", json!(vec![json!([0, [0, 0, 0]]); 9])),
            false,
        ),
        (gradient("colour", json!(1)), false),
        (gradient("type", json!("diamond")), false),
        (gradient("units", json!("user")), false),
        (gradient("spread", json!("mirror")), false),
        (gradient("interpolation", json!("oklab")), false),
        (gradient("radius", json!("big")), false),
        (gradient("radius", json!([1])), false),
        (gradient("start", json!("=vec2(0, 0)")), false),
        (gradient("angle", json!("=frame")), false),
        (gradient("stops", json!([[0]])), false),
        (gradient("stops", json!([[0, [0, 0, 0], 1]])), false),
        (gradient("stops", json!([["0", [0, 0, 0]]])), false),
        (gradient("stops", json!([{ "color": [0, 0, 0] }])), false),
        (
            gradient("stops", json!([{ "offset": 0, "color": "=frame" }])),
            false,
        ),
        (
            gradient(
                "stops",
                json!([{ "offset": 0, "color": [0, 0, 0], "x": 1 }]),
            ),
            false,
        ),
    ];
    for (field, values) in [
        (
            "type",
            &[
                "linear",
                "linear_gradient",
                "radial",
                "radial_gradient",
                "conic",
                "conic_gradient",
            ][..],
        ),
        (
            "units",
            &["object_bounding_box", "user_space", "userSpaceOnUse"],
        ),
        ("spread", &["pad", "repeat", "reflect"]),
        ("interpolation", &["srgb", "linear_srgb", "linear"]),
    ] {
        paints.extend(
            values
                .iter()
                .map(|value| (gradient(field, json!(value)), true)),
        );
    }
    for (paint, valid) in paints {
        let composition =
            composition_with_node(&node_schema("shape"), json!({ "fill_paint": paint }));
        check(&paint.to_string(), composition, valid);
    }

    for (uniforms, valid) in [
        (
            json!({
                "a": 1,
                "b": [1, "=frame"],
                "c": [1, 2, 3],
                "d": [1, 2, 3, "=frame"],
                "e": "=vec2(1, 2)",
                "f": "#ff0000",
                "g": "#ff000080"
            }),
            true,
        ),
        (json!({ "a": true }), false),
        (json!({ "a": [1] }), false),
        (json!({ "a": [1, 2, 3, 4, 5] }), false),
        (json!({ "a": [1, "x"] }), false),
        (json!({ "a": [1, [2]] }), false),
        (json!({ "a": "red" }), false),
        (json!({ "a": "#ff00" }), false),
        (json!([1, 2]), false),
    ] {
        let composition =
            composition_with_node(&node_schema("wgsl_shader"), json!({ "uniforms": uniforms }));
        check(&uniforms.to_string(), composition, valid);
    }

    let demo: Value = serde_json::from_str(include_str!("../../local/demo/clips.json")).unwrap();
    let clip = demo["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .position(|node| node["type"] == "clip")
        .unwrap();
    for (props, valid) in [
        (
            json!({ "title": "=frame", "subtitle": "==x", "accent": [255, 0, 0], "index": 2 }),
            true,
        ),
        (json!({ "accent": "#ff0000", "index": "=frame" }), true),
        (json!({ "index": true }), false),
        (json!({ "index": {} }), false),
        (json!({ "title": null }), false),
        (json!([1]), false),
    ] {
        let mut composition = demo.clone();
        composition["nodes"][clip]["params"]["props"] = props.clone();
        check(&props.to_string(), composition, valid);
    }
}

#[test]
fn dash_pattern_schema_matches_engine_parsing() {
    let validator = schema_validator();
    for (dash, valid) in [
        (json!([]), true),
        (json!([12, 6]), true),
        (json!([0, 4.5, "=frame"]), true),
        (json!(vec![1; 16]), true),
        (json!(vec![1; 17]), false),
        (json!([4, -1]), false),
        (json!(["x"]), false),
        (json!("=frame"), false),
        (json!(4), false),
    ] {
        let composition =
            composition_with_node(&node_schema("path"), json!({ "stroke_dash": dash }));
        assert_eq!(validator.is_valid(&composition), valid, "{dash} schema");
        assert_eq!(
            lumen_engine::json::parse_value(&composition).is_ok(),
            valid,
            "{dash} engine"
        );
    }
}

#[test]
fn compositions_schema_matches_engine_parsing() {
    let validator = schema_validator();
    let demo: Value = serde_json::from_str(include_str!("../../local/demo/clips.json")).unwrap();
    assert!(validator.is_valid(&demo), "clips demo should validate");
    lumen_engine::json::parse_value(&demo).unwrap();

    let with_template = |template: Value| {
        let mut value = demo.clone();
        value["compositions"]["title_card"] = template;
        value
    };
    let template = &demo["compositions"]["title_card"];
    for (patch, valid) in [
        (json!({}), true),
        (
            json!({ "props": { "n": { "type": "number", "default": "x" } } }),
            false,
        ),
        (json!({ "props": { "n": { "type": "matrix" } } }), false),
        (json!({ "functions": {} }), false),
    ] {
        let mut template = template.clone();
        for (key, value) in patch.as_object().unwrap() {
            template[key] = value.clone();
        }
        let composition = with_template(template);
        assert_eq!(validator.is_valid(&composition), valid, "{patch}");
        assert_eq!(
            lumen_engine::json::parse_value(&composition).is_ok(),
            valid,
            "{patch}"
        );
    }
}

#[test]
fn timeline_schema_matches_engine_parsing() {
    let validator = schema_validator();
    let with_timeline = |timeline: Value| {
        let mut value = composition_with_node(&node_schema("exposure"), json!({}));
        value["timeline"] = timeline;
        value
    };

    for (timeline, valid) in [
        (json!({ "duration_frames": 10 }), true),
        (json!({ "fps": 24, "duration_frames": 10.0 }), true),
        (json!({ "fps": 0, "duration_frames": 10 }), false),
        (json!({ "fps": -30, "duration_frames": 10 }), false),
        (json!({ "fps": 30, "duration_frames": 0 }), false),
        (
            json!({ "fps": 30, "duration_frames": 4294967296u64 }),
            false,
        ),
        (json!({ "fpss": 30, "duration_frames": 10 }), false),
        (
            json!({ "durationSeconds": 2, "duration_frames": 10 }),
            false,
        ),
    ] {
        let composition = with_timeline(timeline.clone());
        assert_eq!(validator.is_valid(&composition), valid, "{timeline}");
        assert_eq!(
            lumen_engine::json::parse_value(&composition).is_ok(),
            valid,
            "{timeline}"
        );
    }
}

#[test]
fn version_keys_match_engine_parsing() {
    let validator = schema_validator();

    for (key, version, valid) in [
        ("lumenSchemaVersion", json!("0.1.0"), true),
        ("lumenSchemaVersion", json!(1), false),
        ("lumenSchemaVersion", json!("99.0.0"), false),
        // Retired, so like any unknown root key it can hold anything.
        ("schemaVersion", json!("0.1.0"), true),
        ("schemaVersion", json!("99.0.0"), true),
        ("schemaVersion", json!(1), true),
    ] {
        let mut composition = composition_with_node(&node_schema("exposure"), json!({}));
        composition[key] = version.clone();
        assert_eq!(validator.is_valid(&composition), valid, "{key}: {version}");
        assert_eq!(
            lumen_engine::json::parse_value(&composition).is_ok(),
            valid,
            "{key}: {version}"
        );
    }
}

#[test]
fn document_keys_schema_matches_engine_parsing() {
    let validator = schema_validator();
    let base = composition_with_node(&node_schema("exposure"), json!({}));
    let with_key = |path: &[&str], value: Value| {
        let mut document = base.clone();
        let (last, parents) = path.split_last().unwrap();
        let mut target = &mut document;
        for key in parents {
            target = &mut target[*key];
        }
        target[*last] = value;
        document
    };
    let audio = |clip: Value| {
        let mut with_audio = base.clone();
        let mut full = json!({
            "id": "a", "source_id": "voice", "track_id": "main", "start_ms": 0, "duration_ms": 100
        });
        for (key, value) in clip.as_object().unwrap() {
            if value.is_null() {
                full.as_object_mut().unwrap().remove(key);
            } else {
                full[key] = value.clone();
            }
        }
        with_audio["audio"] = json!({ "tracks": [{ "id": "main" }], "clips": [full] });
        with_audio
    };
    for (label, document, valid) in [
        ("unknown root key", with_key(&["editor"], json!(1)), true),
        (
            "retired root key",
            with_key(&["schemaVersion"], json!("1")),
            true,
        ),
        ("$schema", with_key(&["$schema"], json!("lumen.json")), true),
        (
            "non-string $schema",
            with_key(&["$schema"], json!(1)),
            false,
        ),
        (
            "metadata",
            with_key(&["metadata"], json!({ "name": "Card", "author": 1 })),
            true,
        ),
        (
            "non-object metadata",
            with_key(&["metadata"], json!(1)),
            false,
        ),
        (
            "non-string metadata name",
            with_key(&["metadata"], json!({ "name": 1 })),
            false,
        ),
        (
            "unknown render setting",
            with_key(&["render_settings", "scale"], json!(2)),
            false,
        ),
        (
            "background_color",
            with_key(&["render_settings", "background_color"], json!([0, 0, 0])),
            false,
        ),
        ("whole clip", audio(json!({})), true),
        ("unknown clip key", audio(json!({ "color": "#fff" })), true),
        ("clip name", audio(json!({ "name": "take" })), true),
        ("no start_ms", audio(json!({ "start_ms": null })), false),
        (
            "no duration_ms",
            audio(json!({ "duration_ms": null })),
            false,
        ),
        (
            "fractional time",
            audio(json!({ "source_start_ms": 1.5 })),
            false,
        ),
        ("start_frame", audio(json!({ "start_frame": 1 })), false),
        (
            "duration_frames",
            audio(json!({ "duration_frames": 1 })),
            false,
        ),
        (
            "source_start_seconds",
            audio(json!({ "source_start_seconds": 1 })),
            false,
        ),
    ] {
        assert_eq!(validator.is_valid(&document), valid, "{label} schema");
        assert_eq!(
            lumen_engine::json::parse_value(&document).is_ok(),
            valid,
            "{label} engine"
        );
    }
}

/// Each row is a composition, whether the schema accepts it and whether the engine parses it.
#[test]
fn expression_placement_matches_engine_parsing() {
    let validator = schema_validator();
    let node = |kind: &str, params: Value| composition_with_node(&node_schema(kind), params);
    let with_function = |signature: &str| {
        let mut value = node("shape", json!({ "position": "=vec2(0, 0)" }));
        value["functions"] = json!({ signature: "vec2(0, 0)" });
        value
    };
    let gradient_with_expression_stop = json!({
        "type": "linear",
        "stops": [{ "offset": 0, "color": "=frame" }]
    });
    let rows = [
        (
            "expression missing its `=`",
            node("exposure", json!({ "exposure": "frame" })),
            false,
            false,
        ),
        (
            "expression inside an array",
            node("text", json!({ "position": ["=frame", 0] })),
            false,
            false,
        ),
        (
            "expression for a literal-only enum",
            node("shape", json!({ "geometry_kind": "='ellipse'" })),
            false,
            false,
        ),
        (
            "expression as a paint",
            node("shape", json!({ "fill_paint": "=#ff0000" })),
            true,
            true,
        ),
        (
            "expression in a gradient stop",
            node("text", json!({ "color": gradient_with_expression_stop })),
            false,
            false,
        ),
        (
            "function signature",
            with_function("rise(start)"),
            true,
            true,
        ),
        (
            "function signature with spaces",
            with_function("pop( )"),
            true,
            true,
        ),
        (
            "function signature without `)`",
            with_function("rise(start"),
            false,
            false,
        ),
    ];
    for (label, composition, schema_accepts, engine_accepts) in rows {
        assert_eq!(
            validator.is_valid(&composition),
            schema_accepts,
            "{label} schema"
        );
        assert_eq!(
            lumen_engine::json::parse_value(&composition).is_ok(),
            engine_accepts,
            "{label} engine"
        );
    }
}

/// Each row is a node param set to a value outside its editor slider range, whether the schema
/// accepts it and whether the engine parses it. The engine clamps or floors these when it
/// renders, so the schema must not reject them.
#[test]
fn editor_hint_ranges_are_not_enforced() {
    let schema = schema();
    let opacity = &schema["properties"]["nodes"]["items"]["oneOf"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["properties"]["type"]["const"] == "opacity")
        .unwrap()["properties"]["params"]["properties"]["opacity"];
    assert_eq!(
        opacity["x-lumen-constraints"],
        json!({ "min": 0.0, "max": 1.0, "step": 0.01 })
    );
    assert!(opacity["anyOf"][0].get("minimum").is_none());

    let validator = jsonschema::validator_for(&schema).unwrap();
    let rows = [
        ("opacity above 1", "opacity", json!({ "opacity": 2 }), true),
        ("opacity below 0", "opacity", json!({ "opacity": -1 }), true),
        ("crop width below 0", "crop", json!({ "width": -5 }), true),
        ("resize width of 0", "resize", json!({ "width": 0 }), true),
        ("levels gamma of 0", "levels", json!({ "gamma": 0 }), true),
        (
            "text font size below 1",
            "text",
            json!({ "font_size": 0.5 }),
            true,
        ),
        (
            "text font weight above 1000",
            "text",
            json!({ "font_weight": 2000 }),
            true,
        ),
        (
            "shape motion blur above 360",
            "shape",
            json!({ "motion_blur": 720 }),
            true,
        ),
        (
            "switch layer below 0",
            "switch",
            json!({ "selected_layer": -1 }),
            true,
        ),
    ];
    for (label, kind, params, engine_accepts) in rows {
        let composition = composition_with_node(&node_schema(kind), params);
        assert!(validator.is_valid(&composition), "{label} schema");
        assert_eq!(
            lumen_engine::json::parse_value(&composition).is_ok(),
            engine_accepts,
            "{label} engine"
        );
    }
}

/// Each row is a node type, a param, its lowest literal and a literal below it the engine refuses.
#[test]
fn enforced_ranges_match_engine_parsing() {
    let validator = schema_validator();
    let rows = [
        ("background", "width", json!(0), json!(-1)),
        ("background", "height", json!(0), json!(-1)),
        ("clip", "duration", json!(-1), json!(-2)),
    ];
    for (kind, param, lowest, below) in rows {
        let composition =
            |value: &Value| composition_with_node(&node_schema(kind), json!({ param: value }));
        assert!(
            validator.is_valid(&composition(&lowest)),
            "{kind}.{param} {lowest} schema"
        );
        assert!(
            !validator.is_valid(&composition(&below)),
            "{kind}.{param} {below} schema"
        );
        assert!(
            lumen_engine::json::parse_value(&composition(&below)).is_err(),
            "{kind}.{param} {below} engine"
        );
    }
}

/// Each row is a composition, whether the schema accepts it and whether the engine parses it.
#[test]
fn ids_and_audio_clips_match_engine_parsing() {
    let validator = schema_validator();
    let base = composition_with_node(&node_schema("exposure"), json!({}));
    let with_id = |id: i64| {
        let mut value = base.clone();
        value["nodes"][0]["id"] = json!(id);
        value
    };
    let with_connection_node = |field: &str, id: i64| {
        let mut value = base.clone();
        value["connections"][0][field] = json!(id);
        value
    };
    let with_clip = |clip: Value| {
        let mut value = base.clone();
        value["audio"] = json!({
            "tracks": [{ "id": "track" }],
            "clips": [{ "id": "clip", "source_id": "voice", "track_id": "track" }]
        });
        for (key, field) in clip.as_object().unwrap() {
            value["audio"]["clips"][0][key] = field.clone();
        }
        value
    };
    let rows = [
        ("negative node id", with_id(-1), false),
        (
            "negative connection source",
            with_connection_node("from_node", -1),
            false,
        ),
        (
            "negative connection target",
            with_connection_node("to_node", -1),
            false,
        ),
        ("audio clip without a time", with_clip(json!({})), false),
        (
            "audio clip without a start",
            with_clip(json!({ "duration_ms": 500 })),
            false,
        ),
        (
            "audio clip without a duration",
            with_clip(json!({ "start_ms": 0 })),
            false,
        ),
        (
            "audio clip in milliseconds",
            with_clip(json!({ "start_ms": 0, "duration_ms": 500 })),
            true,
        ),
        (
            "audio clip in frames",
            with_clip(json!({ "start_frame": 0, "duration_frames": 15 })),
            false,
        ),
        (
            "audio clip starting in frames and lasting in milliseconds",
            with_clip(json!({ "start_frame": 3, "duration_ms": 500 })),
            false,
        ),
    ];
    for (label, composition, accepted) in rows {
        assert_eq!(validator.is_valid(&composition), accepted, "{label} schema");
        assert_eq!(
            lumen_engine::json::parse_value(&composition).is_ok(),
            accepted,
            "{label} engine"
        );
    }
}
