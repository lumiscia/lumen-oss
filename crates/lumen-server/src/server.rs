use std::env;

use crate::{app_state::AppState, endpoint, worker};
use anyhow::{Context, anyhow};
use tokio::net::TcpListener;
use tracing::info;

pub async fn run_server() -> anyhow::Result<()> {
    let host = match env::var("HOST") {
        Ok(host) => host,
        Err(env::VarError::NotPresent) => "0.0.0.0".to_string(),
        Err(err) => return Err(err.into()),
    };

    let port: u16 = match env::var("PORT") {
        Ok(port) => match port.parse() {
            Ok(port) => port,
            Err(err) => return Err(err.into()),
        },
        Err(env::VarError::NotPresent) => 8080,
        Err(err) => return Err(err.into()),
    };

    let secret: String = match env::var("SECRET") {
        Ok(secret) => secret,
        Err(env::VarError::NotPresent) => {
            return Err(anyhow!("Missing SECRET environment variable"));
        }
        Err(err) => return Err(err.into()),
    };

    let listener = TcpListener::bind((host.as_str(), port))
        .await
        .with_context(|| format!("failed to bind server listener on {host}:{port}"))?;
    let addr = listener
        .local_addr()
        .context("failed to read local server address")?;

    info!(%addr, "started lumen-server");

    let state = AppState::with_defaults(secret);
    worker::spawn_render_worker(state.clone());
    let app = endpoint::build_router(state);

    axum::serve(listener, app).await?;

    Ok(())
}
