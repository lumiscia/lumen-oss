use std::sync::{Arc, RwLock};

use lumen::{
    AnimatableType, AssetCache, Composition, Connection, Extrapolation, Graph, InputPort,
    InterpolationMode, Keyframe, KeyframeTrack, NodeId, NodeKind, NullMediaStore, OutputPort,
    RasterFrame, RenderContext, RenderSettings, RuntimeCapabilityProfile, SurfacePool,
    TimelineSettings, TrackId,
    animation::PropertyPath,
    node::{
        Node, PropertyValue, media_output::MediaOutput, solid_color::SolidColor,
        transform::Transform,
    },
};

fn base_composition() -> (Composition, NodeId) {
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
        .expect("valid setup connection");

    (
        Composition::new(
            graph,
            TimelineSettings {
                fps: 30.0,
                duration_frames: 120,
            },
            RenderSettings {
                width: 64,
                height: 64,
                background_color: [0, 0, 0, 0],
            },
        ),
        transform,
    )
}

#[test]
fn linear_interpolation_samples_midpoint() {
    let mut track = KeyframeTrack::new(
        TrackId(1),
        NodeId(1),
        PropertyPath::new("translate_x"),
        AnimatableType::Float,
    );
    track.set_key(0, PropertyValue::Float(0.0), InterpolationMode::Linear);
    track.set_key(60, PropertyValue::Float(100.0), InterpolationMode::Linear);

    let sample = track.sample(30).expect("sample should succeed");
    assert_eq!(sample, PropertyValue::Float(50.0));
}

#[test]
fn step_interpolation_holds_previous_boolean_value() {
    let mut track = KeyframeTrack::new(
        TrackId(2),
        NodeId(1),
        PropertyPath::new("visible"),
        AnimatableType::Boolean,
    );
    track.set_key(0, PropertyValue::Bool(false), InterpolationMode::Step);
    track.set_key(60, PropertyValue::Bool(true), InterpolationMode::Step);

    let sample = track.sample(30).expect("sample should succeed");
    assert_eq!(sample, PropertyValue::Bool(false));
}

#[test]
fn hold_extrapolation_before_and_after_key_range() {
    let mut track = KeyframeTrack::new(
        TrackId(3),
        NodeId(1),
        PropertyPath::new("translate_y"),
        AnimatableType::Float,
    );
    track.before_extrapolation = Extrapolation::Hold;
    track.after_extrapolation = Extrapolation::Hold;
    track.set_key(10, PropertyValue::Float(10.0), InterpolationMode::Linear);
    track.set_key(20, PropertyValue::Float(20.0), InterpolationMode::Linear);

    assert_eq!(
        track.sample(0).expect("before range"),
        PropertyValue::Float(10.0)
    );
    assert_eq!(
        track.sample(30).expect("after range"),
        PropertyValue::Float(20.0)
    );
}

#[test]
fn single_key_track_returns_same_value_for_all_frames() {
    let mut track = KeyframeTrack::new(
        TrackId(4),
        NodeId(1),
        PropertyPath::new("opacity"),
        AnimatableType::Float,
    );
    track.set_key(15, PropertyValue::Float(0.25), InterpolationMode::Linear);

    assert_eq!(
        track.sample(0).expect("sample 0"),
        PropertyValue::Float(0.25)
    );
    assert_eq!(
        track.sample(15).expect("sample 15"),
        PropertyValue::Float(0.25)
    );
    assert_eq!(
        track.sample(90).expect("sample 90"),
        PropertyValue::Float(0.25)
    );
}

