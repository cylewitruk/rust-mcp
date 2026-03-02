use super::{Value, common, json, mock_index_sync_context};

#[tokio::test]
async fn index_status_reports_seeded_coverage_counts() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    let response = context
        .mcp
        .call_tool("index_status", json!({}))
        .await
        .expect("index_status call failed");
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
            "index_refresh",
            json!({
                "scope": "local_cache",
                "crate_name": "serde_json"
            }),
        )
        .await
        .expect("index_refresh call failed");
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

#[tokio::test]
async fn index_sync_crates_writes_versions_features_and_dependencies_from_mock_api() {
    let context = mock_index_sync_context()
        .await
        .expect("failed to build mock index sync context");

    let response = context
        .mcp
        .call_tool(
            "index_sync_crates",
            json!({
                "query": "demo",
                "page": 1,
                "per_page": 10,
                "include_dependencies": true
            }),
        )
        .await
        .expect("index_sync_crates call failed");
    let payload = common::structured_content(&response);

    assert_eq!(
        payload
            .get("total_candidates")
            .and_then(Value::as_u64),
        Some(1)
    );
    assert_eq!(
        payload
            .get("synced_crates")
            .and_then(Value::as_u64),
        Some(1)
    );
    assert_eq!(
        payload
            .get("synced_versions")
            .and_then(Value::as_u64),
        Some(1)
    );
    assert_eq!(
        payload
            .get("synced_dependencies")
            .and_then(Value::as_u64),
        Some(1)
    );
    assert_eq!(
        payload
            .get("errors")
            .and_then(Value::as_array)
            .map(|errors| errors.len()),
        Some(0)
    );
    assert!(
        payload
            .get("selected_versions")
            .and_then(Value::as_array)
            .is_some_and(|versions| {
                versions
                    .iter()
                    .any(|version| version.as_str() == Some("demo_crate@1.2.3"))
            })
    );

    let demo_crate_exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM crates WHERE name = 'demo_crate')",
    )
    .fetch_one(&context.state.db)
    .await
    .expect("failed to verify demo_crate row");
    assert!(demo_crate_exists);

    let feature_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*)::BIGINT
         FROM crate_version_features cvf
         JOIN crate_versions cv ON cv.id = cvf.crate_version_id
         JOIN crates c ON c.id = cv.crate_id
         WHERE c.name = 'demo_crate'",
    )
    .fetch_one(&context.state.db)
    .await
    .expect("failed to count crate_version_features");
    assert_eq!(feature_count, 2);

    let dependency_edge_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*)::BIGINT
         FROM dependency_edges de
         JOIN crate_versions cv ON cv.id = de.from_version_id
         JOIN crates from_c ON from_c.id = cv.crate_id
         JOIN crates to_c ON to_c.id = de.to_crate_id
         WHERE from_c.name = 'demo_crate' AND to_c.name = 'serde'",
    )
    .fetch_one(&context.state.db)
    .await
    .expect("failed to count dependency edges");
    assert_eq!(dependency_edge_count, 1);
}

#[tokio::test]
async fn index_refresh_crate_scope_uses_mock_sync_pipeline() {
    let context = mock_index_sync_context()
        .await
        .expect("failed to build mock index sync context");

    let response = context
        .mcp
        .call_tool(
            "index_refresh",
            json!({
                "scope": "crate",
                "crate_name": "demo_crate",
                "include_dependencies": true
            }),
        )
        .await
        .expect("index_refresh crate-scope call failed");
    let payload = common::structured_content(&response);

    assert_eq!(
        payload
            .get("scope")
            .and_then(Value::as_str),
        Some("crate")
    );
    assert_eq!(
        payload
            .get("status")
            .and_then(Value::as_str),
        Some("completed")
    );

    let result = payload
        .get("result")
        .and_then(Value::as_object)
        .expect("result should be an object");
    assert_eq!(
        result
            .get("synced_crates")
            .and_then(Value::as_u64),
        Some(1)
    );
    assert_eq!(
        result
            .get("synced_versions")
            .and_then(Value::as_u64),
        Some(1)
    );
    assert_eq!(
        result
            .get("synced_dependencies")
            .and_then(Value::as_u64),
        Some(1)
    );
}
