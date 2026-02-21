use lumen::{
    Project, Rational,
    model::{
        BaseStyle, Canvas, ClipContent, ClipItem, ClipStyle, Layer, LayerItem, StyleValue,
        Timeline, TransformStyle,
    },
};

use crate::executor::{RenderExecutionOptions, execute_render};

fn missing_source_project() -> Project {
    let mut style = ClipStyle::default();
    style.base = BaseStyle {
        transform: TransformStyle {
            x: StyleValue::Value(0.0),
            y: StyleValue::Value(0.0),
            width: StyleValue::Value(320.0),
            height: StyleValue::Value(180.0),
            ..Default::default()
        },
        ..Default::default()
    };

    Project {
        version: "1".to_string(),
        canvas: Canvas {
            width: 320,
            height: 180,
            background: [0, 0, 0, 255],
        },
        timeline: Timeline {
            fps: Rational::new(30, 1),
            duration_frames: 2,
        },
        sources: Vec::new(),
        layers: vec![Layer {
            id: "layer_video".to_string(),
            items: vec![LayerItem::Clip(ClipItem {
                id: "clip_video_1".to_string(),
                start_frame: 0,
                duration_frames: 2,
                content: ClipContent::Video {
                    source: "missing_source".to_string(),
                    pipeline: Default::default(),
                },
                style,
                mask: None,
            })],
        }],
        audio: Default::default(),
    }
}

#[test]
fn executor_returns_typed_error_for_invalid_project() {
    let project = missing_source_project();
    let mut progress = |_event| {};
    let result = execute_render(&project, &RenderExecutionOptions::default(), &mut progress);

    let error = result.expect_err("expected compile failure");
    assert_eq!(error.code, "compile_failed");
    assert!(!error.retryable);
}
