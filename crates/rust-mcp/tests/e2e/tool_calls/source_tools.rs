use serde_json::{Value, json};

use super::{
    SEEDED_CRATE_NAME, SEEDED_CRATE_VERSION, SEEDED_RUSTDOC_PATH, call_tool_payload,
    call_tool_response, seeded_indexed_context,
};

#[tokio::test]
async fn tool_source_search_returns_well_formed_response() {
    let context = seeded_indexed_context().await;

    let payload = call_tool_payload(
        &context.rust_mcp,
        "source.search",
        json!({
            "query": "parse",
            "crate_name": SEEDED_CRATE_NAME,
            "version": SEEDED_CRATE_VERSION,
            "limit": 10
        }),
    )
    .await;

    // Verify envelope fields are present regardless of hit count.
    assert!(
        payload
            .get("count")
            .and_then(Value::as_u64)
            .is_some(),
        "expected source.search to return count: {payload}"
    );
    assert_eq!(
        payload
            .get("page")
            .and_then(Value::as_u64),
        Some(1),
        "expected source.search to return page=1 by default: {payload}"
    );
    assert!(
        payload
            .get("has_more")
            .and_then(Value::as_bool)
            .is_some(),
        "expected source.search has_more field: {payload}"
    );
    assert!(
        payload
            .get("truncated")
            .and_then(Value::as_bool)
            .is_some(),
        "expected source.search truncated field: {payload}"
    );
    assert!(
        payload
            .get("next_cursor")
            .is_some(),
        "expected source.search next_cursor field: {payload}"
    );
    assert!(
        payload
            .get("hits")
            .and_then(Value::as_array)
            .is_some(),
        "source.search should return hits array: {payload}"
    );
}

#[tokio::test]
async fn tool_source_read_returns_error_for_contentless_file() {
    let context = seeded_indexed_context().await;

    // Rustdoc JSON entries no longer store raw content, so reading one should
    // produce a tool-level error explaining the content is not available.
    let response = call_tool_response(
        &context.rust_mcp,
        "source.read",
        json!({
            "crate_name": SEEDED_CRATE_NAME,
            "version": SEEDED_CRATE_VERSION,
            "path": SEEDED_RUSTDOC_PATH,
            "start_line": 1,
            "end_line": 200
        }),
    )
    .await;

    let result = response
        .get("result")
        .expect("source.read should return a result");
    assert_eq!(
        result
            .get("isError")
            .and_then(Value::as_bool),
        Some(true),
        "expected source.read isError=true for contentless rustdoc_json path: {result}"
    );
}

#[tokio::test]
async fn tool_source_context_returns_error_for_contentless_file() {
    let context = seeded_indexed_context().await;

    let response = call_tool_response(
        &context.rust_mcp,
        "source.context",
        json!({
            "crate_name": SEEDED_CRATE_NAME,
            "version": SEEDED_CRATE_VERSION,
            "path": SEEDED_RUSTDOC_PATH,
            "line": 1
        }),
    )
    .await;

    let result = response
        .get("result")
        .expect("source.context should return a result");
    assert_eq!(
        result
            .get("isError")
            .and_then(Value::as_bool),
        Some(true),
        "expected source.context isError=true for contentless rustdoc_json path: {result}"
    );
}
