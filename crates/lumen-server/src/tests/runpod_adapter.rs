use serde_json::json;

use crate::runpod::{RunpodJobRequest, handle_runpod_request};

fn valid_project_value() -> serde_json::Value {
    json!({
        "timeline": {
            "fps": 30.0,
            "duration_frames": 2
        },
        "render_settings": {
            "width": 320,
            "height": 180,
            "background_color": [0, 0, 0, 255]
        },
        "nodes": [
            {
                "id": 1,
                "type": "solid_color",
                "properties": {
                    "color": [255, 255, 255, 255],
                    "width": 320,
                    "height": 180
                }
            },
            {
                "id": 2,
                "type": "media_output",
                "properties": {}
            }
        ],
        "connections": [
            {
                "from_node": 1,
                "from_port": "output",
                "to_node": 2,
                "to_port": "source"
            }
        ]
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
