mod auth;
mod errors;
mod progress;
mod routes;
mod state;
mod ws;

use std::{net::SocketAddr, num::NonZeroUsize};

use anyhow::bail;
use axum::{Router, middleware, routing::get, routing::post};
use clap::Parser;

use self::{
    auth::require_token,
    routes::{
        create_render, get_render, get_render_artifact, get_render_progress, health, render_socket,
    },
    state::AppState,
};

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

    /// Minimum progress delta required before broadcasting another non-terminal progress update.
    #[arg(long, env = "LUMEN_SERVER_PROGRESS_MIN_DELTA", default_value_t = 0.02)]
    pub progress_min_delta: f32,

    /// Emit verbose per-frame renderer diagnostics.
    #[arg(long, env = "LUMEN_SERVER_VERBOSE_DEBUG", default_value_t = false)]
    pub verbose_debug: bool,
}

pub async fn run(mut args: Args) -> anyhow::Result<()> {
    init_tracing(args.verbose_debug);
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
        .with_state(state)
}

fn init_tracing(verbose_debug: bool) {
    let default_filter = if verbose_debug {
        "lumen_server=debug,warn"
    } else {
        "lumen_server=info,warn"
    };
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| default_filter.to_string()))
        .with_writer(std::io::stderr)
        .init();
}
