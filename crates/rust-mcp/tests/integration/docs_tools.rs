//! Integration tests for `docs.*` MCP tools.

use serde_json::{Value, json};

use super::common;

#[tokio::test]
async fn docs_search_returns_seeded_docs_page() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    let response = context
        .mcp
        .call_tool(
            "docs.search",
            json!({
                "query": "Deserialize",
                "crate_name": "serde_json",
                "limit": 10
            }),
        )
        .await
        .expect("docs.search call failed");
    let payload = common::structured_content(&response);

    assert_eq!(
        payload
            .get("count")
            .and_then(Value::as_u64),
        Some(1)
    );
    assert_eq!(
        payload
            .get("page")
            .and_then(Value::as_u64),
        Some(1)
    );
    assert_eq!(
        payload
            .get("has_more")
            .and_then(Value::as_bool),
        Some(false)
    );
    assert_eq!(
        payload
            .get("truncated")
            .and_then(Value::as_bool),
        Some(false)
    );
    assert!(
        payload
            .get("next_cursor")
            .is_some(),
        "docs.search response should include next_cursor field"
    );
    let first_hit = payload
        .get("hits")
        .and_then(Value::as_array)
        .and_then(|hits| hits.first())
        .expect("expected one docs.search hit");
    assert_eq!(
        first_hit
            .get("crate_name")
            .and_then(Value::as_str),
        Some("serde_json")
    );
    assert_eq!(
        first_hit
            .get("path")
            .and_then(Value::as_str),
        Some("/serde_json/fn.from_str.html")
    );
}
