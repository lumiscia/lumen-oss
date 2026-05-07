use std::{env, net::SocketAddr};

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use lumen_server::server::{RenderJobInput, RenderJobResponse, handle_render_job};
use serde::Serialize;

#[derive(Clone)]
struct AppState {
    api_token: Option<String>,
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    ok: bool,
    service: &'static str,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            env::var("RUST_LOG")
                .unwrap_or_else(|_| "lumen_server=info,lumen_vast=info,warn".to_string()),
        )
        .with_writer(std::io::stderr)
        .init();

    let bind = env::var("LUMEN_VAST_BIND").unwrap_or_else(|_| "0.0.0.0:8080".to_string());
    let address: SocketAddr = bind.parse()?;
    let state = AppState {
        api_token: env::var("LUMEN_VAST_API_TOKEN").ok(),
    };
    let app = Router::new()
        .route("/health", get(health))
        .route("/render", post(render))
        .with_state(state);

    tracing::info!(%address, "starting Vast render server");
    let listener = tokio::net::TcpListener::bind(address).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        ok: true,
        service: "lumen-vast",
    })
}

async fn render(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(input): Json<RenderJobInput>,
) -> impl IntoResponse {
    if let Some(expected_token) = state.api_token.as_deref() {
        let actual_token = headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "));
        if actual_token != Some(expected_token) {
            return (
                StatusCode::UNAUTHORIZED,
                Json(RenderJobResponse {
                    ok: false,
                    artifact: None,
                    metrics: None,
                    error: Some(lumen_server::server::RenderJobError {
                        code: "unauthorized".to_string(),
                        message: "invalid Vast render API token".to_string(),
                        retryable: false,
                    }),
                }),
            );
        }
    }

    (StatusCode::OK, Json(handle_render_job(input).await))
}
