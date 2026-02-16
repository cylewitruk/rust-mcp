use super::{
    Value, common, json, mock_index_sync_context, mock_index_sync_context_with_rustdoc_dir,
    seed_crate_release, write_invalid_utf8_rustdoc_fixture_file,
};

#[tokio::test]
async fn index_refresh_rustdoc_json_scope_reports_remote_and_fallback_failures() {
    let context = mock_index_sync_context()
        .await
        .expect("failed to build index context");

    let seeded = seed_crate_release(
        &context.state.db,
        "demo-rustdoc",
        "1.2.3",
        42,
        Some("2026-01-01T00:00:00Z"),
    )
    .await
    .expect("failed to seed crate release for rustdoc failure-path test");
    assert!(seeded.version_id > 0);

    let response = context
        .mcp
        .call_tool(
            "index.refresh",
            json!({
                "scope": "rustdoc_json",
                "crate_name": "demo-rustdoc",
                "page": 1,
                "per_page": 20
            }),
        )
        .await
        .expect("index.refresh rustdoc_json failure-path call failed");
    let payload = common::structured_content(&response);

    assert_eq!(
        payload
            .get("status")
            .and_then(Value::as_str),
        Some("completed_with_errors")
    );
    let result = payload
        .get("result")
        .and_then(Value::as_object)
        .expect("result should be present");
    assert!(
        result
            .get("errors")
            .and_then(Value::as_array)
            .is_some_and(|errors| {
                errors
                    .iter()
                    .filter_map(Value::as_str)
                    .any(|error| {
                        error.contains("docs.rs rustdoc JSON request failed")
                            && error.contains("local rustdoc fallback unavailable")
                    })
            })
    );
}

#[tokio::test]
async fn index_refresh_rustdoc_json_scope_reports_invalid_utf8_fixture() {
    let rustdoc_dir = write_invalid_utf8_rustdoc_fixture_file();
    let context = mock_index_sync_context_with_rustdoc_dir(Some(rustdoc_dir))
        .await
        .expect("failed to build invalid-rustdoc index context");

    let seeded = seed_crate_release(
        &context.state.db,
        "demo-rustdoc",
        "1.2.3",
        42,
        Some("2026-01-01T00:00:00Z"),
    )
    .await
    .expect("failed to seed crate release for invalid-utf8 rustdoc sync");
    assert!(seeded.version_id > 0);

    let response = context
        .mcp
        .call_tool(
            "index.refresh",
            json!({
                "scope": "rustdoc_json",
                "crate_name": "demo-rustdoc",
                "page": 1,
                "per_page": 20
            }),
        )
        .await
        .expect("index.refresh rustdoc_json invalid-utf8 call failed");
    let payload = common::structured_content(&response);

    assert_eq!(
        payload
            .get("status")
            .and_then(Value::as_str),
        Some("completed_with_errors")
    );
    let result = payload
        .get("result")
        .and_then(Value::as_object)
        .expect("result should be present");
    assert!(
        result
            .get("errors")
            .and_then(Value::as_array)
            .is_some_and(|errors| {
                errors
                    .iter()
                    .filter_map(Value::as_str)
                    .any(|error| error.contains("invalid UTF-8 in rustdoc JSON file"))
            })
    );
}
