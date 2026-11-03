mod auth;
mod errors;
mod routes;
mod state;
mod ws;

use std::{net::SocketAddr, num::NonZeroUsize};

use anyhow::bail;
use axum::{Router, extract::DefaultBodyLimit, middleware, routing::get, routing::post};
use clap::Parser;

use self::{
    auth::require_token,
    routes::{
        create_render, get_render, get_render_artifact, get_render_progress, health, render_socket,
    },
    state::AppState,
};

/// Largest `POST /renders` body. Compositions reference media through the manifest, so they stay
/// small.
pub(crate) const MAX_REQUEST_BODY_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Parser)]
#[command(version, about = "Run a provider-neutral Lumen render HTTP server.")]
pub struct Args {
    /// Address to bind the HTTP server to.
    #[arg(long, env = "LUMEN_SERVER_BIND", default_value = "127.0.0.1:8080")]
    pub bind: SocketAddr,

    /// Bearer token required on every route except /health. A non-loopback --bind requires one.
    #[arg(long, env = "LUMEN_SERVER_TOKEN")]
    pub token: Option<String>,

    /// Renders that may run at once. Later renders stay queued until a slot frees.
    #[arg(long, env = "LUMEN_SERVER_MAX_CONCURRENT_RENDERS", default_value = "1")]
    pub max_concurrent_renders: NonZeroUsize,

    /// Finished renders kept in memory. Beyond this, the oldest record and artifact are dropped.
    #[arg(long, env = "LUMEN_SERVER_RETAINED_RENDERS", default_value = "16")]
    pub retained_renders: NonZeroUsize,

    /// Media host allowed to resolve to non-public addresses, as written in media URLs. Repeatable.
    #[arg(
        long = "allow-media-host",
        env = "LUMEN_SERVER_ALLOW_MEDIA_HOSTS",
        value_delimiter = ','
    )]
    pub allowed_media_hosts: Vec<String>,

    /// Minimum progress increase, from 0 to 1, before sockets get another progress event.
    #[arg(
        long,
        env = "LUMEN_SERVER_PROGRESS_MIN_DELTA",
        default_value_t = 0.02,
        value_parser = parse_fraction
    )]
    pub progress_min_delta: f32,
}

fn parse_fraction(raw: &str) -> Result<f32, String> {
    match raw.parse::<f32>() {
        Ok(value) if (0.0..=1.0).contains(&value) => Ok(value),
        _ => Err(format!("expected a number from 0 to 1, got `{raw}`")),
    }
}

pub async fn run(mut args: Args) -> anyhow::Result<()> {
    init_tracing();
    args.token.take_if(|token| token.is_empty());
    if args.token.is_none() && !args.bind.ip().to_canonical().is_loopback() {
        bail!(
            "refusing to serve {} without a token: set --token or LUMEN_SERVER_TOKEN, or bind a loopback address",
            args.bind
        );
    }
    let bind = args.bind;
    let app = router(AppState::new(args)?);

    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(address = %listener.local_addr()?, "lumen render server listening");
    axum::serve(listener, app).await?;

    Ok(())
}

fn router(state: AppState) -> Router {
    Router::new()
        .route("/renders", post(create_render))
        .route("/renders/{id}", get(get_render))
        .route("/renders/{id}/progress", get(get_render_progress))
        .route("/renders/{id}/socket", get(render_socket))
        .route("/renders/{id}/artifact", get(get_render_artifact))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_token))
        .route("/health", get(health))
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BODY_BYTES))
        .layer(middleware::from_fn(errors::json_errors))
        .with_state(state)
}

fn init_tracing() {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "lumen_server=info,warn".to_string()),
        )
        .with_writer(std::io::stderr)
        .init();
}
