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
        .body(Body::from(valid_project_json().to_string()))
        .expect("request")
}

fn valid_project_json() -> serde_json::Value {
    json!({
        "schema_revision": "lumen_graph_v1",
        "timeline": {
            "fps": 30.0,
            "duration_frames": 2
        },
        "render_settings": {
            "width": 320,
            "height": 180,
            "background_color": [0, 0, 0, 255]
        },
        "graph": {
            "nodes": [
                {
                    "id": 1,
                    "kind": {
                        "type": "solid_color",
                        "color": [255, 255, 255, 255],
                        "width": 320,
                        "height": 180
                    }
                },
                {
                    "id": 2,
                    "kind": { "type": "media_output" }
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
        }
    })
}
