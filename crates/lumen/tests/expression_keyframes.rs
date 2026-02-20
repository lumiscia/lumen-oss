use lumen::{
    Canvas, Clip, ClipAnimation, ClipContent, ColorRgba, Easing, Layer, LayerItem, Project, Scalar,
    ScalarKeyframe, TextClip, Timeline, Transform, compile::CompileError, compile_project,
    time::Rational,
};

fn text_clip(id: &str, transform: Transform, animation: ClipAnimation) -> Clip {
    Clip {
        id: id.to_string(),
        start_frame: 0,
        duration_frames: 30,
        opacity: 1.0,
        transform,
        animation,
        mask: None,
        content: ClipContent::Text(TextClip {
            text: id.to_string(),
            font_size: 20.0,
            color: ColorRgba(255, 255, 255, 255),
            align: Default::default(),
        }),
    }
}

fn base_project(items: Vec<LayerItem>) -> Project {
    Project {
        canvas: Canvas {
            width: 400,
            height: 240,
            background: ColorRgba(0, 0, 0, 255),
        },
        timeline: Timeline {
            fps: Rational::new(30, 1).expect("fps"),
            total_frames: 30,
        },
        sources: vec![],
        layers: vec![Layer {
            id: "layer_a".to_string(),
            z_index: 0,
            items,
        }],
        audio: Default::default(),
    }
}

#[test]
fn compiles_complex_expressions_in_transforms_and_keyframes() {
    let anchor = text_clip(
        "clip_anchor",
        Transform {
            x: Scalar::Literal(20.0),
            y: Scalar::Literal(0.0),
            width: Some(Scalar::Literal(80.0)),
            height: None,
            rotation_degrees: 0.0,
        },
        Default::default(),
    );

    let target = text_clip(
        "clip_target",
        Transform {
            x: Scalar::Expr("(clip_anchor.x + clip_anchor.width) - 5".to_string()),
            y: Scalar::Literal(0.0),
            width: Some(Scalar::Literal(40.0)),
            height: None,
            rotation_degrees: 0.0,
        },
        ClipAnimation {
            width: vec![ScalarKeyframe {
                frame: 0,
                value: Scalar::Expr("((canvas.width - clip_anchor.x) / 2) + 5".to_string()),
                duration_frames: 0,
                easing: Easing::Linear,
            }],
            ..Default::default()
        },
    );

    let project = base_project(vec![LayerItem::Clip(anchor), LayerItem::Clip(target)]);
    let compiled = compile_project(&project).expect("compile");
    let frame_ops = compiled.operation_indices_for_frame(0).expect("frame ops");
    let op = compiled.operation(frame_ops[1]).expect("target op");

    let transform = op.resolved_transform(0);
    assert_eq!(transform.x, 95.0);
    assert_eq!(transform.width, Some(195.0));
}

#[test]
fn rejects_unknown_refs_in_keyframe_expressions() {
    let clip = text_clip(
        "clip_a",
        Transform {
            x: Scalar::Literal(0.0),
            y: Scalar::Literal(0.0),
            width: Some(Scalar::Literal(100.0)),
            height: None,
            rotation_degrees: 0.0,
        },
        ClipAnimation {
            width: vec![ScalarKeyframe {
                frame: 0,
                value: Scalar::Expr("ghost.width + 10".to_string()),
                duration_frames: 0,
                easing: Easing::Linear,
            }],
            ..Default::default()
        },
    );

    let project = base_project(vec![LayerItem::Clip(clip)]);
    let error = compile_project(&project).expect_err("must fail");
    assert!(matches!(error, CompileError::ExprError { .. }));
}
