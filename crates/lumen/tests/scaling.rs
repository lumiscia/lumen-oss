//! Scale-factor tests for compile_project_with_scale.

use lumen::{
    Canvas, Clip, ClipAnimation, ClipContent, ClipGroup, ClipShadow, ColorRgba, Easing,
    GroupTransform, ImageClip, Layer, LayerItem, Project, Scalar, ScalarKeyframe, Shape, ShapeClip,
    Source, SourceKind, SourceMediaType, TextClip, Timeline, Transform, compile_project_with_scale,
    time::Rational,
};

fn base_project() -> Project {
    Project {
        canvas: Canvas {
            width: 200,
            height: 100,
            background: ColorRgba(0, 0, 0, 255),
        },
        timeline: Timeline {
            fps: Rational::new(30, 1).unwrap(),
            total_frames: 30,
        },
        sources: vec![],
        layers: vec![Layer {
            id: "layer_1".into(),
            z_index: 0,
            items: vec![],
        }],
        audio: Default::default(),
    }
}

fn solid_clip(id: &str, x: f32, y: f32, w: f32, h: f32) -> Clip {
    Clip {
        id: id.into(),
        start_frame: 0,
        duration_frames: 30,
        opacity: 1.0,
        transform: Transform {
            x: Scalar::Literal(x),
            y: Scalar::Literal(y),
            width: Some(Scalar::Literal(w)),
            height: Some(Scalar::Literal(h)),
            rotation_degrees: 0.0,
        },
        animation: Default::default(),
        shadow: None,
        mask: None,
        content: ClipContent::Solid {
            color: ColorRgba(255, 255, 255, 255),
        },
    }
}

// ===========================================================================
// Canvas scaling
// ===========================================================================

#[test]
fn scales_canvas_dimensions() {
    let p = base_project();
    let compiled = compile_project_with_scale(&p, 0.5).unwrap();
    assert_eq!(compiled.canvas.width, 100);
    assert_eq!(compiled.canvas.height, 50);
}

#[test]
fn scale_of_1_is_identity() {
    let mut p = base_project();
    p.layers[0].items = vec![LayerItem::Clip(solid_clip("c", 10.0, 20.0, 30.0, 40.0))];
    let compiled = compile_project_with_scale(&p, 1.0).unwrap();
    assert_eq!(compiled.canvas.width, 200);
    assert_eq!(compiled.canvas.height, 100);
    let op = compiled.operation(0).unwrap();
    let t = op.resolved_transform(0);
    assert_eq!(t.x, 10.0);
    assert_eq!(t.y, 20.0);
    assert_eq!(t.width, Some(30.0));
    assert_eq!(t.height, Some(40.0));
}

// ===========================================================================
// Transform scaling
// ===========================================================================

#[test]
fn scales_clip_transform_by_half() {
    let mut p = base_project();
    p.layers[0].items = vec![LayerItem::Clip(solid_clip("c", 100.0, 200.0, 300.0, 400.0))];
    let compiled = compile_project_with_scale(&p, 0.5).unwrap();
    let op = compiled.operation(0).unwrap();
    let t = op.resolved_transform(0);
    assert_eq!(t.x, 50.0);
    assert_eq!(t.y, 100.0);
    assert_eq!(t.width, Some(150.0));
    assert_eq!(t.height, Some(200.0));
}

#[test]
fn does_not_scale_rotation() {
    let mut p = base_project();
    let mut clip = solid_clip("c", 0.0, 0.0, 100.0, 100.0);
    clip.transform.rotation_degrees = 45.0;
    p.layers[0].items = vec![LayerItem::Clip(clip)];
    let compiled = compile_project_with_scale(&p, 0.5).unwrap();
    let op = compiled.operation(0).unwrap();
    assert_eq!(op.resolved_transform(0).rotation_degrees, 45.0);
}

#[test]
fn does_not_scale_opacity() {
    let mut p = base_project();
    let mut clip = solid_clip("c", 0.0, 0.0, 100.0, 100.0);
    clip.opacity = 0.7;
    p.layers[0].items = vec![LayerItem::Clip(clip)];
    let compiled = compile_project_with_scale(&p, 0.5).unwrap();
    let op = compiled.operation(0).unwrap();
    assert!((op.opacity - 0.7).abs() < 0.001);
}

// ===========================================================================
// Keyframe value scaling
// ===========================================================================

