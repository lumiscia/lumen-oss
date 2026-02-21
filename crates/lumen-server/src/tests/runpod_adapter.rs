use lumen::{
    Rational,
    model::{
        BaseStyle, Canvas, ClipContent, ClipItem, ClipStyle, Layer, LayerItem, Project, StyleValue,
        Timeline, TransformStyle,
    },
};
use serde_json::json;

use crate::runpod::{RunpodJobRequest, handle_runpod_request};

fn valid_project_value() -> serde_json::Value {
    let mut style = ClipStyle::default();
    style.base = BaseStyle {
        transform: TransformStyle {
            x: StyleValue::Value(20.0),
            y: StyleValue::Value(20.0),
            width: StyleValue::Value(280.0),
            height: StyleValue::Value(80.0),
            ..Default::default()
        },
        ..Default::default()
    };
    style.font_size = Some(StyleValue::Value(24.0));
    style.color = Some([255, 255, 255, 255]);
    style.align = Some(lumen::model::TextAlign::Center);

    let project = Project {
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
            id: "layer_text".to_string(),
            items: vec![LayerItem::Clip(ClipItem {
                id: "clip_text_1".to_string(),
                start_frame: 0,
                duration_frames: 2,
                content: ClipContent::Text {
                    content: "Hello".to_string(),
                },
                style,
                mask: None,
            })],
        }],
        audio: Default::default(),
    };

    serde_json::to_value(project).expect("serialize project")
}

#[tokio::test]
async fn runpod_adapter_returns_non_retryable_error_without_staging() {
    let request: RunpodJobRequest = serde_json::from_value(json!({
        "input": {
            "job_id": "job_test",
            "project": valid_project_value()
        }
    }))
    .expect("request json");

    let result = handle_runpod_request(request).await;
    assert!(!result.ok);
    let error = result.error.expect("error payload");
    assert_eq!(error.code, "artifact_staging_missing");
    assert!(!error.retryable);
}

#[tokio::test]
async fn runpod_adapter_rejects_non_project_payload() {
    let request: RunpodJobRequest = serde_json::from_value(json!({
        "input": {
            "job_id": "job_bad_preset",
            "project": {
                "kind": "chat_story_v2",
                "version": 1
            }
        }
    }))
    .expect("request json");

    let result = handle_runpod_request(request).await;
    assert!(!result.ok);
    let error = result.error.expect("error payload");
    assert_eq!(error.code, "invalid_project_payload");
    assert!(!error.retryable);
}
