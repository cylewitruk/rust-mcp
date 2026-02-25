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
        .call_tool("schema.get", json!({"tool_name": "ping"}))
        .await
        .expect("schema.get call failed");
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
        .expect("schema.get should return one schema entry");
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
