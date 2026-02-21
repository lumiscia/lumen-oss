//! Animation interpolation and easing tests.
//!
//! Tests cover keyframe resolution at various frame positions, easing curves
//! (linear, ease-in, ease-out, ease-in-out), multi-keyframe sequences, and
//! opacity clamping behavior.

use lumen::{
    Canvas, Clip, ClipAnimation, ClipContent, ColorRgba, Easing, Layer, LayerItem, Project, Scalar,
    ScalarKeyframe, Timeline, Transform, compile_project, time::Rational,
};

fn solid_clip_with_animation(
    id: &str,
    duration: u64,
    transform: Transform,
    animation: ClipAnimation,
) -> Clip {
    Clip {
        id: id.to_string(),
        start_frame: 0,
        duration_frames: duration,
        opacity: 1.0,
        transform,
        animation,
        mask: None,
        content: ClipContent::Solid {
            color: ColorRgba(255, 255, 255, 255),
        },
    }
}

fn anim_project(clip: Clip) -> Project {
    Project {
        canvas: Canvas {
            width: 100,
            height: 100,
            background: ColorRgba(0, 0, 0, 255),
        },
        timeline: Timeline {
            fps: Rational::new(30, 1).unwrap(),
            total_frames: clip.duration_frames,
        },
        sources: vec![],
        layers: vec![Layer {
            id: "layer_1".to_string(),
            z_index: 0,
            items: vec![LayerItem::Clip(clip)],
        }],
        audio: Default::default(),
    }
}

// ===========================================================================
// Instant keyframes (duration_frames = 0)
// ===========================================================================

#[test]
fn instant_keyframe_jumps_to_value() {
    let clip = solid_clip_with_animation(
        "clip",
        30,
        Transform {
            x: Scalar::Literal(0.0),
            y: Scalar::Literal(0.0),
            width: None,
            height: None,
            rotation_degrees: 0.0,
        },
        ClipAnimation {
            x: vec![ScalarKeyframe {
                frame: 10,
                value: Scalar::Literal(100.0),
                duration_frames: 0,
                easing: Easing::Linear,
            }],
            ..Default::default()
        },
    );

    let project = anim_project(clip);
    let compiled = compile_project(&project).expect("compile");
    let op = compiled.operation(0).unwrap();

    // Before keyframe: base value (0)
    assert_eq!(op.resolved_transform(9).x, 0.0);
    // At keyframe: jump to 100
    assert_eq!(op.resolved_transform(10).x, 100.0);
    // After keyframe: stays at 100
    assert_eq!(op.resolved_transform(20).x, 100.0);
}

// ===========================================================================
// Linear interpolation
// ===========================================================================

#[test]
fn linear_interpolation_midpoint() {
    let clip = solid_clip_with_animation(
        "clip",
        30,
        Transform {
            x: Scalar::Literal(0.0),
            y: Scalar::Literal(0.0),
            width: None,
            height: None,
            rotation_degrees: 0.0,
        },
        ClipAnimation {
            x: vec![ScalarKeyframe {
                frame: 0,
                value: Scalar::Literal(100.0),
                duration_frames: 20,
                easing: Easing::Linear,
            }],
            ..Default::default()
        },
    );

    let project = anim_project(clip);
    let compiled = compile_project(&project).expect("compile");
    let op = compiled.operation(0).unwrap();

    // Frame 0: start of transition, base x=0
    assert_eq!(op.resolved_transform(0).x, 0.0);
    // Frame 10: midpoint = 50
    assert_eq!(op.resolved_transform(10).x, 50.0);
    // Frame 20: end of transition = 100
    assert_eq!(op.resolved_transform(20).x, 100.0);
    // Frame 25: after transition, stays at 100
    assert_eq!(op.resolved_transform(25).x, 100.0);
}

// ===========================================================================
// Easing curves
// ===========================================================================

#[test]
fn ease_in_starts_slow() {
    let clip = solid_clip_with_animation(
        "clip",
        30,
        Transform {
            x: Scalar::Literal(0.0),
            y: Scalar::Literal(0.0),
            width: None,
            height: None,
            rotation_degrees: 0.0,
        },
        ClipAnimation {
            x: vec![ScalarKeyframe {
                frame: 0,
                value: Scalar::Literal(100.0),
                duration_frames: 20,
                easing: Easing::EaseIn,
            }],
            ..Default::default()
        },
    );

    let project = anim_project(clip);
    let compiled = compile_project(&project).expect("compile");
    let op = compiled.operation(0).unwrap();

    // At 25% progress (frame 5), ease-in (t^2) gives 0.0625
    let at_quarter = op.resolved_transform(5).x;
    assert!(
        at_quarter < 10.0,
        "ease-in should be slow at start: got {at_quarter}"
    );

    // At 75% progress (frame 15), ease-in gives 0.5625
    let at_three_quarter = op.resolved_transform(15).x;
    assert!(
        at_three_quarter > 50.0,
        "ease-in should be accelerating: got {at_three_quarter}"
    );

    // End should reach target
    assert_eq!(op.resolved_transform(20).x, 100.0);
}

