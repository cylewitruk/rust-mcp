use serde_json::{Value, json};

use super::{
    SEEDED_CRATE_NAME, SEEDED_CRATE_VERSION, SEEDED_RUSTDOC_PATH, call_tool_payload,
    seeded_indexed_context,
};

#[tokio::test]
async fn tool_symbol_search_returns_seeded_parse_symbol() {
    let context = seeded_indexed_context().await;

    let payload = call_tool_payload(
        &context.rust_mcp,
        "symbol.search",
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
        "expected symbol.search count >= 1 for seeded fixtures: {payload}"
    );

    let hits = payload
        .get("hits")
        .and_then(Value::as_array)
        .expect("symbol.search should return hits array");
    assert!(
        hits.iter().any(|hit| {
            hit.get("name")
                .and_then(Value::as_str)
                == Some("parse")
                && hit
                    .get("source_path")
                    .and_then(Value::as_str)
                    == Some(SEEDED_RUSTDOC_PATH)
        }),
        "expected symbol.search to include parse from seeded rustdoc path: {hits:?}"
    );
}
