use serde_json::{Value, json};

use super::{SEEDED_CRATE_NAME, call_tool_payload, seeded_docs_indexed_context};

#[tokio::test]
async fn tool_docs_search_returns_seeded_docs_hits() {
    let context = seeded_docs_indexed_context().await;

    let payload = call_tool_payload(
        &context.rust_mcp,
        "docs_search",
        json!({
            "query": "parse",
            "crate_name": SEEDED_CRATE_NAME,
            "limit": 10
        }),
    )
    .await;

    assert!(
        payload
            .get("count")
            .and_then(Value::as_u64)
            .is_some_and(|count| count >= 1),
        "expected docs.search count >= 1 for seeded fixtures: {payload}"
    );
    assert_eq!(
        payload
            .get("page")
            .and_then(Value::as_u64),
        Some(1),
        "expected docs.search to return page=1 by default: {payload}"
    );
    assert!(
        payload
            .get("has_more")
            .and_then(Value::as_bool)
            .is_some(),
        "expected docs.search has_more field: {payload}"
    );
    assert!(
        payload
            .get("truncated")
            .and_then(Value::as_bool)
            .is_some(),
        "expected docs.search truncated field: {payload}"
    );
    assert!(
        payload
            .get("next_cursor")
            .is_some(),
        "expected docs.search next_cursor field: {payload}"
    );
    assert_eq!(
        payload
            .get("confidence")
            .and_then(Value::as_str),
        Some("medium")
    );

    let hits = payload
        .get("hits")
        .and_then(Value::as_array)
        .expect("docs_search should return hits array");
    assert!(
        hits.iter().any(|hit| {
            hit.get("crate_name")
                .and_then(Value::as_str)
                == Some(SEEDED_CRATE_NAME)
        }),
        "expected docs.search to include crate {SEEDED_CRATE_NAME}: {hits:?}"
    );
    assert!(
        hits.iter().any(|hit| {
            hit.get("path")
                .and_then(Value::as_str)
                .is_some_and(|path| path.contains("demo-crate/1.2.3/demo-crate/fn.parse.html"))
        }),
        "expected docs.search to include seeded parse docs page path: {hits:?}"
    );
    assert!(
        hits.iter().any(|hit| {
            hit.get("snippet")
                .and_then(Value::as_str)
                .is_some_and(|snippet| {
                    snippet
                        .to_ascii_lowercase()
                        .contains("parse fixture")
                })
        }),
        "expected docs.search snippet to contain seeded docs text: {hits:?}"
    );
}
