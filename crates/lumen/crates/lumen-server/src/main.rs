use tracing::{error, warn};
use tracing_subscriber::EnvFilter;

use lumen_server::server::serve;

#[tokio::main]
async fn main() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::fmt().with_env_filter(filter).init();

    if cfg!(debug_assertions) {
        warn!(
            "running debug build; render throughput is significantly slower. \
             use `cargo run --release -p lumen-server` for realistic performance"
        );
    }

    if let Err(err) = serve().await {
        error!("{err:?}");
    }
}
