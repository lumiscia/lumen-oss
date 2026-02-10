use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use serde_json::json;
use tower::ServiceExt;

use crate::{app_state::AppState, endpoint};

#[tokio::test]
async fn unauthorized_request_is_rejected() {
    let app = endpoint::build_router(AppState::with_defaults("secret".to_string()));

    let request = Request::builder()
        .method("POST")
        .uri("/renders")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({ "sequence": "noop" }).to_string()))
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
        .body(Body::from(json!({ "sequence": "noop" }).to_string()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::ACCEPTED);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["state"], "queued");
    assert!(json["job_id"].as_str().unwrap().starts_with("render_"));
}
