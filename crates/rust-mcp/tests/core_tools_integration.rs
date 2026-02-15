//! Integration tests for namespace-free MCP tools.

mod common;

use serde_json::json;

#[tokio::test]
async fn ping_tool_returns_db_ready() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    let response = context
        .mcp
        .call_tool("ping", json!({"message": "integration"}))
        .await
        .expect("ping call failed");
    let text = common::first_content_text(&response);

    assert!(text.contains("pong (db_ready) integration"));
}