#[test]
fn scales_position_keyframe_values() {
    let mut p = base_project();
    let clip = Clip {
        id: "c".into(),
        start_frame: 0,
        duration_frames: 30,
        opacity: 1.0,
        transform: Transform {
            x: Scalar::Literal(0.0),
            y: Scalar::Literal(0.0),
            width: None,
            height: None,
            rotation_degrees: 0.0,
        },
        animation: ClipAnimation {
            x: vec![ScalarKeyframe {
                frame: 0,
                value: Scalar::Literal(200.0),
                duration_frames: 0,
                easing: Easing::Linear,
            }],
            ..Default::default()
        },
        shadow: None,
        mask: None,
        content: ClipContent::Solid {
            color: ColorRgba(255, 255, 255, 255),
        },
    };
    p.layers[0].items = vec![LayerItem::Clip(clip)];
    let compiled = compile_project_with_scale(&p, 0.5).unwrap();
    let op = compiled.operation(0).unwrap();
    assert_eq!(op.resolved_transform(0).x, 100.0); // 200 * 0.5
}

#[test]
fn does_not_scale_opacity_keyframes() {
    let mut p = base_project();
    let clip = Clip {
        id: "c".into(),
        start_frame: 0,
        duration_frames: 30,
        opacity: 1.0,
        transform: Transform {
            x: Scalar::Literal(0.0),
            y: Scalar::Literal(0.0),
            width: None,
            height: None,
            rotation_degrees: 0.0,
        },
        animation: ClipAnimation {
            opacity: vec![ScalarKeyframe {
                frame: 0,
                value: Scalar::Literal(0.5),
                duration_frames: 0,
                easing: Easing::Linear,
            }],
            ..Default::default()
        },
        shadow: None,
        mask: None,
        content: ClipContent::Solid {
            color: ColorRgba(255, 255, 255, 255),
        },
    };
    p.layers[0].items = vec![LayerItem::Clip(clip)];
    let compiled = compile_project_with_scale(&p, 0.5).unwrap();
    let op = compiled.operation(0).unwrap();
    assert!((op.resolved_opacity(0) - 0.5).abs() < 0.01);
}

// ===========================================================================
// Content scaling
// ===========================================================================

#[test]
fn scales_shape_corner_radius() {
    let mut p = base_project();
    p.layers[0].items = vec![LayerItem::Clip(Clip {
        id: "shape".into(),
        start_frame: 0,
        duration_frames: 30,
        opacity: 1.0,
        transform: Transform {
            x: Scalar::Literal(0.0),
            y: Scalar::Literal(0.0),
            width: Some(Scalar::Literal(100.0)),
            height: Some(Scalar::Literal(100.0)),
            rotation_degrees: 0.0,
        },
        animation: Default::default(),
        shadow: None,
        mask: None,
        content: ClipContent::Shape(ShapeClip {
            shape: Shape::Rectangle {
                fill: ColorRgba(255, 0, 0, 255),
                radius: 20.0,
            },
        }),
    })];
    let compiled = compile_project_with_scale(&p, 0.5).unwrap();
    let op = compiled.operation(0).unwrap();
    match &op.kind {
        lumen::compile::CompiledOperationKind::Shape(shape) => match shape.shape {
            Shape::Rectangle { radius, .. } => assert_eq!(radius, 10.0),
            _ => panic!("expected rectangle"),
        },
        _ => panic!("expected shape"),
    }
}

#[test]
fn scales_text_font_size() {
    let mut p = base_project();
    p.layers[0].items = vec![LayerItem::Clip(Clip {
        id: "text".into(),
        start_frame: 0,
        duration_frames: 30,
        opacity: 1.0,
        transform: Transform {
            x: Scalar::Literal(0.0),
            y: Scalar::Literal(0.0),
            width: None,
            height: None,
            rotation_degrees: 0.0,
        },
        animation: Default::default(),
        shadow: None,
        mask: None,
        content: ClipContent::Text(TextClip {
            text: "hello".into(),
            font_size: 48.0,
            color: ColorRgba(255, 255, 255, 255),
            align: Default::default(),
        }),
    })];
    let compiled = compile_project_with_scale(&p, 0.5).unwrap();
    let op = compiled.operation(0).unwrap();
    match &op.kind {
        lumen::compile::CompiledOperationKind::Text(text) => {
            assert_eq!(text.font_size, 24.0);
        }
        _ => panic!("expected text"),
    }
}

