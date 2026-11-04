mod server;

use clap::Parser;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    lumen_server::render::font_dir_from_env()?;
    server::http::run(server::http::Args::parse()).await
}