#[test]
fn ease_out_starts_fast() {
    let clip = solid_clip_with_animation(
        "clip",
        30,
        Transform {
            x: Scalar::Literal(0.0),
            y: Scalar::Literal(0.0),
            width: None,
            height: None,
            rotation_degrees: 0.0,
        },
        ClipAnimation {
            x: vec![ScalarKeyframe {
                frame: 0,
                value: Scalar::Literal(100.0),
                duration_frames: 20,
                easing: Easing::EaseOut,
            }],
            ..Default::default()
        },
    );

    let project = anim_project(clip);
    let compiled = compile_project(&project).expect("compile");
    let op = compiled.operation(0).unwrap();

    // At 25% progress, ease-out (1-(1-t)^2) gives 0.4375
    let at_quarter = op.resolved_transform(5).x;
    assert!(
        at_quarter > 40.0,
        "ease-out should be fast at start: got {at_quarter}"
    );

    // End should reach target
    assert_eq!(op.resolved_transform(20).x, 100.0);
}

#[test]
fn ease_in_out_is_symmetric() {
    let clip = solid_clip_with_animation(
        "clip",
        30,
        Transform {
            x: Scalar::Literal(0.0),
            y: Scalar::Literal(0.0),
            width: None,
            height: None,
            rotation_degrees: 0.0,
        },
        ClipAnimation {
            x: vec![ScalarKeyframe {
                frame: 0,
                value: Scalar::Literal(100.0),
                duration_frames: 20,
                easing: Easing::EaseInOut,
            }],
            ..Default::default()
        },
    );

    let project = anim_project(clip);
    let compiled = compile_project(&project).expect("compile");
    let op = compiled.operation(0).unwrap();

    // At midpoint (frame 10), ease-in-out should be at 50%
    let midpoint = op.resolved_transform(10).x;
    assert!(
        (midpoint - 50.0).abs() < 1.0,
        "ease-in-out midpoint should be ~50: got {midpoint}"
    );

    // End should reach target
    assert_eq!(op.resolved_transform(20).x, 100.0);
}

// ===========================================================================
// Multi-keyframe sequences
// ===========================================================================

#[test]
fn multiple_sequential_keyframes() {
    let clip = solid_clip_with_animation(
        "clip",
        60,
        Transform {
            x: Scalar::Literal(0.0),
            y: Scalar::Literal(0.0),
            width: None,
            height: None,
            rotation_degrees: 0.0,
        },
        ClipAnimation {
            x: vec![
                ScalarKeyframe {
                    frame: 0,
                    value: Scalar::Literal(100.0),
                    duration_frames: 10,
                    easing: Easing::Linear,
                },
                ScalarKeyframe {
                    frame: 20,
                    value: Scalar::Literal(50.0),
                    duration_frames: 10,
                    easing: Easing::Linear,
                },
            ],
            ..Default::default()
        },
    );

    let project = Project {
        canvas: Canvas {
            width: 100,
            height: 100,
            background: ColorRgba(0, 0, 0, 255),
        },
        timeline: Timeline {
            fps: Rational::new(30, 1).unwrap(),
            total_frames: 60,
        },
        sources: vec![],
        layers: vec![Layer {
            id: "layer_1".to_string(),
            z_index: 0,
            items: vec![LayerItem::Clip(clip)],
        }],
        audio: Default::default(),
    };

    let compiled = compile_project(&project).expect("compile");
    let op = compiled.operation(0).unwrap();

    // Frame 0: start of first transition (base=0)
    assert_eq!(op.resolved_transform(0).x, 0.0);
    // Frame 5: midpoint of first transition (0 → 100, halfway = 50)
    assert_eq!(op.resolved_transform(5).x, 50.0);
    // Frame 10: end of first transition = 100
    assert_eq!(op.resolved_transform(10).x, 100.0);
    // Frame 15: between keyframes, holds at 100
    assert_eq!(op.resolved_transform(15).x, 100.0);
    // Frame 25: midpoint of second transition (100 → 50, halfway = 75)
    assert_eq!(op.resolved_transform(25).x, 75.0);
    // Frame 30: end of second transition = 50
    assert_eq!(op.resolved_transform(30).x, 50.0);
    // Frame 40: after all transitions, holds at 50
    assert_eq!(op.resolved_transform(40).x, 50.0);
}

// ===========================================================================
// Opacity animation
// ===========================================================================

#[test]
fn opacity_animation_clamps_to_zero_one() {
    let clip = Clip {
        id: "clip".to_string(),
        start_frame: 0,
        duration_frames: 30,
        opacity: 0.5,
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
                value: Scalar::Literal(2.0), // > 1.0
                duration_frames: 0,
                easing: Easing::Linear,
            }],
            ..Default::default()
        },
        mask: None,
        content: ClipContent::Solid {
            color: ColorRgba(255, 255, 255, 255),
        },
    };

    let project = anim_project(clip);
    let compiled = compile_project(&project).expect("compile");
    let op = compiled.operation(0).unwrap();

    // Opacity should be clamped to [0, 1]
    let opacity = op.resolved_opacity(0);
    assert!(
        opacity <= 1.0,
        "opacity should be clamped to 1.0: got {opacity}"
    );
}

