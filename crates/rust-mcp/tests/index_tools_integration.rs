//! Integration tests for `index.*` MCP tools.

mod common;

use serde_json::{Value, json};

#[tokio::test]
async fn index_status_reports_seeded_coverage_counts() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    let response = context
        .mcp
        .call_tool("index.status", json!({}))
        .await
        .expect("index.status call failed");
    let payload = common::structured_content(&response);

    let coverage = payload
        .get("coverage")
        .and_then(Value::as_object)
        .expect("coverage should be an object");

    assert_eq!(
        coverage
            .get("crates")
            .and_then(Value::as_i64),
        Some(3)
    );
    assert_eq!(
        coverage
            .get("crate_versions")
            .and_then(Value::as_i64),
        Some(3)
    );
    assert_eq!(
        coverage
            .get("dependency_edges")
            .and_then(Value::as_i64),
        Some(2)
    );
    assert_eq!(
        coverage
            .get("source_files")
            .and_then(Value::as_i64),
        Some(1)
    );
}

#[tokio::test]
async fn index_refresh_local_cache_returns_terminal_status() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    let response = context
        .mcp
        .call_tool(
            "index.refresh",
            json!({
                "scope": "local_cache",
                "crate_name": "serde_json"
            }),
        )
        .await
        .expect("index.refresh call failed");
    let payload = common::structured_content(&response);

    assert_eq!(
        payload
            .get("scope")
            .and_then(Value::as_str),
        Some("local_cache")
    );
    assert_eq!(
        payload
            .get("accepted")
            .and_then(Value::as_bool),
        Some(true)
    );

    let status = payload
        .get("status")
        .and_then(Value::as_str)
        .expect("status should be a string");
    assert!(matches!(status, "completed" | "completed_with_errors"));
}
