use std::sync::{Arc, RwLock};

use lumen::{
    AssetCache, Composition, Connection, Graph, InputPort, InterpolationMode, KeyframeTrack,
    LumenError, NodeId, NodeKind, NullMediaStore, OutputPort, RenderContext, RenderSettings,
    RuntimeCapabilityProfile, SurfacePool, TimelineSettings, TrackId,
    animation::PropertyPath,
    node::{Node, PropertyValue, media_output::MediaOutput, transform::Transform},
};

fn expression_context() -> (Composition, RenderContext, NodeId) {
    let mut graph = Graph::new();
    let transform = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::Transform(Transform::default()),
    ));
    let output = graph.add_node(Node::new(NodeId(0), NodeKind::MediaOutput(MediaOutput)));
    graph
        .connect(Connection {
            from_node: transform,
            from_port: OutputPort::default(),
            to_node: output,
            to_port: InputPort::named("source"),
        })
        .expect("valid transform->output connection");

    let composition = Composition::new(
        graph,
        TimelineSettings {
            fps: 30.0,
            duration_frames: 120,
        },
        RenderSettings {
            width: 1920,
            height: 1080,
            background_color: [0, 0, 0, 0],
        },
    );
    let context = RenderContext::new(
        &composition,
        Arc::new(SurfacePool::new()),
        Arc::new(RwLock::new(AssetCache::new())),
        Arc::new(NullMediaStore),
        RuntimeCapabilityProfile::cpu_only(),
    );
    (composition, context, transform)
}

fn expect_number(value: lumen::ExpressionValue) -> f64 {
    match value {
        lumen::ExpressionValue::Number(number) => number,
        other => panic!("expected numeric expression value, got {other:?}"),
    }
}

#[test]
fn globals_resolve_from_render_context() {
    let (_composition, mut context, _transform) = expression_context();
    context.request.frame = 30;

    let frame = expect_number(
        lumen::Expression::parse("frame")
            .unwrap()
            .evaluate(&context)
            .unwrap(),
    );
    let time = expect_number(
        lumen::Expression::parse("time")
            .unwrap()
            .evaluate(&context)
            .unwrap(),
    );
    let fps = expect_number(
        lumen::Expression::parse("fps")
            .unwrap()
            .evaluate(&context)
            .unwrap(),
    );
    let width = expect_number(
        lumen::Expression::parse("width")
            .unwrap()
            .evaluate(&context)
            .unwrap(),
    );
    let height = expect_number(
        lumen::Expression::parse("height")
            .unwrap()
            .evaluate(&context)
            .unwrap(),
    );

    assert_eq!(frame, 30.0);
    assert!((time - 1.0).abs() < 1e-6);
    assert_eq!(fps, 30.0);
    assert_eq!(width, 1920.0);
    assert_eq!(height, 1080.0);
}

#[test]
fn math_builtins_evaluate_expected_results() {
    let (_composition, context, _transform) = expression_context();
    let cases = [
        ("sin(0)", 0.0),
        ("cos(0)", 1.0),
        ("lerp(10, 20, 0.5)", 15.0),
        ("clamp(10, 0, 5)", 5.0),
        ("smoothstep(0, 1, 0.5)", 0.5),
        ("pow(2, 3)", 8.0),
        ("mod(10, 3)", 1.0),
        ("fract(2.75)", 0.75),
        ("floor(2.9)", 2.0),
        ("ceil(2.1)", 3.0),
        ("round(2.5)", 3.0),
        ("abs(-9)", 9.0),
        ("min(5, 2, 9)", 2.0),
        ("max(5, 2, 9)", 9.0),
    ];

    for (source, expected) in cases {
        let expression = lumen::Expression::parse(source).expect("expression should parse");
        let value = expect_number(
            expression
                .evaluate(&context)
                .expect("expression should evaluate"),
        );
        assert!(
            (value - expected).abs() < 1e-6,
            "{source} expected {expected}, got {value}"
        );
    }
}

#[test]
fn text_builtins_handle_case_conversion() {
    let (_composition, context, _transform) = expression_context();

    let uppercase = lumen::Expression::parse("uppercase('hello world')")
        .unwrap()
        .evaluate(&context)
        .unwrap();
    let lowercase = lumen::Expression::parse("lowercase('HeLLo')")
        .unwrap()
        .evaluate(&context)
        .unwrap();

    assert_eq!(
        uppercase,
        lumen::ExpressionValue::String("HELLO WORLD".to_string())
    );
    assert_eq!(
        lowercase,
        lumen::ExpressionValue::String("hello".to_string())
    );
}

#[test]
fn undefined_variable_reports_node_and_property_context() {
    let (composition, context, transform) = expression_context();
    let expression =
        lumen::Expression::parse("unknown_symbol + 1").expect("expression should parse");
    let error = expression
        .evaluate_with_context(
            &context,
            Some(&composition),
            Some(transform),
            Some("translate_x".to_string()),
        )
        .expect_err("undefined variable should fail evaluation");

    assert!(matches!(
        error,
        LumenError::Expression(lumen::error::ExpressionError::UndefinedVariable {
            node_id: Some(id),
            property_path: Some(path),
            name,
        }) if id == transform && path == "translate_x" && name == "unknown_symbol"
    ));
}

#[test]
fn precedence_and_nested_function_calls_work() {
    let (_composition, context, _transform) = expression_context();

    let precedence = expect_number(
        lumen::Expression::parse("1 + 2 * 3")
            .unwrap()
            .evaluate(&context)
            .unwrap(),
    );
    let nested = expect_number(
        lumen::Expression::parse("clamp(lerp(0, 100, 0.5) + pow(2, 3), 0, 60)")
            .unwrap()
            .evaluate(&context)
            .unwrap(),
    );

    assert_eq!(precedence, 7.0);
    assert_eq!(nested, 58.0);
}

#[test]
fn expression_precedence_overrides_keyframes_and_static_values() {
    let (mut composition, context, transform) = expression_context();

    let mut track = KeyframeTrack::new(
        TrackId(1),
        transform,
        PropertyPath::new("translate_x"),
        lumen::AnimatableType::Float,
    );
    track.set_key(0, PropertyValue::Float(0.0), InterpolationMode::Linear);
    track.set_key(60, PropertyValue::Float(100.0), InterpolationMode::Linear);
    composition.add_track(track);
    composition.set_expression(
        transform,
        "translate_x",
        lumen::Expression::parse("42").expect("expression should parse"),
    );

    let sampled = composition
        .sample_property(transform, "translate_x", 30, &context)
        .expect("sampled property should resolve");
    assert_eq!(sampled, PropertyValue::Float(42.0));
}
