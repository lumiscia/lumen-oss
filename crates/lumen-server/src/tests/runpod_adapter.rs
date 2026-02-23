use serde_json::json;

use crate::runpod::{RunpodJobRequest, handle_runpod_request};

fn valid_project_value() -> serde_json::Value {
    json!({
        "version": "1",
        "canvas": {
            "width": 320,
            "height": 180,
            "background": [0, 0, 0, 255]
        },
        "timeline": {
            "fps": { "num": 30, "den": 1 },
            "duration_frames": 2
        },
        "sources": [],
        "layers": [{
            "id": "layer_text",
            "items": [{
                "type": "clip",
                "id": "clip_text_1",
                "start_frame": 0,
                "duration_frames": 2,
                "content": {
                    "type": "text",
                    "content": "Hello"
                },
                "style": {
                    "transform": {
                        "x": 20.0,
                        "y": 20.0,
                        "width": 280.0,
                        "height": 80.0
                    },
                    "font_size": 24.0,
                    "color": [255, 255, 255, 255],
                    "align": "center"
                }
            }]
        }],
        "audio": { "tracks": [] }
    })
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
