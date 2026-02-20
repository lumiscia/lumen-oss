use lumen::{
    Canvas, Clip, ClipAnimation, ClipContent, ColorRgba, Easing, ExprEvalCtx, ExprProp, Layer,
    LayerItem, LayoutClip, LayoutNode, LayoutNodeKind, LayoutNodeStyle, Project, Scalar,
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

struct StubExprCtx;

impl ExprEvalCtx for StubExprCtx {
    fn resolve(&self, target: &str, property: ExprProp) -> Option<f32> {
        if target == "chat_msg_row_s0_m0" && property == ExprProp::Height {
            return Some(36.0);
        }
        None
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

#[test]
fn resolves_layout_node_refs_in_keyframe_expressions_at_runtime() {
    let layout = Clip {
        id: "layout_source".to_string(),
        start_frame: 0,
        duration_frames: 30,
        opacity: 1.0,
        transform: Transform {
            x: Scalar::Literal(0.0),
            y: Scalar::Literal(0.0),
            width: Some(Scalar::Literal(200.0)),
            height: Some(Scalar::Literal(120.0)),
            rotation_degrees: 0.0,
        },
        animation: Default::default(),
        mask: None,
        content: ClipContent::Layout(LayoutClip {
            root: LayoutNode {
                id: None,
                style: Default::default(),
                kind: LayoutNodeKind::Container {
                    children: vec![LayoutNode {
                        id: Some("chat_msg_row_s0_m0".to_string()),
                        style: LayoutNodeStyle {
                            width: Some(Scalar::Literal(160.0)),
                            height: Some(Scalar::Literal(24.0)),
                            ..Default::default()
                        },
                        kind: LayoutNodeKind::Container { children: vec![] },
                    }],
                },
            },
        }),
    };

    let animated_mask = text_clip(
        "animated_mask",
        Transform {
            x: Scalar::Literal(0.0),
            y: Scalar::Literal(0.0),
            width: Some(Scalar::Literal(160.0)),
            height: Some(Scalar::Literal(10.0)),
            rotation_degrees: 0.0,
        },
        ClipAnimation {
            height: vec![ScalarKeyframe {
                frame: 0,
                value: Scalar::Expr("chat_msg_row_s0_m0.height + 12".to_string()),
                duration_frames: 0,
                easing: Easing::Linear,
            }],
            ..Default::default()
        },
    );

    let project = base_project(vec![LayerItem::Clip(layout), LayerItem::Clip(animated_mask)]);
    let compiled = compile_project(&project).expect("compile");
    let frame_ops = compiled.operation_indices_for_frame(0).expect("frame ops");
    let op = compiled.operation(frame_ops[1]).expect("animated op");

    let resolved = op.resolved_transform_with_ctx(0, &StubExprCtx);
    assert_eq!(resolved.height, Some(48.0));
}
