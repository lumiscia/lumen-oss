use lumen::Project;
use serde_json::json;

use crate::executor::{RenderExecutionOptions, execute_render};

fn missing_source_project() -> Project {
    serde_json::from_value(json!({
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
            "id": "layer_video",
            "z_index": 0,
            "items": [{
                "kind": "clip",
                "id": "clip_video_1",
                "start_frame": 0,
                "duration_frames": 2,
                "opacity": 1.0,
                "transform": {
                    "x": 0.0,
                    "y": 0.0,
                    "width": 320.0,
                    "height": 180.0,
                    "rotation_degrees": 0.0
                },
                "content": {
                    "type": "video",
                    "source": "missing_source",
                    "pipeline": {
                        "trim": null,
                        "speed": 1.0,
                        "reverse": false,
                        "looping": { "mode": "none" }
                    },
                    "fit": "cover"
                }
            }]
        }],
        "audio": { "tracks": [] }
    }))
    .expect("project json")
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
