use super::{Value, common, json};

/// `mode: "contains"` is an alias for `"text"`. Verify it is accepted and
/// produces the same results as the canonical mode value.
#[tokio::test]
async fn source_search_accepts_contains_as_mode_alias() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    let response = context
        .mcp
        .call_tool(
            "source_search",
            json!({
                "query": "from_str",
                "crate_name": "serde_json",
                "mode": "contains",
                "limit": 10
            }),
        )
        .await
        .expect("source_search with mode='contains' should succeed");
    let payload = common::structured_content(&response);

    assert!(
        payload
            .get("count")
            .and_then(Value::as_u64)
            .unwrap_or_default()
            >= 1,
        "mode='contains' should find the same results as mode='text'"
    );
    // The canonical mode value in the response should be "text", not "contains".
    assert_eq!(
        payload
            .get("mode")
            .and_then(Value::as_str),
        Some("text"),
        "response mode should normalize to 'text'"
    );
}

#[tokio::test]
async fn source_search_and_read_return_seeded_source_content() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    let search_response = context
        .mcp
        .call_tool(
            "source_search",
            json!({
                "query": "from_str",
                "crate_name": "serde_json",
                "limit": 10
            }),
        )
        .await
        .expect("source_search call failed");
    let search_payload = common::structured_content(&search_response);
    assert_eq!(
        search_payload
            .get("count")
            .and_then(Value::as_u64),
        Some(1)
    );
    assert_eq!(
        search_payload
            .get("page")
            .and_then(Value::as_u64),
        Some(1)
    );
    assert_eq!(
        search_payload
            .get("has_more")
            .and_then(Value::as_bool),
        Some(false)
    );
    assert_eq!(
        search_payload
            .get("truncated")
            .and_then(Value::as_bool),
        Some(false)
    );
    assert!(
        search_payload
            .get("next_cursor")
            .is_some(),
        "source_search response should include next_cursor field"
    );

    let read_response = context
        .mcp
        .call_tool(
            "source_read",
            json!({
                "crate_name": "serde_json",
                "path": "src/lib.rs",
                "start_line": 1,
                "end_line": 1
            }),
        )
        .await
        .expect("source_read call failed");
    let read_payload = common::structured_content(&read_response);

    assert_eq!(
        read_payload
            .get("crate_name")
            .and_then(Value::as_str),
        Some("serde_json")
    );
    assert_eq!(
        read_payload
            .get("path")
            .and_then(Value::as_str),
        Some("src/lib.rs")
    );
    assert!(
        read_payload
            .get("content")
            .and_then(Value::as_str)
            .expect("content should be a string")
            .contains("pub fn from_str")
    );
}
