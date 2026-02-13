use serde_json::json;

use crate::runpod::{RunpodJobRequest, handle_runpod_request};

#[tokio::test]
async fn runpod_adapter_returns_non_retryable_error_without_staging() {
    let request: RunpodJobRequest = serde_json::from_value(json!({
        "input": {
            "job_id": "job_test",
            "project": {
                "canvas": {
                    "width": 320,
                    "height": 180,
                    "background": [0, 0, 0, 255]
                },
                "timeline": {
                    "fps": { "num": 30, "den": 1 },
                    "total_frames": 2
                },
                "sources": [],
                "layers": [{
                    "id": "layer_text",
                    "z_index": 0,
                    "clips": [{
                        "id": "clip_text_1",
                        "start_frame": 0,
                        "duration_frames": 2,
                        "opacity": 1.0,
                        "transform": {
                            "x": 20.0,
                            "y": 20.0,
                            "width": 280.0,
                            "height": 80.0,
                            "rotation_degrees": 0.0
                        },
                        "content": {
                            "type": "text",
                            "text": "Hello",
                            "font_size": 24.0,
                            "color": [255, 255, 255, 255],
                            "align": "center"
                        }
                    }]
                }],
                "audio": { "tracks": [] }
            }
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
async fn runpod_adapter_rejects_unknown_preset_kind() {
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
