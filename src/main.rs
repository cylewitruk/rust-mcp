#![allow(missing_docs)]
#![doc = include_str!("../README.md")]

/// Main entry point for the Rust MCP server application.
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    rust_mcp::app::run().await
}
