use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use serde_json::json;
use tower::ServiceExt;

use crate::{app_state::AppState, endpoint};

#[tokio::test]
async fn rejects_missing_authorization_header() {
    let app = endpoint::build_router(AppState::with_defaults("secret".to_string()));
    let response = app
        .oneshot(valid_render_request(None))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn rejects_non_bearer_authorization_scheme() {
    let app = endpoint::build_router(AppState::with_defaults("secret".to_string()));
    let response = app
        .oneshot(valid_render_request(Some("Basic secret")))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn rejects_empty_bearer_token() {
    let app = endpoint::build_router(AppState::with_defaults("secret".to_string()));
    let response = app
        .oneshot(valid_render_request(Some("Bearer ")))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn rejects_invalid_bearer_token() {
    let app = endpoint::build_router(AppState::with_defaults("secret".to_string()));
    let response = app
        .oneshot(valid_render_request(Some("Bearer wrong")))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn accepts_valid_bearer_token() {
    let app = endpoint::build_router(AppState::with_defaults("secret".to_string()));
    let response = app
        .oneshot(valid_render_request(Some("Bearer secret")))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::ACCEPTED);
}

fn valid_render_request(auth_header: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/renders")
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(auth_header) = auth_header {
        builder = builder.header(header::AUTHORIZATION, auth_header);
    }

    builder
        .body(Body::from(valid_sequence_json().to_string()))
        .expect("request")
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
