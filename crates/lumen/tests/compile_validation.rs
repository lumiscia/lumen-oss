//! Compile-time validation tests for the lumen project compiler.
//!
//! Tests cover canvas validation, timeline validation, clip validation,
//! group validation, source validation, layer ordering, item tree depth
//! limits, and duplicate ID handling.

use lumen::{
    Canvas, Clip, ClipContent, ClipGroup, ColorRgba, Easing, GroupTransform, ImageClip, Layer,
    LayerItem, LayoutClip, LayoutNode, LayoutNodeKind, LoopMode, Project, Scalar, ScalarKeyframe,
    Shape, ShapeClip, Source, SourceKind, SourceMediaType, SourcePipeline, Timeline, Transform,
    TrimRange, VideoClip, compile::CompileError, compile_project, compile_project_with_scale,
    time::Rational,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn fps30() -> Rational {
    Rational::new(30, 1).unwrap()
}

fn white() -> ColorRgba {
    ColorRgba(255, 255, 255, 255)
}

fn black() -> ColorRgba {
    ColorRgba(0, 0, 0, 255)
}

fn solid_content() -> ClipContent {
    ClipContent::Solid { color: white() }
}


fn default_transform(w: f32, h: f32) -> Transform {
    Transform {
        x: Scalar::Literal(0.0),
        y: Scalar::Literal(0.0),
        width: Some(Scalar::Literal(w)),
        height: Some(Scalar::Literal(h)),
        rotation_degrees: 0.0,
    }
}

fn minimal_transform() -> Transform {
    Transform {
        x: Scalar::Literal(0.0),
        y: Scalar::Literal(0.0),
        width: None,
        height: None,
        rotation_degrees: 0.0,
    }
}

fn solid_clip(id: &str, start: u64, duration: u64) -> Clip {
    Clip {
        id: id.to_string(),
        start_frame: start,
        duration_frames: duration,
        opacity: 1.0,
        transform: minimal_transform(),
        animation: Default::default(),
        mask: None,
        content: solid_content(),
    }
}

fn base_project() -> Project {
    Project {
        canvas: Canvas {
            width: 100,
            height: 100,
            background: black(),
        },
        timeline: Timeline {
            fps: fps30(),
            total_frames: 30,
        },
        sources: vec![],
        layers: vec![Layer {
            id: "layer_1".into(),
            z_index: 0,
            items: vec![LayerItem::Clip(solid_clip("clip_1", 0, 30))],
        }],
        audio: Default::default(),
    }
}

// ===========================================================================
// Canvas validation
// ===========================================================================

#[test]
fn rejects_zero_canvas_width() {
    let mut p = base_project();
    p.canvas.width = 0;
    let err = compile_project(&p).unwrap_err();
    assert!(matches!(err, CompileError::InvalidCanvas(_)));
}

#[test]
fn rejects_zero_canvas_height() {
    let mut p = base_project();
    p.canvas.height = 0;
    let err = compile_project(&p).unwrap_err();
    assert!(matches!(err, CompileError::InvalidCanvas(_)));
}

#[test]
fn rejects_scale_that_zeros_canvas() {
    let p = base_project(); // 100x100
    // scale of 0.001 → 0.1 pixels, rounds to 0
    let err = compile_project_with_scale(&p, 0.001).unwrap_err();
    assert!(matches!(err, CompileError::InvalidCanvas(_)));
}

#[test]
fn rejects_zero_scale() {
    let p = base_project();
    let err = compile_project_with_scale(&p, 0.0).unwrap_err();
    assert!(matches!(err, CompileError::InvalidCanvas(_)));
}

#[test]
fn rejects_negative_scale() {
    let p = base_project();
    let err = compile_project_with_scale(&p, -1.0).unwrap_err();
    assert!(matches!(err, CompileError::InvalidCanvas(_)));
}

#[test]
fn rejects_nan_scale() {
    let p = base_project();
    let err = compile_project_with_scale(&p, f32::NAN).unwrap_err();
    assert!(matches!(err, CompileError::InvalidCanvas(_)));
}

#[test]
fn rejects_infinite_scale() {
    let p = base_project();
    let err = compile_project_with_scale(&p, f32::INFINITY).unwrap_err();
    assert!(matches!(err, CompileError::InvalidCanvas(_)));
}

#[test]
fn accepts_valid_fractional_scale() {
    let p = base_project();
    let compiled = compile_project_with_scale(&p, 0.5).expect("should compile");
    assert_eq!(compiled.canvas.width, 50);
    assert_eq!(compiled.canvas.height, 50);
}

// ===========================================================================
// Timeline validation
// ===========================================================================

#[test]
fn rejects_zero_total_frames() {
    let mut p = base_project();
    p.timeline.total_frames = 0;
    p.layers[0].items = vec![]; // no clips
    let err = compile_project(&p).unwrap_err();
    assert!(matches!(err, CompileError::InvalidTimeline(_)));
}

#[test]
fn rejects_zero_fps_numerator() {
    let mut p = base_project();
    p.timeline.fps = Rational { num: 0, den: 1 };
    let err = compile_project(&p).unwrap_err();
    assert!(matches!(err, CompileError::InvalidTimeline(_)));
}

#[test]
fn rejects_zero_fps_denominator() {
    let mut p = base_project();
    p.timeline.fps = Rational { num: 30, den: 0 };
    let err = compile_project(&p).unwrap_err();
    assert!(matches!(err, CompileError::InvalidTimeline(_)));
}

// ===========================================================================
// Clip validation
// ===========================================================================

#[test]
fn rejects_clip_with_zero_duration() {
    let mut p = base_project();
    if let LayerItem::Clip(clip) = &mut p.layers[0].items[0] {
        clip.duration_frames = 0;
    }
    let err = compile_project(&p).unwrap_err();
    assert!(matches!(err, CompileError::InvalidClip { .. }));
}

#[test]
fn rejects_clip_extending_past_timeline() {
    let mut p = base_project();
    if let LayerItem::Clip(clip) = &mut p.layers[0].items[0] {
        clip.start_frame = 20;
        clip.duration_frames = 20; // ends at 40 > total_frames(30)
    }
    let err = compile_project(&p).unwrap_err();
    assert!(matches!(err, CompileError::InvalidClip { .. }));
}

#[test]
fn rejects_clip_with_negative_opacity() {
    let mut p = base_project();
    if let LayerItem::Clip(clip) = &mut p.layers[0].items[0] {
        clip.opacity = -0.5;
    }
    let err = compile_project(&p).unwrap_err();
    assert!(matches!(err, CompileError::InvalidClip { .. }));
}

#[test]
fn rejects_clip_with_nan_opacity() {
    let mut p = base_project();
    if let LayerItem::Clip(clip) = &mut p.layers[0].items[0] {
        clip.opacity = f32::NAN;
    }
    let err = compile_project(&p).unwrap_err();
    assert!(matches!(err, CompileError::InvalidClip { .. }));
}

#[test]
fn rejects_clip_with_nan_rotation() {
    let mut p = base_project();
    if let LayerItem::Clip(clip) = &mut p.layers[0].items[0] {
        clip.transform.rotation_degrees = f32::NAN;
    }
    let err = compile_project(&p).unwrap_err();
    assert!(matches!(err, CompileError::InvalidClip { .. }));
}

#[test]
fn rejects_clip_with_infinite_rotation() {
    let mut p = base_project();
    if let LayerItem::Clip(clip) = &mut p.layers[0].items[0] {
        clip.transform.rotation_degrees = f32::INFINITY;
    }
    let err = compile_project(&p).unwrap_err();
    assert!(matches!(err, CompileError::InvalidClip { .. }));
}

#[test]
fn rejects_clip_with_zero_width() {
    let mut p = base_project();
    if let LayerItem::Clip(clip) = &mut p.layers[0].items[0] {
        clip.transform.width = Some(Scalar::Literal(0.0));
    }
    let err = compile_project(&p).unwrap_err();
    assert!(matches!(err, CompileError::InvalidClip { .. }));
}

#[test]
fn rejects_clip_with_negative_width() {
    let mut p = base_project();
    if let LayerItem::Clip(clip) = &mut p.layers[0].items[0] {
        clip.transform.width = Some(Scalar::Literal(-10.0));
    }
    let err = compile_project(&p).unwrap_err();
    assert!(matches!(err, CompileError::InvalidClip { .. }));
}

#[test]
fn rejects_clip_with_nan_width() {
    let mut p = base_project();
    if let LayerItem::Clip(clip) = &mut p.layers[0].items[0] {
        clip.transform.width = Some(Scalar::Literal(f32::NAN));
    }
    let err = compile_project(&p).unwrap_err();
    assert!(matches!(err, CompileError::InvalidClip { .. }));
}

#[test]
fn accepts_clip_with_no_width() {
    let p = base_project();
    compile_project(&p).expect("no width should be fine");
}

#[test]
fn accepts_clip_at_exact_timeline_end() {
    let mut p = base_project();
    if let LayerItem::Clip(clip) = &mut p.layers[0].items[0] {
        clip.start_frame = 0;
        clip.duration_frames = 30; // ends exactly at total_frames
    }
    compile_project(&p).expect("clip ending exactly at total_frames is valid");
}

#[test]
fn clamps_opacity_above_one_to_one() {
    let mut p = base_project();
    if let LayerItem::Clip(clip) = &mut p.layers[0].items[0] {
        clip.opacity = 2.0; // should clamp to 1.0
    }
    let compiled = compile_project(&p).expect("compile");
    let op = compiled.operation(0).unwrap();
    assert!(op.opacity <= 1.0);
}

// ===========================================================================
// Animation keyframe validation
// ===========================================================================

#[test]
fn rejects_keyframe_past_clip_duration() {
    let mut p = base_project();
    if let LayerItem::Clip(clip) = &mut p.layers[0].items[0] {
        clip.transform.width = Some(Scalar::Literal(50.0));
        clip.animation.width = vec![ScalarKeyframe {
            frame: 30, // clip duration is 30, so frame 30 is out of bounds
            value: Scalar::Literal(100.0),
            duration_frames: 0,
            easing: Easing::Linear,
        }];
    }
    let err = compile_project(&p).unwrap_err();
    assert!(matches!(err, CompileError::InvalidClip { .. }));
}

#[test]
fn rejects_keyframe_transition_extending_past_clip() {
    let mut p = base_project();
    if let LayerItem::Clip(clip) = &mut p.layers[0].items[0] {
        clip.transform.width = Some(Scalar::Literal(50.0));
        clip.animation.width = vec![ScalarKeyframe {
            frame: 25,
            value: Scalar::Literal(100.0),
            duration_frames: 10, // ends at 35 > 30
            easing: Easing::Linear,
        }];
    }
    let err = compile_project(&p).unwrap_err();
    assert!(matches!(err, CompileError::InvalidClip { .. }));
}

#[test]
fn rejects_overlapping_keyframes() {
    let mut p = base_project();
    if let LayerItem::Clip(clip) = &mut p.layers[0].items[0] {
        clip.transform.width = Some(Scalar::Literal(50.0));
        clip.animation.width = vec![
            ScalarKeyframe {
                frame: 0,
                value: Scalar::Literal(50.0),
                duration_frames: 15,
                easing: Easing::Linear,
            },
            ScalarKeyframe {
                frame: 10, // starts at 10 but previous ends at 15 — overlap
                value: Scalar::Literal(100.0),
                duration_frames: 5,
                easing: Easing::Linear,
            },
        ];
    }
    let err = compile_project(&p).unwrap_err();
    assert!(matches!(err, CompileError::InvalidClip { .. }));
}

#[test]
fn rejects_nan_keyframe_value() {
    let mut p = base_project();
    if let LayerItem::Clip(clip) = &mut p.layers[0].items[0] {
        clip.animation.x = vec![ScalarKeyframe {
            frame: 0,
            value: Scalar::Literal(f32::NAN),
            duration_frames: 0,
            easing: Easing::Linear,
        }];
    }
    let err = compile_project(&p).unwrap_err();
    assert!(matches!(err, CompileError::InvalidClip { .. }));
}

#[test]
fn rejects_negative_width_keyframe_value() {
    let mut p = base_project();
    if let LayerItem::Clip(clip) = &mut p.layers[0].items[0] {
        clip.transform.width = Some(Scalar::Literal(50.0));
        clip.animation.width = vec![ScalarKeyframe {
            frame: 0,
            value: Scalar::Literal(-10.0),
            duration_frames: 0,
            easing: Easing::Linear,
        }];
    }
    let err = compile_project(&p).unwrap_err();
    assert!(matches!(err, CompileError::InvalidClip { .. }));
}

#[test]
fn rejects_width_animation_without_base_width() {
    let mut p = base_project();
    if let LayerItem::Clip(clip) = &mut p.layers[0].items[0] {
        clip.transform.width = None;
        clip.animation.width = vec![ScalarKeyframe {
            frame: 0,
            value: Scalar::Literal(100.0),
            duration_frames: 0,
            easing: Easing::Linear,
        }];
    }
    let err = compile_project(&p).unwrap_err();
    assert!(matches!(err, CompileError::InvalidClip { .. }));
}

#[test]
fn accepts_valid_keyframe_sequence() {
    let mut p = base_project();
    if let LayerItem::Clip(clip) = &mut p.layers[0].items[0] {
        clip.animation.x = vec![
            ScalarKeyframe {
                frame: 0,
                value: Scalar::Literal(10.0),
                duration_frames: 10,
                easing: Easing::EaseIn,
            },
            ScalarKeyframe {
                frame: 15,
                value: Scalar::Literal(20.0),
                duration_frames: 5,
                easing: Easing::EaseOut,
            },
        ];
    }
    compile_project(&p).expect("valid keyframe sequence");
}

// ===========================================================================
// Source validation
// ===========================================================================

#[test]
fn rejects_duplicate_source_ids() {
    let mut p = base_project();
    p.sources = vec![
        Source {
            id: "src_1".into(),
            kind: SourceKind::File {
                media: SourceMediaType::Image,
                path: "a.png".into(),
            },
        },
        Source {
            id: "src_1".into(),
            kind: SourceKind::File {
                media: SourceMediaType::Image,
                path: "b.png".into(),
            },
        },
    ];
    let err = compile_project(&p).unwrap_err();
    assert!(matches!(err, CompileError::DuplicateSourceId(_)));
}

#[test]
fn rejects_missing_image_source() {
    let mut p = base_project();
    p.layers[0].items = vec![LayerItem::Clip(Clip {
        id: "clip_img".into(),
        start_frame: 0,
        duration_frames: 30,
        opacity: 1.0,
        transform: default_transform(100.0, 100.0),
        animation: Default::default(),
        mask: None,
        content: ClipContent::Image(ImageClip {
            source: "nonexistent".into(),
            fit: Default::default(),
            corner_radius: 0.0,
        }),
    })];
    let err = compile_project(&p).unwrap_err();
    assert!(matches!(err, CompileError::MissingSource(_)));
}

#[test]
fn rejects_source_type_mismatch() {
    let mut p = base_project();
    p.sources = vec![Source {
        id: "src_vid".into(),
        kind: SourceKind::File {
            media: SourceMediaType::Video,
            path: "video.mp4".into(),
        },
    }];
    p.layers[0].items = vec![LayerItem::Clip(Clip {
        id: "clip_img".into(),
        start_frame: 0,
        duration_frames: 30,
        opacity: 1.0,
        transform: default_transform(100.0, 100.0),
        animation: Default::default(),
        mask: None,
        content: ClipContent::Image(ImageClip {
            source: "src_vid".into(), // wrong type: video, expected image
            fit: Default::default(),
            corner_radius: 0.0,
        }),
    })];
    let err = compile_project(&p).unwrap_err();
    assert!(matches!(err, CompileError::SourceTypeMismatch { .. }));
}

#[test]
fn accepts_valid_image_source() {
    let mut p = base_project();
    p.sources = vec![Source {
        id: "src_img".into(),
        kind: SourceKind::File {
            media: SourceMediaType::Image,
            path: "image.png".into(),
        },
    }];
    p.layers[0].items = vec![LayerItem::Clip(Clip {
        id: "clip_img".into(),
        start_frame: 0,
        duration_frames: 30,
        opacity: 1.0,
        transform: default_transform(100.0, 100.0),
        animation: Default::default(),
        mask: None,
        content: ClipContent::Image(ImageClip {
            source: "src_img".into(),
            fit: Default::default(),
            corner_radius: 0.0,
        }),
    })];
    compile_project(&p).expect("valid image source");
}

#[test]
fn rejects_negative_corner_radius() {
    let mut p = base_project();
    p.sources = vec![Source {
        id: "src_img".into(),
        kind: SourceKind::File {
            media: SourceMediaType::Image,
            path: "image.png".into(),
        },
    }];
    p.layers[0].items = vec![LayerItem::Clip(Clip {
        id: "clip_img".into(),
        start_frame: 0,
        duration_frames: 30,
        opacity: 1.0,
        transform: default_transform(100.0, 100.0),
        animation: Default::default(),
        mask: None,
        content: ClipContent::Image(ImageClip {
            source: "src_img".into(),
            fit: Default::default(),
            corner_radius: -5.0,
        }),
    })];
    let err = compile_project(&p).unwrap_err();
    assert!(matches!(err, CompileError::InvalidClip { .. }));
}

// ===========================================================================
// Group validation
// ===========================================================================

#[test]
fn rejects_group_with_nan_opacity() {
    let mut p = base_project();
    p.layers[0].items = vec![LayerItem::Group(ClipGroup {
        id: "group_1".into(),
        opacity: f32::NAN,
        transform: GroupTransform {
            x: Scalar::Literal(0.0),
            y: Scalar::Literal(0.0),
            rotation_degrees: 0.0,
        },
        items: vec![LayerItem::Clip(solid_clip("inner", 0, 30))],
        mask: None,
    })];
    let err = compile_project(&p).unwrap_err();
    assert!(matches!(err, CompileError::InvalidGroup { .. }));
}

#[test]
fn rejects_group_with_negative_opacity() {
    let mut p = base_project();
    p.layers[0].items = vec![LayerItem::Group(ClipGroup {
        id: "group_1".into(),
        opacity: -0.1,
        transform: GroupTransform {
            x: Scalar::Literal(0.0),
            y: Scalar::Literal(0.0),
            rotation_degrees: 0.0,
        },
        items: vec![LayerItem::Clip(solid_clip("inner", 0, 30))],
        mask: None,
    })];
    let err = compile_project(&p).unwrap_err();
    assert!(matches!(err, CompileError::InvalidGroup { .. }));
}

#[test]
fn rejects_group_with_nan_rotation() {
    let mut p = base_project();
    p.layers[0].items = vec![LayerItem::Group(ClipGroup {
        id: "group_1".into(),
        opacity: 1.0,
        transform: GroupTransform {
            x: Scalar::Literal(0.0),
            y: Scalar::Literal(0.0),
            rotation_degrees: f32::INFINITY,
        },
        items: vec![LayerItem::Clip(solid_clip("inner", 0, 30))],
        mask: None,
    })];
    let err = compile_project(&p).unwrap_err();
    assert!(matches!(err, CompileError::InvalidGroup { .. }));
}

#[test]
fn accepts_valid_group() {
    let mut p = base_project();
    p.layers[0].items = vec![LayerItem::Group(ClipGroup {
        id: "group_1".into(),
        opacity: 0.5,
        transform: GroupTransform {
            x: Scalar::Literal(10.0),
            y: Scalar::Literal(20.0),
            rotation_degrees: 45.0,
        },
        items: vec![LayerItem::Clip(solid_clip("inner", 0, 30))],
        mask: None,
    })];
    compile_project(&p).expect("valid group");
}

// ===========================================================================
// Item tree depth
// ===========================================================================

#[test]
fn rejects_excessive_nesting_depth() {
    let mut p = base_project();

    // Build 17 levels of nesting (MAX_ITEM_TREE_DEPTH is 16)
    let mut deepest = LayerItem::Clip(solid_clip("deep", 0, 30));
    for i in 0..17 {
        deepest = LayerItem::Group(ClipGroup {
            id: format!("group_{i}"),
            opacity: 1.0,
            transform: GroupTransform {
                x: Scalar::Literal(0.0),
                y: Scalar::Literal(0.0),
                rotation_degrees: 0.0,
            },
            items: vec![deepest],
            mask: None,
        });
    }
    p.layers[0].items = vec![deepest];
    let err = compile_project(&p).unwrap_err();
    assert!(matches!(err, CompileError::ItemTreeDepthExceeded { .. }));
}

#[test]
fn accepts_nesting_within_depth_limit() {
    let mut p = base_project();

    let mut item = LayerItem::Clip(solid_clip("deep", 0, 30));
    for i in 0..14 {
        item = LayerItem::Group(ClipGroup {
            id: format!("group_{i}"),
            opacity: 1.0,
            transform: GroupTransform {
                x: Scalar::Literal(0.0),
                y: Scalar::Literal(0.0),
                rotation_degrees: 0.0,
            },
            items: vec![item],
            mask: None,
        });
    }
    p.layers[0].items = vec![item];
    compile_project(&p).expect("within depth limit");
}

// ===========================================================================
// Layer ordering
// ===========================================================================

#[test]
fn sorts_layers_by_z_index() {
    let p = Project {
        canvas: Canvas {
            width: 100,
            height: 100,
            background: black(),
        },
        timeline: Timeline {
            fps: fps30(),
            total_frames: 30,
        },
        sources: vec![],
        layers: vec![
            Layer {
                id: "bg".into(),
                z_index: -1,
                items: vec![LayerItem::Clip(solid_clip("bg_clip", 0, 30))],
            },
            Layer {
                id: "fg".into(),
                z_index: 1,
                items: vec![LayerItem::Clip(solid_clip("fg_clip", 0, 30))],
            },
            Layer {
                id: "mid".into(),
                z_index: 0,
                items: vec![LayerItem::Clip(solid_clip("mid_clip", 0, 30))],
            },
        ],
        audio: Default::default(),
    };

    let compiled = compile_project(&p).expect("compile");
    let layer_ids: Vec<&str> = compiled.layers().iter().map(|l| l.id.as_str()).collect();
    assert_eq!(layer_ids, vec!["bg", "mid", "fg"]);
}

// ===========================================================================
// Frame index
// ===========================================================================

#[test]
fn frame_index_maps_operations_correctly() {
    let p = Project {
        canvas: Canvas {
            width: 100,
            height: 100,
            background: black(),
        },
        timeline: Timeline {
            fps: fps30(),
            total_frames: 30,
        },
        sources: vec![],
        layers: vec![Layer {
            id: "layer_1".into(),
            z_index: 0,
            items: vec![
                LayerItem::Clip(solid_clip("early", 0, 10)),
                LayerItem::Clip(solid_clip("late", 20, 10)),
            ],
        }],
        audio: Default::default(),
    };

    let compiled = compile_project(&p).expect("compile");

    // Frame 0 should contain "early" but not "late"
    let ops_0 = compiled.operation_indices_for_frame(0).expect("frame 0");
    assert_eq!(ops_0.len(), 1);

    // Frame 15 should contain neither
    let ops_15 = compiled.operation_indices_for_frame(15).expect("frame 15");
    assert_eq!(ops_15.len(), 0);

    // Frame 25 should contain "late"
    let ops_25 = compiled.operation_indices_for_frame(25).expect("frame 25");
    assert_eq!(ops_25.len(), 1);
}

#[test]
fn frame_out_of_range_returns_error() {
    let p = base_project();
    let compiled = compile_project(&p).expect("compile");
    let err = compiled.operation_indices_for_frame(999);
    assert!(err.is_err());
}

// ===========================================================================
// Layout clip validation
// ===========================================================================

#[test]
fn rejects_layout_clip_without_width() {
    let mut p = base_project();
    p.layers[0].items = vec![LayerItem::Clip(Clip {
        id: "layout_1".into(),
        start_frame: 0,
        duration_frames: 30,
        opacity: 1.0,
        transform: Transform {
            x: Scalar::Literal(0.0),
            y: Scalar::Literal(0.0),
            width: None, // missing
            height: Some(Scalar::Literal(100.0)),
            rotation_degrees: 0.0,
        },
        animation: Default::default(),
        mask: None,
        content: ClipContent::Layout(LayoutClip {
            root: LayoutNode {
                id: None,
                style: Default::default(),
                kind: LayoutNodeKind::Container { children: vec![] },
            },
        }),
    })];
    let err = compile_project(&p).unwrap_err();
    assert!(matches!(err, CompileError::InvalidClip { .. }));
}

#[test]
fn rejects_layout_clip_without_height() {
    let mut p = base_project();
    p.layers[0].items = vec![LayerItem::Clip(Clip {
        id: "layout_1".into(),
        start_frame: 0,
        duration_frames: 30,
        opacity: 1.0,
        transform: Transform {
            x: Scalar::Literal(0.0),
            y: Scalar::Literal(0.0),
            width: Some(Scalar::Literal(100.0)),
            height: None, // missing
            rotation_degrees: 0.0,
        },
        animation: Default::default(),
        mask: None,
        content: ClipContent::Layout(LayoutClip {
            root: LayoutNode {
                id: None,
                style: Default::default(),
                kind: LayoutNodeKind::Container { children: vec![] },
            },
        }),
    })];
    let err = compile_project(&p).unwrap_err();
    assert!(matches!(err, CompileError::InvalidClip { .. }));
}

// ===========================================================================
// Video pipeline validation
// ===========================================================================

#[test]
fn rejects_reverse_without_bounded_trim() {
    let mut p = base_project();
    p.sources = vec![Source {
        id: "vid".into(),
        kind: SourceKind::File {
            media: SourceMediaType::Video,
            path: "video.mp4".into(),
        },
    }];
    p.layers[0].items = vec![LayerItem::Clip(Clip {
        id: "clip_vid".into(),
        start_frame: 0,
        duration_frames: 30,
        opacity: 1.0,
        transform: default_transform(100.0, 100.0),
        animation: Default::default(),
        mask: None,
        content: ClipContent::Video(VideoClip {
            source: "vid".into(),
            pipeline: SourcePipeline {
                trim: None,
                speed: 1.0,
                reverse: true, // needs bounded trim
                looping: LoopMode::None,
            },
            fit: Default::default(),
            corner_radius: 0.0,
        }),
    })];
    let err = compile_project(&p).unwrap_err();
    assert!(matches!(err, CompileError::Pipeline(_)));
}

#[test]
fn rejects_loop_without_bounded_trim() {
    let mut p = base_project();
    p.sources = vec![Source {
        id: "vid".into(),
        kind: SourceKind::File {
            media: SourceMediaType::Video,
            path: "video.mp4".into(),
        },
    }];
    p.layers[0].items = vec![LayerItem::Clip(Clip {
        id: "clip_vid".into(),
        start_frame: 0,
        duration_frames: 30,
        opacity: 1.0,
        transform: default_transform(100.0, 100.0),
        animation: Default::default(),
        mask: None,
        content: ClipContent::Video(VideoClip {
            source: "vid".into(),
            pipeline: SourcePipeline {
                trim: None,
                speed: 1.0,
                reverse: false,
                looping: LoopMode::Infinite, // needs bounded trim
            },
            fit: Default::default(),
            corner_radius: 0.0,
        }),
    })];
    let err = compile_project(&p).unwrap_err();
    assert!(matches!(err, CompileError::Pipeline(_)));
}

#[test]
fn accepts_valid_video_pipeline() {
    let mut p = base_project();
    p.sources = vec![Source {
        id: "vid".into(),
        kind: SourceKind::File {
            media: SourceMediaType::Video,
            path: "video.mp4".into(),
        },
    }];
    p.layers[0].items = vec![LayerItem::Clip(Clip {
        id: "clip_vid".into(),
        start_frame: 0,
        duration_frames: 30,
        opacity: 1.0,
        transform: default_transform(100.0, 100.0),
        animation: Default::default(),
        mask: None,
        content: ClipContent::Video(VideoClip {
            source: "vid".into(),
            pipeline: SourcePipeline {
                trim: Some(TrimRange {
                    start_frame: 0,
                    end_frame: Some(60),
                }),
                speed: 1.0,
                reverse: false,
                looping: LoopMode::Finite { count: 2 },
            },
            fit: Default::default(),
            corner_radius: 0.0,
        }),
    })];
    compile_project(&p).expect("valid video pipeline");
}

// ===========================================================================
// Mask compilation
// ===========================================================================

#[test]
fn compiles_clip_with_mask() {
    let mut p = base_project();
    p.layers[0].items = vec![LayerItem::Clip(Clip {
        id: "masked_clip".into(),
        start_frame: 0,
        duration_frames: 30,
        opacity: 1.0,
        transform: minimal_transform(),
        animation: Default::default(),
        mask: Some(Box::new(LayerItem::Clip(solid_clip("mask", 0, 30)))),
        content: solid_content(),
    })];
    let compiled = compile_project(&p).expect("compile with mask");
    assert!(compiled.has_compositing_nodes());
}

// ===========================================================================
// Multiple layers with multiple clips
// ===========================================================================

#[test]
fn compiles_multi_layer_multi_clip_project() {
    let p = Project {
        canvas: Canvas {
            width: 1920,
            height: 1080,
            background: black(),
        },
        timeline: Timeline {
            fps: fps30(),
            total_frames: 90,
        },
        sources: vec![],
        layers: vec![
            Layer {
                id: "bg".into(),
                z_index: 0,
                items: vec![LayerItem::Clip(solid_clip("bg_solid", 0, 90))],
            },
            Layer {
                id: "content".into(),
                z_index: 1,
                items: vec![
                    LayerItem::Clip(solid_clip("clip_a", 0, 30)),
                    LayerItem::Clip(solid_clip("clip_b", 30, 30)),
                    LayerItem::Clip(solid_clip("clip_c", 60, 30)),
                ],
            },
        ],
        audio: Default::default(),
    };

    let compiled = compile_project(&p).expect("compile");
    assert_eq!(compiled.total_frames(), 90);

    // Frame 0: bg_solid + clip_a
    let ops = compiled.operation_indices_for_frame(0).unwrap();
    assert_eq!(ops.len(), 2);

    // Frame 45: bg_solid + clip_b
    let ops = compiled.operation_indices_for_frame(45).unwrap();
    assert_eq!(ops.len(), 2);
}

// ===========================================================================
// Shape clip content
// ===========================================================================

#[test]
fn compiles_rectangle_shape() {
    let mut p = base_project();
    p.layers[0].items = vec![LayerItem::Clip(Clip {
        id: "shape_clip".into(),
        start_frame: 0,
        duration_frames: 30,
        opacity: 1.0,
        transform: default_transform(50.0, 50.0),
        animation: Default::default(),
        mask: None,
        content: ClipContent::Shape(ShapeClip {
            shape: Shape::Rectangle {
                fill: ColorRgba(255, 0, 0, 255),
                radius: 8.0,
            },
        }),
    })];
    compile_project(&p).expect("valid rectangle shape");
}

#[test]
fn compiles_ellipse_shape() {
    let mut p = base_project();
    p.layers[0].items = vec![LayerItem::Clip(Clip {
        id: "shape_clip".into(),
        start_frame: 0,
        duration_frames: 30,
        opacity: 1.0,
        transform: default_transform(50.0, 50.0),
        animation: Default::default(),
        mask: None,
        content: ClipContent::Shape(ShapeClip {
            shape: Shape::Ellipse {
                fill: ColorRgba(0, 255, 0, 255),
            },
        }),
    })];
    compile_project(&p).expect("valid ellipse shape");
}
