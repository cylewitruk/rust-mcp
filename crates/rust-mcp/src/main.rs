#![allow(missing_docs)]
#![doc = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../README.md"))]

/// Main entry point for the Rust MCP server application.
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    rust_mcp::app::run().await
}
