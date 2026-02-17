use serde_json::{Value, json};

use super::{
    SEEDED_CRATE_NAME, SEEDED_CRATE_NEXT_VERSION, TOKIO_FALLBACK_CRATE_NAME,
    TOKIO_FALLBACK_CRATE_VERSION, TOKIO_FALLBACK_LOCAL_SOURCE_PATH, call_tool_payload,
    refresh_seeded_rustdoc_json, seeded_indexed_context, seeded_initialized_context,
    seeded_rustdoc_fallback_initialized_context, sync_seeded_demo_crates,
};

#[tokio::test]
async fn tool_index_sync_crates_syncs_seeded_fixtures() {
    let context = seeded_initialized_context().await;

    let payload = sync_seeded_demo_crates(&context.rust_mcp, true).await;

    assert!(
        payload
            .get("synced_crates")
            .and_then(Value::as_u64)
            .is_some_and(|count| count >= 2),
        "expected sync to ingest seeded crates: {payload}"
    );

    let selected_versions = payload
        .get("selected_versions")
        .and_then(Value::as_array)
        .expect("index.sync_crates should return selected_versions array");
    let expected_primary = format!("{SEEDED_CRATE_NAME}@{SEEDED_CRATE_NEXT_VERSION}");
    assert!(
        selected_versions
            .iter()
            .filter_map(Value::as_str)
            .any(|value| value == expected_primary),
        "expected selected_versions to include {expected_primary}, got: {selected_versions:?}"
    );
}

#[tokio::test]
async fn tool_index_refresh_rustdoc_json_completes_for_seeded_fixtures() {
    let context = seeded_initialized_context().await;
    let _ = sync_seeded_demo_crates(&context.rust_mcp, true).await;

    let payload = refresh_seeded_rustdoc_json(&context.rust_mcp).await;

    assert!(
        matches!(
            payload
                .get("status")
                .and_then(Value::as_str),
            Some("completed") | Some("completed_with_errors")
        ),
        "unexpected index.refresh status payload: {payload}"
    );
}

#[tokio::test]
async fn tool_index_status_reports_seeded_coverage() {
    let context = seeded_indexed_context().await;

    let payload = call_tool_payload(&context.rust_mcp, "index.status", json!({})).await;

    let coverage = payload
        .get("coverage")
        .and_then(Value::as_object)
        .expect("index.status should return coverage object");
    assert!(
        coverage
            .get("crates")
            .and_then(Value::as_i64)
            .unwrap_or_default()
            >= 2,
        "expected at least two crates in coverage: {coverage:?}"
    );
    assert!(
        coverage
            .get("crate_versions")
            .and_then(Value::as_i64)
            .unwrap_or_default()
            >= 3,
        "expected at least three crate versions in coverage: {coverage:?}"
    );
}

#[tokio::test]
async fn tool_index_refresh_rustdoc_json_uses_local_fallback_when_docs_rs_is_unreachable() {
    let context = seeded_rustdoc_fallback_initialized_context().await;

    let sync_payload = call_tool_payload(
        &context.rust_mcp,
        "index.sync_crates",
        json!({
            "query": TOKIO_FALLBACK_CRATE_NAME,
            "page": 1,
            "per_page": 5,
            "include_dependencies": false
        }),
    )
    .await;

    let expected_selected = format!("{TOKIO_FALLBACK_CRATE_NAME}@{TOKIO_FALLBACK_CRATE_VERSION}");
    let selected_versions = sync_payload
        .get("selected_versions")
        .and_then(Value::as_array)
        .expect("index.sync_crates should return selected_versions array");
    assert!(
        selected_versions
            .iter()
            .filter_map(Value::as_str)
            .any(|value| value == expected_selected),
        "expected selected_versions to include {expected_selected}: {sync_payload}"
    );

    let refresh_payload = call_tool_payload(
        &context.rust_mcp,
        "index.refresh",
        json!({
            "scope": "rustdoc_json",
            "crate_name": TOKIO_FALLBACK_CRATE_NAME,
            "page": 1,
            "per_page": 5
        }),
    )
    .await;
    assert!(
        matches!(
            refresh_payload
                .get("status")
                .and_then(Value::as_str),
            Some("completed") | Some("completed_with_errors")
        ),
        "unexpected index.refresh status payload: {refresh_payload}"
    );

    let result = refresh_payload
        .get("result")
        .and_then(Value::as_object)
        .expect("index.refresh should return result object");
    assert!(
        result
            .get("synced_versions")
            .and_then(Value::as_u64)
            .is_some_and(|count| count >= 1),
        "expected at least one rustdoc version to sync via local fallback: {refresh_payload}"
    );

    let symbol_payload = call_tool_payload(
        &context.rust_mcp,
        "symbol.search",
        json!({
            "query": "Runtime",
            "crate_name": TOKIO_FALLBACK_CRATE_NAME,
            "version": TOKIO_FALLBACK_CRATE_VERSION,
            "limit": 20
        }),
    )
    .await;

    let hits = symbol_payload
        .get("hits")
        .and_then(Value::as_array)
        .expect("symbol.search should return hits array");
    assert!(
        hits.iter().any(|hit| {
            hit.get("name")
                .and_then(Value::as_str)
                == Some("Runtime")
                && hit
                    .get("source_path")
                    .and_then(Value::as_str)
                    == Some(TOKIO_FALLBACK_LOCAL_SOURCE_PATH)
        }),
        "expected Runtime symbol from local fallback source path {}: {hits:?}",
        TOKIO_FALLBACK_LOCAL_SOURCE_PATH
    );
}
