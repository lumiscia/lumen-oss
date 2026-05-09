use std::net::SocketAddr;

mod server;

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use clap::Parser;
use serde::Serialize;

use self::server::{RenderJobError, RenderJobInput, RenderJobResponse, handle_render_job};

#[derive(Debug, Parser)]
#[command(version, about = "Run a provider-neutral Lumen render HTTP server.")]
struct Args {
    /// Address to bind the HTTP server to.
    #[arg(long, env = "LUMEN_SERVER_BIND", default_value = "127.0.0.1:8080")]
    bind: SocketAddr,

    /// Optional bearer token required for render requests.
    #[arg(long, env = "LUMEN_SERVER_TOKEN")]
    token: Option<String>,
}

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
    let args = Args::parse();
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "lumen_server=info,warn".to_string()),
        )
        .with_writer(std::io::stderr)
        .init();

    let state = AppState {
        api_token: args.token,
    };
    let app = Router::new()
        .route("/health", get(health))
        .route("/render", post(render))
        .with_state(state);

    tracing::info!(address = %args.bind, "starting lumen render server");
    let listener = tokio::net::TcpListener::bind(args.bind).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        ok: true,
        service: "lumen-server",
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
                    error: Some(RenderJobError {
                        code: "unauthorized".to_string(),
                        message: "invalid render API token".to_string(),
                        retryable: false,
                    }),
                }),
            );
        }
    }

    (StatusCode::OK, Json(handle_render_job(input).await))
}
