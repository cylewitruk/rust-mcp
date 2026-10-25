use super::{
    PINNED_SOURCE_CRATE, PINNED_SOURCE_OLDEST_VERSION, Value, common, json,
    mock_index_sync_context, pinned_source_versions, run_refresh_worker_for_tests,
};

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
async fn index_crates_writes_versions_features_and_dependencies_from_mock_api() {
    let context = mock_index_sync_context()
        .await
        .expect("failed to build mock index sync context");

    // Spawn the refresh worker so enrichment jobs enqueued by index_crates
    // are processed promptly instead of hitting the 45 s on-demand timeout.
    let _worker = tokio::spawn(run_refresh_worker_for_tests(context.state.clone()));

    let response = context
        .mcp
        .call_tool(
            "index_crates",
            json!({
                "crates": [{ "name": "demo_crate" }],
                "include_dependencies": true
            }),
        )
        .await
        .expect("index_crates call failed");
    let payload = common::structured_content(&response);

    assert_eq!(
        payload
            .get("succeeded")
            .and_then(Value::as_u64),
        Some(1)
    );
    assert_eq!(
        payload
            .get("failed")
            .and_then(Value::as_u64),
        Some(0)
    );

    let results = payload
        .get("results")
        .and_then(Value::as_array)
        .expect("index_crates should return results array");
    assert!(results.iter().any(|r| {
        r.get("name")
            .and_then(Value::as_str)
            == Some("demo_crate")
            && r.get("version")
                .and_then(Value::as_str)
                == Some("1.2.3")
            && r.get("error")
                .is_none_or(Value::is_null)
    }));

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
        Some(2),
        "the mock serves demo_crate 1.2.3 and 1.1.0"
    );
    assert_eq!(
        result
            .get("synced_dependencies")
            .and_then(Value::as_u64),
        Some(1)
    );
}

#[tokio::test]
async fn index_crates_honors_pinned_version() {
    let context = mock_index_sync_context()
        .await
        .expect("failed to build mock index sync context");
    let _worker = tokio::spawn(run_refresh_worker_for_tests(context.state.clone()));

    let response = context
        .mcp
        .call_tool(
            "index_crates",
            json!({
                "crates": [{ "name": "demo_crate", "version": "1.1.0" }],
                "include_dependencies": false
            }),
        )
        .await
        .expect("index_crates call failed");
    let payload = common::structured_content(&response);
    assert_eq!(
        payload
            .get("succeeded")
            .and_then(Value::as_u64),
        Some(1),
        "{payload}"
    );
    assert_eq!(
        payload
            .pointer("/results/0/version")
            .and_then(Value::as_str),
        Some("1.1.0")
    );

    let attempted = sqlx::query_scalar::<_, bool>(
        "SELECT cv.rustdoc_last_attempt_at IS NOT NULL
         FROM crate_versions cv
         JOIN crates c ON c.id = cv.crate_id
         WHERE c.name = 'demo_crate' AND cv.version = '1.1.0'",
    )
    .fetch_one(&context.state.db)
    .await
    .expect("failed to load pinned version row");
    assert!(attempted, "rustdoc enrichment should target the pinned version");
}

#[tokio::test]
async fn index_crates_reports_unknown_pinned_version() {
    let context = mock_index_sync_context()
        .await
        .expect("failed to build mock index sync context");
    let _worker = tokio::spawn(run_refresh_worker_for_tests(context.state.clone()));

    let response = context
        .mcp
        .call_tool(
            "index_crates",
            json!({
                "crates": [{ "name": "demo_crate", "version": "9.9.9" }],
                "include_dependencies": false
            }),
        )
        .await
        .expect("index_crates call failed");
    let payload = common::structured_content(&response);
    assert_eq!(
        payload
            .get("failed")
            .and_then(Value::as_u64),
        Some(1),
        "{payload}"
    );
    let error = payload
        .pointer("/results/0/error")
        .and_then(Value::as_str)
        .expect("unknown version should report an error");
    assert!(error.contains("9.9.9"), "{error}");
}

/// A pinned release older than the newest 25 cached versions must still get
/// its own source indexed.
#[tokio::test]
async fn index_crates_indexes_source_for_old_pinned_version() {
    let context = mock_index_sync_context()
        .await
        .expect("failed to build mock index sync context");
    let _worker = tokio::spawn(run_refresh_worker_for_tests(context.state.clone()));

    let cache_dir = context
        .state
        .config
        .crate_source_cache_dir
        .clone();
    let source_dirs = pinned_source_versions()
        .into_iter()
        .map(|version| {
            let dir = cache_dir.join(format!("{PINNED_SOURCE_CRATE}-{version}"));
            std::fs::create_dir_all(dir.join("src")).expect("create source dir");
            std::fs::write(
                dir.join("src/lib.rs"),
                format!("pub const RELEASE: &str = \"{version}\";\n"),
            )
            .expect("write source file");
            dir
        })
        .collect::<Vec<_>>();

    let response = context
        .mcp
        .call_tool(
            "index_crates",
            json!({
                "crates": [{ "name": PINNED_SOURCE_CRATE, "version": PINNED_SOURCE_OLDEST_VERSION }],
                "include_dependencies": false
            }),
        )
        .await;
    let read = context
        .mcp
        .call_tool(
            "source_read",
            json!({
                "crate_name": PINNED_SOURCE_CRATE,
                "version": PINNED_SOURCE_OLDEST_VERSION,
                "path": "src/lib.rs"
            }),
        )
        .await;
    for dir in source_dirs {
        let _ = std::fs::remove_dir_all(dir);
    }

    let payload_response = response.expect("index_crates call failed");
    let payload = common::structured_content(&payload_response);
    assert_eq!(
        payload
            .get("succeeded")
            .and_then(Value::as_u64),
        Some(1),
        "{payload}"
    );
    let read = read.expect("pinned version source should be readable");
    assert!(
        common::first_content_text(&read).contains(PINNED_SOURCE_OLDEST_VERSION),
        "expected the pinned release's own source: {read}"
    );
}
