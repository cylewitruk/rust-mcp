use serde_json::{Value, json};

use super::{
    SEEDED_CRATE_NAME, SEEDED_CRATE_NEXT_VERSION, call_tool_payload, refresh_seeded_rustdoc_json,
    seeded_indexed_context, seeded_initialized_context, sync_seeded_demo_crates,
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