#[test]
fn empty_keys_duplicate_times_and_invalid_targets_are_rejected_by_composition_validation() {
    let (mut composition, transform) = base_composition();

    let mut empty_track = KeyframeTrack::new(
        TrackId(10),
        transform,
        PropertyPath::new("translate_x"),
        AnimatableType::Float,
    );
    empty_track.keys.clear();
    composition.add_track(empty_track);

    let mut duplicate_track = KeyframeTrack::new(
        TrackId(11),
        transform,
        PropertyPath::new("translate_y"),
        AnimatableType::Float,
    );
    duplicate_track.keys = vec![
        Keyframe {
            time_frame: 12,
            value: PropertyValue::Float(1.0),
            interpolation: InterpolationMode::Linear,
        },
        Keyframe {
            time_frame: 12,
            value: PropertyValue::Float(2.0),
            interpolation: InterpolationMode::Linear,
        },
    ];
    composition.add_track(duplicate_track);

    let mut missing_node_track = KeyframeTrack::new(
        TrackId(12),
        NodeId(999),
        PropertyPath::new("translate_x"),
        AnimatableType::Float,
    );
    missing_node_track.set_key(0, PropertyValue::Float(1.0), InterpolationMode::Linear);
    composition.add_track(missing_node_track);

    let mut invalid_path_track = KeyframeTrack::new(
        TrackId(13),
        transform,
        PropertyPath::new("does_not_exist"),
        AnimatableType::Float,
    );
    invalid_path_track.set_key(0, PropertyValue::Float(1.0), InterpolationMode::Linear);
    composition.add_track(invalid_path_track);

    let errors = composition
        .validate_structure()
        .expect_err("composition validation should fail for invalid tracks");
    assert!(errors.len() >= 4);

    let validation_with_profile = composition.validate(&RuntimeCapabilityProfile::cpu_only());
    assert!(validation_with_profile.is_err());
}

#[test]
fn keyframed_transform_affects_rendered_frame_positions() {
    let mut graph = Graph::new();
    let solid = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::SolidColor(SolidColor {
            color: [255, 255, 255, 255],
            width: Some(4),
            height: Some(1),
        }),
    ));
    let transform = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::Transform(Transform::default()),
    ));
    let output = graph.add_node(Node::new(NodeId(0), NodeKind::MediaOutput(MediaOutput)));
    graph
        .connect(Connection {
            from_node: solid,
            from_port: OutputPort::default(),
            to_node: transform,
            to_port: InputPort::named("source"),
        })
        .expect("valid solid->transform connection");
    graph
        .connect(Connection {
            from_node: transform,
            from_port: OutputPort::default(),
            to_node: output,
            to_port: InputPort::named("source"),
        })
        .expect("valid transform->output connection");

    let mut composition = Composition::new(
        graph,
        TimelineSettings {
            fps: 30.0,
            duration_frames: 31,
        },
        RenderSettings {
            width: 4,
            height: 1,
            background_color: [0, 0, 0, 0],
        },
    );

    let mut track = KeyframeTrack::new(
        TrackId(20),
        transform,
        PropertyPath::new("translate_x"),
        AnimatableType::Float,
    );
    track.set_key(0, PropertyValue::Float(0.0), InterpolationMode::Linear);
    track.set_key(30, PropertyValue::Float(2.0), InterpolationMode::Linear);
    composition.add_track(track);

    let mut context = RenderContext::new(
        &composition,
        Arc::new(SurfacePool::new()),
        Arc::new(RwLock::new(AssetCache::new())),
        Arc::new(NullMediaStore),
        RuntimeCapabilityProfile::cpu_only(),
    );

    let start = composition
        .render_frame(0, &mut context)
        .expect("frame 0 render");
    let end = composition
        .render_frame(30, &mut context)
        .expect("frame 30 render");

    let start_bytes = match start {
        RasterFrame::Bitmap(bytes, _, _) => bytes,
        RasterFrame::Surface(_) => panic!("expected bitmap"),
    };
    let end_bytes = match end {
        RasterFrame::Bitmap(bytes, _, _) => bytes,
        RasterFrame::Surface(_) => panic!("expected bitmap"),
    };

    assert_eq!(&start_bytes[0..4], &[255, 255, 255, 255]);
    assert_eq!(&end_bytes[0..4], &[0, 0, 0, 0]);
    assert_eq!(&end_bytes[8..12], &[255, 255, 255, 255]);
}