// ===========================================================================
// Rotation animation
// ===========================================================================

#[test]
fn rotation_animation() {
    let clip = solid_clip_with_animation(
        "clip",
        30,
        Transform {
            x: Scalar::Literal(0.0),
            y: Scalar::Literal(0.0),
            width: None,
            height: None,
            rotation_degrees: 0.0,
        },
        ClipAnimation {
            rotation_degrees: vec![ScalarKeyframe {
                frame: 0,
                value: Scalar::Literal(360.0),
                duration_frames: 30,
                easing: Easing::Linear,
            }],
            ..Default::default()
        },
    );

    let project = anim_project(clip);
    let compiled = compile_project(&project).expect("compile");
    let op = compiled.operation(0).unwrap();

    // Midpoint should be 180 degrees
    let mid = op.resolved_transform(15).rotation_degrees;
    assert!(
        (mid - 180.0).abs() < 1.0,
        "rotation midpoint should be ~180: got {mid}"
    );
}

// ===========================================================================
// Width/height animation
// ===========================================================================

#[test]
fn width_height_animation() {
    let clip = solid_clip_with_animation(
        "clip",
        30,
        Transform {
            x: Scalar::Literal(0.0),
            y: Scalar::Literal(0.0),
            width: Some(Scalar::Literal(100.0)),
            height: Some(Scalar::Literal(100.0)),
            rotation_degrees: 0.0,
        },
        ClipAnimation {
            width: vec![ScalarKeyframe {
                frame: 0,
                value: Scalar::Literal(200.0),
                duration_frames: 10,
                easing: Easing::Linear,
            }],
            height: vec![ScalarKeyframe {
                frame: 0,
                value: Scalar::Literal(50.0),
                duration_frames: 10,
                easing: Easing::Linear,
            }],
            ..Default::default()
        },
    );

    let project = anim_project(clip);
    let compiled = compile_project(&project).expect("compile");
    let op = compiled.operation(0).unwrap();

    // Frame 5: midpoint of both transitions
    let t = op.resolved_transform(5);
    assert_eq!(t.width, Some(150.0)); // 100 → 200, halfway = 150
    assert_eq!(t.height, Some(75.0)); // 100 → 50, halfway = 75
}

// ===========================================================================
// Operation frame containment
// ===========================================================================

#[test]
fn operation_contains_frame_at_boundaries() {
    let clip = Clip {
        id: "clip".to_string(),
        start_frame: 10,
        duration_frames: 5,
        opacity: 1.0,
        transform: Transform {
            x: Scalar::Literal(0.0),
            y: Scalar::Literal(0.0),
            width: None,
            height: None,
            rotation_degrees: 0.0,
        },
        animation: Default::default(),
        mask: None,
        content: ClipContent::Solid {
            color: ColorRgba(255, 255, 255, 255),
        },
    };

    let project = Project {
        canvas: Canvas {
            width: 100,
            height: 100,
            background: ColorRgba(0, 0, 0, 255),
        },
        timeline: Timeline {
            fps: Rational::new(30, 1).unwrap(),
            total_frames: 30,
        },
        sources: vec![],
        layers: vec![Layer {
            id: "l".to_string(),
            z_index: 0,
            items: vec![LayerItem::Clip(clip)],
        }],
        audio: Default::default(),
    };

    let compiled = compile_project(&project).expect("compile");
    let op = compiled.operation(0).unwrap();

    assert!(!op.contains_frame(9));
    assert!(op.contains_frame(10));
    assert!(op.contains_frame(14));
    assert!(!op.contains_frame(15));
}

#[test]
fn local_frame_is_relative_to_start() {
    let clip = Clip {
        id: "clip".to_string(),
        start_frame: 10,
        duration_frames: 20,
        opacity: 1.0,
        transform: Transform {
            x: Scalar::Literal(0.0),
            y: Scalar::Literal(0.0),
            width: None,
            height: None,
            rotation_degrees: 0.0,
        },
        animation: Default::default(),
        mask: None,
        content: ClipContent::Solid {
            color: ColorRgba(255, 255, 255, 255),
        },
    };

    let project = Project {
        canvas: Canvas {
            width: 100,
            height: 100,
            background: ColorRgba(0, 0, 0, 255),
        },
        timeline: Timeline {
            fps: Rational::new(30, 1).unwrap(),
            total_frames: 30,
        },
        sources: vec![],
        layers: vec![Layer {
            id: "l".to_string(),
            z_index: 0,
            items: vec![LayerItem::Clip(clip)],
        }],
        audio: Default::default(),
    };

    let compiled = compile_project(&project).expect("compile");
    let op = compiled.operation(0).unwrap();

    assert_eq!(op.local_frame(10), 0);
    assert_eq!(op.local_frame(15), 5);
    assert_eq!(op.local_frame(29), 19);
}
