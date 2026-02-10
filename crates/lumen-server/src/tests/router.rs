use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use serde_json::json;
use tower::ServiceExt;
use tokio::time::{Duration, sleep};

use crate::{app_state::AppState, endpoint, worker};

#[tokio::test]
async fn unauthorized_request_is_rejected() {
    let app = endpoint::build_router(AppState::with_defaults("secret".to_string()));

    let request = Request::builder()
        .method("POST")
        .uri("/renders")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(valid_sequence_json().to_string()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn authorized_post_creates_queued_job() {
    let app = endpoint::build_router(AppState::with_defaults("secret".to_string()));

    let request = Request::builder()
        .method("POST")
        .uri("/renders")
        .header(header::AUTHORIZATION, "Bearer secret")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(valid_sequence_json().to_string()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        status,
        StatusCode::ACCEPTED,
        "unexpected response body: {}",
        String::from_utf8_lossy(&body)
    );

    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["state"], "queued");
    assert!(json["job_id"].as_str().unwrap().starts_with("render_"));
}

#[tokio::test]
async fn lifecycle_completes_and_returns_artifact_and_frame() {
    let state = AppState::with_defaults("secret".to_string());
    worker::spawn_render_worker(state.clone());
    let app = endpoint::build_router(state);

    let request = Request::builder()
        .method("POST")
        .uri("/renders")
        .header(header::AUTHORIZATION, "Bearer secret")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(valid_sequence_json().to_string()))
        .unwrap();

    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        status,
        StatusCode::ACCEPTED,
        "unexpected response body: {}",
        String::from_utf8_lossy(&body)
    );
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let job_id = json["job_id"].as_str().unwrap().to_string();

    let mut completed = false;
    for _ in 0..40 {
        let request = Request::builder()
            .method("GET")
            .uri(format!("/renders/{job_id}"))
            .header(header::AUTHORIZATION, "Bearer secret")
            .body(Body::empty())
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        if json["state"] == "completed" {
            completed = true;
            break;
        }

        sleep(Duration::from_millis(50)).await;
    }

    assert!(completed, "job did not complete in time");

    let artifact_request = Request::builder()
        .method("GET")
        .uri(format!("/renders/{job_id}/artifact"))
        .header(header::AUTHORIZATION, "Bearer secret")
        .body(Body::empty())
        .unwrap();
    let artifact_response = app.clone().oneshot(artifact_request).await.unwrap();
    assert_eq!(artifact_response.status(), StatusCode::OK);
    assert_eq!(
        artifact_response.headers().get(header::CONTENT_TYPE).unwrap(),
        "video/mp4"
    );
    let artifact_bytes = artifact_response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes();
    assert!(!artifact_bytes.is_empty());

    let frame_request = Request::builder()
        .method("GET")
        .uri(format!("/renders/{job_id}/frames/0"))
        .header(header::AUTHORIZATION, "Bearer secret")
        .body(Body::empty())
        .unwrap();
    let frame_response = app.clone().oneshot(frame_request).await.unwrap();
    assert_eq!(frame_response.status(), StatusCode::OK);
    assert_eq!(
        frame_response.headers().get(header::CONTENT_TYPE).unwrap(),
        "image/png"
    );
    let frame_bytes = frame_response.into_body().collect().await.unwrap().to_bytes();
    assert!(!frame_bytes.is_empty());
}

fn valid_sequence_json() -> serde_json::Value {
    json!({
        "canvas": {
            "width": 320,
            "height": 180,
            "background": [0, 0, 0, 255]
        },
        "timeline": {
            "fps": { "num": 30, "den": 1 },
            "duration": { "value": 1, "timescale": 30 }
        },
        "assets": [],
        "tracks": [{
            "id": "track_text",
            "kind": "text",
            "clips": [{
                "id": "clip_text_1",
                "start": { "value": 0, "timescale": 30 },
                "duration": { "value": 1, "timescale": 30 },
                "opacity": 1.0,
                "blend_mode": "normal",
                "transform": { "x": 0.0, "y": 0.0, "width": null, "height": null },
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
    })
}
