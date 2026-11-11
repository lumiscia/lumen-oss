mod server;

use clap::Parser;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    lumen_ffmpeg::raise_open_file_limit();
    server::http::run(server::http::Args::parse()).await
}
