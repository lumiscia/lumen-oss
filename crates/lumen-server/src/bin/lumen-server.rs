mod server;

use clap::Parser;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    server::http::run(server::http::Args::parse()).await
}
