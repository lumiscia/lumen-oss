#[tokio::main]
async fn main() -> anyhow::Result<()> {
    lumen_server::server::run_server().await
}
