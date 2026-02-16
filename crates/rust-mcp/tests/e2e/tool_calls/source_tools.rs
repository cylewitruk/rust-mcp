use serde_json::{Value, json};

use super::{
    SEEDED_CRATE_NAME, SEEDED_CRATE_VERSION, SEEDED_RUSTDOC_PATH, call_tool_payload,
    seeded_indexed_context,
};

#[tokio::test]
async fn tool_source_search_returns_seeded_path() {
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

    assert!(
        payload
            .get("count")
            .and_then(Value::as_u64)
            .is_some_and(|count| count >= 1),
        "expected source.search count >= 1 for seeded fixtures: {payload}"
    );

    let hits = payload
        .get("hits")
        .and_then(Value::as_array)
        .expect("source.search should return hits array");
    assert!(
        hits.iter().any(|hit| {
            hit.get("path")
                .and_then(Value::as_str)
                == Some(SEEDED_RUSTDOC_PATH)
        }),
        "expected source.search hits to include {SEEDED_RUSTDOC_PATH}: {hits:?}"
    );
}

#[tokio::test]
async fn tool_source_read_returns_seeded_content() {
    let context = seeded_indexed_context().await;

    let payload = call_tool_payload(
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

    assert_eq!(
        payload
            .get("crate_name")
            .and_then(Value::as_str),
        Some(SEEDED_CRATE_NAME)
    );
    assert_eq!(
        payload
            .get("path")
            .and_then(Value::as_str),
        Some(SEEDED_RUSTDOC_PATH)
    );
    assert!(
        payload
            .get("content")
            .and_then(Value::as_str)
            .is_some_and(|content| content.contains("parse")),
        "expected source.read content to include parse: {payload}"
    );
}

#[tokio::test]
async fn tool_source_context_resolves_location() {
    let context = seeded_indexed_context().await;

    let payload = call_tool_payload(
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

    assert_eq!(
        payload
            .get("path")
            .and_then(Value::as_str),
        Some(SEEDED_RUSTDOC_PATH)
    );
    assert_eq!(
        payload
            .get("line")
            .and_then(Value::as_u64),
        Some(1)
    );
    assert!(
        payload
            .get("module_path")
            .and_then(Value::as_str)
            .is_some_and(|module_path| !module_path.trim().is_empty()),
        "expected source.context to return a non-empty module_path: {payload}"
    );
}