#[test]
fn scales_image_corner_radius() {
    let mut p = base_project();
    p.sources = vec![Source {
        id: "img".into(),
        kind: SourceKind::File {
            media: SourceMediaType::Image,
            path: "test.png".into(),
        },
    }];
    p.layers[0].items = vec![LayerItem::Clip(Clip {
        id: "img_clip".into(),
        start_frame: 0,
        duration_frames: 30,
        opacity: 1.0,
        transform: Transform {
            x: Scalar::Literal(0.0),
            y: Scalar::Literal(0.0),
            width: Some(Scalar::Literal(100.0)),
            height: Some(Scalar::Literal(100.0)),
            rotation_degrees: 0.0,
        },
        animation: Default::default(),
        shadow: None,
        mask: None,
        content: ClipContent::Image(ImageClip {
            source: "img".into(),
            fit: Default::default(),
            corner_radius: 16.0,
        }),
    })];
    let compiled = compile_project_with_scale(&p, 0.5).unwrap();
    let op = compiled.operation(0).unwrap();
    match &op.kind {
        lumen::compile::CompiledOperationKind::Image(img) => {
            assert_eq!(img.corner_radius, 8.0);
        }
        _ => panic!("expected image"),
    }
}

#[test]
fn scales_clip_shadow_offsets_and_blur_sigma() {
    let mut p = base_project();
    let mut clip = solid_clip("shadow", 0.0, 0.0, 100.0, 100.0);
    clip.shadow = Some(ClipShadow {
        offset_x: 16.0,
        offset_y: -6.0,
        blur_sigma: 12.0,
        color: ColorRgba(10, 20, 30, 200),
    });
    p.layers[0].items = vec![LayerItem::Clip(clip)];

    let compiled = compile_project_with_scale(&p, 0.5).unwrap();
    let op = compiled.operation(0).unwrap();
    let shadow = op.shadow.expect("shadow");
    assert_eq!(shadow.offset_x, 8.0);
    assert_eq!(shadow.offset_y, -3.0);
    assert_eq!(shadow.blur_sigma, 6.0);
    assert_eq!(shadow.color, ColorRgba(10, 20, 30, 200));
}

#[test]
fn scales_group_shadow_offsets_and_blur_sigma() {
    let mut p = base_project();
    p.layers[0].items = vec![LayerItem::Group(ClipGroup {
        id: "group".into(),
        opacity: 1.0,
        transform: GroupTransform {
            x: Scalar::Literal(0.0),
            y: Scalar::Literal(0.0),
            rotation_degrees: 0.0,
        },
        items: vec![LayerItem::Clip(solid_clip("inner", 0.0, 0.0, 100.0, 100.0))],
        shadow: Some(ClipShadow {
            offset_x: 20.0,
            offset_y: -8.0,
            blur_sigma: 10.0,
            color: ColorRgba(0, 0, 0, 220),
        }),
        mask: None,
    })];

    let compiled = compile_project_with_scale(&p, 0.5).unwrap();
    let layer = &compiled.layers()[0];
    let group = match &layer.items[0] {
        lumen::compile::CompiledLayerItem::Group(group) => group,
        _ => panic!("expected group"),
    };
    let shadow = group.shadow.expect("shadow");
    assert_eq!(shadow.offset_x, 10.0);
    assert_eq!(shadow.offset_y, -4.0);
    assert_eq!(shadow.blur_sigma, 5.0);
    assert_eq!(shadow.color, ColorRgba(0, 0, 0, 220));
}

// ===========================================================================
// Does not scale frame numbers
// ===========================================================================

#[test]
fn preserves_frame_numbers_under_scale() {
    let mut p = base_project();
    let clip = Clip {
        id: "c".into(),
        start_frame: 5,
        duration_frames: 10,
        opacity: 1.0,
        transform: Transform {
            x: Scalar::Literal(0.0),
            y: Scalar::Literal(0.0),
            width: None,
            height: None,
            rotation_degrees: 0.0,
        },
        animation: Default::default(),
        shadow: None,
        mask: None,
        content: ClipContent::Solid {
            color: ColorRgba(255, 255, 255, 255),
        },
    };
    p.layers[0].items = vec![LayerItem::Clip(clip)];
    let compiled = compile_project_with_scale(&p, 0.5).unwrap();
    let op = compiled.operation(0).unwrap();
    assert_eq!(op.start_frame, 5);
    assert_eq!(op.end_frame, 15);
    assert_eq!(compiled.total_frames(), 30);
}
