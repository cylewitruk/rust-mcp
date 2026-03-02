//! Integration tests for `symbol.*` MCP tools.

use serde_json::{Value, json};

use super::common;

#[tokio::test]
async fn symbol_search_returns_seeded_symbol() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    let response = context
        .mcp
        .call_tool(
            "symbol_search",
            json!({
                "query": "from_str",
                "crate_name": "serde_json",
                "limit": 10
            }),
        )
        .await
        .expect("symbol_search call failed");
    let payload = common::structured_content(&response);

    assert_eq!(
        payload
            .get("count")
            .and_then(Value::as_u64),
        Some(1)
    );

    let first_hit = payload
        .get("hits")
        .and_then(Value::as_array)
        .and_then(|hits| hits.first())
        .expect("expected one symbol hit");
    assert_eq!(
        first_hit
            .get("name")
            .and_then(Value::as_str),
        Some("from_str")
    );
    assert_eq!(
        first_hit
            .get("source_path")
            .and_then(Value::as_str),
        Some("src/lib.rs")
    );
}
