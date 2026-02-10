use std::{env, net::Ipv4Addr};

use crate::{app_state::AppState, endpoint};
use anyhow::anyhow;
use tokio::net::TcpListener;
use tracing::info;

pub async fn serve() -> anyhow::Result<()> {
    let host: Ipv4Addr = match env::var("HOST") {
        Ok(host) => match host.parse() {
            Ok(host) => host,
            Err(err) => return Err(err.into()),
        },
        Err(env::VarError::NotPresent) => Ipv4Addr::new(0, 0, 0, 0),
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

    let listener = TcpListener::bind((host, port)).await?;

    info!("Started server at {}:{}", host, port);

    let state = AppState::with_defaults(secret);
    let app = endpoint::build_router(state);

    axum::serve(listener, app).await?;

    Ok(())
}
