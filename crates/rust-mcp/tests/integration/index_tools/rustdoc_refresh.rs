use super::{
    Value, common, json, mock_index_sync_context, mock_index_sync_context_with_rustdoc_dir,
    seed_crate_release, write_rustdoc_fixture_file,
};

#[tokio::test]
async fn index_refresh_rustdoc_json_scope_ingests_docs_rs_payload() {
    let context = mock_index_sync_context()
        .await
        .expect("failed to build rustdoc index context");

    let seeded = seed_crate_release(
        &context.state.db,
        "docs-rustdoc",
        "1.2.3",
        84,
        Some("2026-01-01T00:00:00Z"),
    )
    .await
    .expect("failed to seed crate release for docs.rs rustdoc sync");
    assert!(seeded.version_id > 0);

    let response = context
        .mcp
        .call_tool(
            "index.refresh",
            json!({
                "scope": "rustdoc_json",
                "crate_name": "docs-rustdoc",
                "page": 1,
                "per_page": 20
            }),
        )
        .await
        .expect("index.refresh rustdoc_json docs.rs call failed");
    let payload = common::structured_content(&response);

    assert_eq!(
        payload
            .get("scope")
            .and_then(Value::as_str),
        Some("rustdoc_json")
    );
    assert_eq!(
        payload
            .get("status")
            .and_then(Value::as_str),
        Some("completed")
    );

    let rustdoc_symbol_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*)::BIGINT
         FROM symbols s
         JOIN crate_versions cv ON cv.id = s.crate_version_id
         JOIN crates c ON c.id = cv.crate_id
         WHERE c.name = $1
           AND cv.version = $2
           AND s.index_source = 'rustdoc_json'",
    )
    .bind("docs-rustdoc")
    .bind("1.2.3")
    .fetch_one(&context.state.db)
    .await
    .expect("failed to count docs.rs rustdoc symbols");
    assert!(rustdoc_symbol_count >= 1);
}

#[tokio::test]
async fn index_refresh_rustdoc_json_scope_ingests_fixture_file() {
    let (rustdoc_dir, crate_name, crate_version) = write_rustdoc_fixture_file();
    let context = mock_index_sync_context_with_rustdoc_dir(Some(rustdoc_dir))
        .await
        .expect("failed to build rustdoc index context");

    let seeded = seed_crate_release(
        &context.state.db,
        &crate_name,
        &crate_version,
        42,
        Some("2026-01-01T00:00:00Z"),
    )
    .await
    .expect("failed to seed crate release for rustdoc sync");
    assert!(seeded.version_id > 0);

    let response = context
        .mcp
        .call_tool(
            "index.refresh",
            json!({
                "scope": "rustdoc_json",
                "crate_name": crate_name,
                "page": 1,
                "per_page": 20
            }),
        )
        .await
        .expect("index.refresh rustdoc_json call failed");
    let payload = common::structured_content(&response);

    assert_eq!(
        payload
            .get("scope")
            .and_then(Value::as_str),
        Some("rustdoc_json")
    );
    assert!(matches!(
        payload
            .get("status")
            .and_then(Value::as_str),
        Some("completed" | "completed_with_errors")
    ));

    let result = payload
        .get("result")
        .and_then(Value::as_object)
        .expect("result should be present");
    assert!(
        result
            .get("synced_versions")
            .and_then(Value::as_u64)
            .unwrap_or_default()
            >= 1
    );
    assert!(
        result
            .get("synced_dependencies")
            .and_then(Value::as_u64)
            .unwrap_or_default()
            >= 1
    );

    let rustdoc_symbol_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*)::BIGINT
         FROM symbols s
         JOIN crate_versions cv ON cv.id = s.crate_version_id
         JOIN crates c ON c.id = cv.crate_id
         WHERE c.name = $1
           AND cv.version = $2
           AND s.index_source = 'rustdoc_json'",
    )
    .bind("demo-rustdoc")
    .bind("1.2.3")
    .fetch_one(&context.state.db)
    .await
    .expect("failed to count rustdoc symbols");
    assert!(rustdoc_symbol_count >= 1);
}
