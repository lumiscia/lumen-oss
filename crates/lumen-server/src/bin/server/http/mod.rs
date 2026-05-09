mod auth;
mod errors;
mod progress;
mod routes;
mod state;
mod ws;

use std::net::SocketAddr;

use axum::{Router, routing::get, routing::post};
use clap::Parser;

use self::{
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

    /// Optional bearer token required for render requests.
    #[arg(long, env = "LUMEN_SERVER_TOKEN")]
    pub token: Option<String>,

    /// Minimum progress delta required before broadcasting another non-terminal progress update.
    #[arg(long, env = "LUMEN_SERVER_PROGRESS_MIN_DELTA", default_value_t = 0.02)]
    pub progress_min_delta: f32,

    /// Emit verbose per-frame renderer diagnostics.
    #[arg(long, env = "LUMEN_SERVER_VERBOSE_DEBUG", default_value_t = false)]
    pub verbose_debug: bool,
}

pub async fn run(args: Args) -> anyhow::Result<()> {
    init_tracing(args.verbose_debug);
    let app = router(AppState::new(
        args.token,
        args.progress_min_delta,
        args.verbose_debug,
    ));

    tracing::info!(address = %args.bind, "starting lumen render server");
    let listener = tokio::net::TcpListener::bind(args.bind).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/renders", post(create_render))
        .route("/renders/{id}", get(get_render))
        .route("/renders/{id}/progress", get(get_render_progress))
        .route("/renders/{id}/socket", get(render_socket))
        .route("/renders/{id}/artifact", get(get_render_artifact))
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
