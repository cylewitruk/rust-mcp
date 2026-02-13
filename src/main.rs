#[tokio::main]
async fn main() -> anyhow::Result<()> {
    rust_mcp::app::run().await
}
