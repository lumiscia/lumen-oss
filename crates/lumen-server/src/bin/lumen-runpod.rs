use std::io::{self, Read};

use lumen_server::runpod::{RunpodJobRequest, handle_runpod_request, worker};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "lumen_server=info,warn".to_string()),
        )
        .with_writer(std::io::stderr)
        .init();

    if worker::is_runpod_serverless() {
        return worker::run_serverless_worker().await;
    }

    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;
    let request: RunpodJobRequest = serde_json::from_str(&input)?;
    let response = handle_runpod_request(request).await;
    println!("{}", serde_json::to_string(&response)?);

    Ok(())
}
