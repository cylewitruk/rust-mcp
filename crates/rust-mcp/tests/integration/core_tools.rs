//! Integration tests for namespace-free MCP tools.

use serde_json::{Value, json};

use super::common;

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

#[tokio::test]
async fn schema_get_returns_contract_for_known_tool() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    let response = context
        .mcp
        .call_tool("schema_get", json!({"tool_name": "ping"}))
        .await
        .expect("schema_get call failed");
    let payload = common::structured_content(&response);

    assert_eq!(
        payload
            .get("total_tools")
            .and_then(Value::as_u64),
        Some(1)
    );

    let first_schema = payload
        .get("schemas")
        .and_then(Value::as_array)
        .and_then(|schemas| schemas.first())
        .expect("schema_get should return one schema entry");
    assert_eq!(
        first_schema
            .get("tool_name")
            .and_then(Value::as_str),
        Some("ping")
    );
    assert_eq!(
        first_schema
            .get("request")
            .and_then(|schema| schema.get("type"))
            .and_then(Value::as_str),
        Some("object")
    );
}

/// Names needing Base64 header encoding must pass rmcp's standard-header
/// validation; the call then fails only because the tool is unknown.
#[tokio::test]
async fn stateless_call_with_encoded_tool_name_passes_header_validation() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    let error = context
        .mcp
        .call_tool("ツール", json!({}))
        .await
        .expect_err("unknown tool should fail")
        .to_string();

    assert!(!error.contains("-32020"), "Mcp-Name header was rejected: {error}");
    assert!(error.contains("ツール"), "expected an unknown-tool error: {error}");
}
