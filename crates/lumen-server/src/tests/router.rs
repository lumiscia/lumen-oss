use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use serde_json::json;
use tokio::time::{Duration, sleep};
use tower::ServiceExt;

use crate::{app_state::AppState, endpoint, worker};

#[tokio::test]
async fn unauthorized_request_is_rejected() {
    let app = endpoint::build_router(AppState::with_defaults("secret".to_string()));

    let request = Request::builder()
        .method("POST")
        .uri("/renders")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(valid_project_json().to_string()))
        .expect("request");

    let response = app.oneshot(request).await.expect("response");

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
        .body(Body::from(valid_project_json().to_string()))
        .expect("request");

    let response = app.oneshot(request).await.expect("response");

    let status = response.status();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    assert_eq!(
        status,
        StatusCode::ACCEPTED,
        "unexpected response body: {}",
        String::from_utf8_lossy(&body)
    );

    let json: serde_json::Value = serde_json::from_slice(&body).expect("json");

    assert_eq!(json["state"], "queued");
    assert!(
        json["job_id"]
            .as_str()
            .expect("job id")
            .starts_with("render_")
    );
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
        .body(Body::from(valid_project_json().to_string()))
        .expect("request");

    let response = app.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    assert_eq!(
        status,
        StatusCode::ACCEPTED,
        "unexpected response body: {}",
        String::from_utf8_lossy(&body)
    );
    let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
    let job_id = json["job_id"].as_str().expect("job id").to_string();

    let mut completed = false;
    for _ in 0..80 {
        let request = Request::builder()
            .method("GET")
            .uri(format!("/renders/{job_id}"))
            .header(header::AUTHORIZATION, "Bearer secret")
            .body(Body::empty())
            .expect("request");
        let response = app.clone().oneshot(request).await.expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).expect("json");

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
        .expect("request");
    let artifact_response = app
        .clone()
        .oneshot(artifact_request)
        .await
        .expect("response");
    assert_eq!(artifact_response.status(), StatusCode::OK);
    assert_eq!(
        artifact_response
            .headers()
            .get(header::CONTENT_TYPE)
            .expect("content type"),
        "video/mp4"
    );
    let artifact_bytes = artifact_response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    assert!(!artifact_bytes.is_empty());

    let frame_request = Request::builder()
        .method("GET")
        .uri(format!("/renders/{job_id}/frames/0"))
        .header(header::AUTHORIZATION, "Bearer secret")
        .body(Body::empty())
        .expect("request");
    let frame_response = app.clone().oneshot(frame_request).await.expect("response");
    assert_eq!(frame_response.status(), StatusCode::OK);
    assert_eq!(
        frame_response
            .headers()
            .get(header::CONTENT_TYPE)
            .expect("content type"),
        "image/png"
    );
    let frame_bytes = frame_response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    assert!(!frame_bytes.is_empty());
}

#[tokio::test]
async fn list_cancel_and_retry_flow() {
    let app = endpoint::build_router(AppState::with_defaults("secret".to_string()));

    let create_request = Request::builder()
        .method("POST")
        .uri("/renders")
        .header(header::AUTHORIZATION, "Bearer secret")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(valid_project_json().to_string()))
        .expect("request");
    let create_response = app.clone().oneshot(create_request).await.expect("response");
    assert_eq!(create_response.status(), StatusCode::ACCEPTED);
    let create_body = create_response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let created: serde_json::Value = serde_json::from_slice(&create_body).expect("json");
    let job_id = created["job_id"].as_str().expect("job id").to_string();

    let list_request = Request::builder()
        .method("GET")
        .uri("/renders?state=queued")
        .header(header::AUTHORIZATION, "Bearer secret")
        .body(Body::empty())
        .expect("request");
    let list_response = app.clone().oneshot(list_request).await.expect("response");
    assert_eq!(list_response.status(), StatusCode::OK);
    let list_body = list_response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let listed: serde_json::Value = serde_json::from_slice(&list_body).expect("json");
    assert!(
        listed["items"]
            .as_array()
            .expect("array")
            .iter()
            .any(|item| item["job_id"] == job_id)
    );

    let cancel_request = Request::builder()
        .method("POST")
        .uri(format!("/renders/{job_id}/cancel"))
        .header(header::AUTHORIZATION, "Bearer secret")
        .body(Body::empty())
        .expect("request");
    let cancel_response = app.clone().oneshot(cancel_request).await.expect("response");
    assert_eq!(cancel_response.status(), StatusCode::OK);
    let cancel_body = cancel_response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let canceled: serde_json::Value = serde_json::from_slice(&cancel_body).expect("json");
    assert_eq!(canceled["state"], "canceled");

    let retry_request = Request::builder()
        .method("POST")
        .uri(format!("/renders/{job_id}/retry"))
        .header(header::AUTHORIZATION, "Bearer secret")
        .body(Body::empty())
        .expect("request");
    let retry_response = app.clone().oneshot(retry_request).await.expect("response");
    assert_eq!(retry_response.status(), StatusCode::ACCEPTED);
    let retry_body = retry_response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let retried: serde_json::Value = serde_json::from_slice(&retry_body).expect("json");
    assert_eq!(retried["state"], "queued");
    assert_eq!(retried["job_id"], job_id);
}

#[tokio::test]
async fn render_events_endpoint_returns_sse_stream() {
    let app = endpoint::build_router(AppState::with_defaults("secret".to_string()));

    let create_request = Request::builder()
        .method("POST")
        .uri("/renders")
        .header(header::AUTHORIZATION, "Bearer secret")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(valid_project_json().to_string()))
        .expect("request");
    let create_response = app.clone().oneshot(create_request).await.expect("response");
    assert_eq!(create_response.status(), StatusCode::ACCEPTED);
    let create_body = create_response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let created: serde_json::Value = serde_json::from_slice(&create_body).expect("json");
    let job_id = created["job_id"].as_str().expect("job id").to_string();

    let events_request = Request::builder()
        .method("GET")
        .uri(format!("/renders/{job_id}/events"))
        .header(header::AUTHORIZATION, "Bearer secret")
        .body(Body::empty())
        .expect("request");
    let events_response = app.clone().oneshot(events_request).await.expect("response");
    assert_eq!(events_response.status(), StatusCode::OK);
    let content_type = events_response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    assert!(
        content_type.starts_with("text/event-stream"),
        "unexpected content type: {content_type}"
    );
}

fn valid_project_json() -> serde_json::Value {
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
