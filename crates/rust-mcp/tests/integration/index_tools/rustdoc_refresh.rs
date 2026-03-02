use rust_mcp_testing::fixtures::{
    materialize_workspace_rustdoc_fixture_zst, materialize_workspace_rustdoc_fixtures_zst,
};

use super::{
    Value, common, json, mock_index_sync_context, mock_index_sync_context_with_rustdoc_dir,
    seed_crate_release, write_rustdoc_fixture_file,
};

const TOKIO_RUSTDOC_FIXTURE: &str = "tokio_1.49.0_x86_64-unknown-linux-gnu_latest.json.zst";
const TOKIO_RUSTDOC_FIXTURE_148: &str = "tokio_1.48.0_x86_64-unknown-linux-gnu_latest.json.zst";
const TOKIO_CRATE_NAME: &str = "tokio";
const TOKIO_VERSION: &str = "1.49.0";
const TOKIO_VERSION_148: &str = "1.48.0";

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
            "index_refresh",
            json!({
                "scope": "rustdoc_json",
                "crate_name": "docs-rustdoc",
                "page": 1,
                "per_page": 20
            }),
        )
        .await
        .expect("index_refresh rustdoc_json docs.rs call failed");
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
            "index_refresh",
            json!({
                "scope": "rustdoc_json",
                "crate_name": crate_name,
                "page": 1,
                "per_page": 20
            }),
        )
        .await
        .expect("index_refresh rustdoc_json call failed");
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

#[tokio::test]
async fn index_refresh_rustdoc_json_scope_tracks_canonical_reexport_paths() {
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
            "index_refresh",
            json!({
                "scope": "rustdoc_json",
                "crate_name": crate_name,
                "page": 1,
                "per_page": 20
            }),
        )
        .await
        .expect("index_refresh rustdoc_json call failed");
    let payload = common::structured_content(&response);
    assert!(matches!(
        payload
            .get("status")
            .and_then(Value::as_str),
        Some("completed" | "completed_with_errors")
    ));

    let canonical_row = sqlx::query_as::<_, (Option<String>, Option<String>)>(
        "SELECT s.canonical_path, s.definition_path
         FROM symbols s
         JOIN crate_versions cv ON cv.id = s.crate_version_id
         JOIN crates c ON c.id = cv.crate_id
         WHERE c.name = $1
           AND cv.version = $2
           AND s.index_source = 'rustdoc_json'
           AND s.name = 'Parser'
         ORDER BY s.start_line ASC
         LIMIT 1",
    )
    .bind("demo-rustdoc")
    .bind("1.2.3")
    .fetch_one(&context.state.db)
    .await
    .expect("failed to load rustdoc canonical/definition path pair");

    assert_eq!(canonical_row.0.as_deref(), Some("demo_rustdoc::Parser"));
    assert_eq!(canonical_row.1.as_deref(), Some("demo_rustdoc::internal::Parser"));
}

#[tokio::test]
async fn index_refresh_rustdoc_json_scope_ingests_real_tokio_fixture_and_surfaces_enriched_tools() {
    let fixture_dir = materialize_workspace_rustdoc_fixture_zst(
        TOKIO_RUSTDOC_FIXTURE,
        TOKIO_CRATE_NAME,
        TOKIO_VERSION,
    )
    .expect("failed to materialize tokio rustdoc fixture");

    let context = mock_index_sync_context_with_rustdoc_dir(Some(
        fixture_dir
            .path()
            .to_path_buf(),
    ))
    .await
    .expect("failed to build rustdoc index context");

    let seeded = seed_crate_release(
        &context.state.db,
        TOKIO_CRATE_NAME,
        TOKIO_VERSION,
        1_000_000,
        Some("2026-01-01T00:00:00Z"),
    )
    .await
    .expect("failed to seed tokio release for rustdoc sync");
    assert!(seeded.version_id > 0);

    let response = context
        .mcp
        .call_tool(
            "index_refresh",
            json!({
                "scope": "rustdoc_json",
                "crate_name": TOKIO_CRATE_NAME,
                "page": 1,
                "per_page": 20
            }),
        )
        .await
        .expect("index_refresh rustdoc_json call for tokio fixture failed");
    let payload = common::structured_content(&response);
    assert!(matches!(
        payload
            .get("status")
            .and_then(Value::as_str),
        Some("completed" | "completed_with_errors")
    ));

    let counts = sqlx::query_as::<_, (i64, i64, i64)>(
        "SELECT
            (SELECT COUNT(*)::BIGINT FROM symbols WHERE crate_version_id = $1 AND index_source = \
         'rustdoc_json') AS symbol_count,
            (SELECT COUNT(*)::BIGINT FROM crate_types WHERE crate_version_id = $1 AND index_source \
         = 'rustdoc_json') AS type_count,
            (SELECT COUNT(*)::BIGINT FROM crate_impls WHERE crate_version_id = $1 AND index_source \
         = 'rustdoc_json') AS impl_count",
    )
    .bind(seeded.version_id)
    .fetch_one(&context.state.db)
    .await
    .expect("failed to count tokio rustdoc rows");
    assert!(
        counts.0 >= 100,
        "expected substantial symbol coverage from tokio rustdoc fixture, got {}",
        counts.0
    );
    assert!(
        counts.1 >= 10,
        "expected crate_types rows from tokio rustdoc fixture, got {}",
        counts.1
    );
    assert!(
        counts.2 >= 10,
        "expected crate_impls rows from tokio rustdoc fixture, got {}",
        counts.2
    );

    let runtime_symbol_exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(
            SELECT 1
            FROM symbols
            WHERE crate_version_id = $1
              AND index_source = 'rustdoc_json'
              AND name = 'Runtime'
              AND canonical_path LIKE 'tokio::%'
         )",
    )
    .bind(seeded.version_id)
    .fetch_one(&context.state.db)
    .await
    .expect("failed to check for tokio Runtime symbol");
    assert!(runtime_symbol_exists, "expected tokio rustdoc fixture to index Runtime symbol");

    let impl_trait_pair = sqlx::query_as::<_, (String, String)>(
        "SELECT ci.type_name, ci.trait_name
         FROM crate_impls ci
         JOIN crate_traits ct
           ON ct.crate_version_id = ci.crate_version_id
          AND ct.trait_name = ci.trait_name
         WHERE ci.crate_version_id = $1
           AND ci.index_source = 'rustdoc_json'
           AND ci.trait_name IS NOT NULL
         ORDER BY ci.type_name ASC, ci.trait_name ASC
         LIMIT 1",
    )
    .bind(seeded.version_id)
    .fetch_optional(&context.state.db)
    .await
    .expect("failed to select tokio impl/trait pair")
    .expect("tokio rustdoc fixture should include at least one local trait impl pair");

    let trait_impls_response = context
        .mcp
        .call_tool(
            "crate_trait_impls",
            json!({
                "crate_name": TOKIO_CRATE_NAME,
                "version": TOKIO_VERSION,
                "type_name": impl_trait_pair.0,
                "limit": 50
            }),
        )
        .await
        .expect("crate_trait_impls call failed for tokio fixture");
    let trait_impls_payload = common::structured_content(&trait_impls_response);
    assert!(
        trait_impls_payload
            .get("trait_definitions")
            .and_then(Value::as_array)
            .is_some_and(|definitions| {
                definitions
                    .iter()
                    .any(|definition| {
                        definition
                            .get("trait_name")
                            .and_then(Value::as_str)
                            == Some(impl_trait_pair.1.as_str())
                    })
            }),
        "expected crate.trait_impls to return trait_definitions for {}: {trait_impls_payload}",
        impl_trait_pair.1
    );

    let type_info_response = context
        .mcp
        .call_tool(
            "crate_type_info",
            json!({
                "crate_name": TOKIO_CRATE_NAME,
                "version": TOKIO_VERSION,
                "type_name": impl_trait_pair.0,
                "include_trait_impls": true
            }),
        )
        .await
        .expect("crate_type_info call failed for tokio fixture");
    let type_info_payload = common::structured_content(&type_info_response);
    assert!(
        type_info_payload
            .get("trait_definitions")
            .and_then(Value::as_array)
            .is_some_and(|definitions| {
                definitions
                    .iter()
                    .any(|definition| {
                        definition
                            .get("trait_name")
                            .and_then(Value::as_str)
                            == Some(impl_trait_pair.1.as_str())
                    })
            }),
        "expected crate.type_info to return trait_definitions for {}: {type_info_payload}",
        impl_trait_pair.1
    );

    let canonical_pair = sqlx::query_as::<_, (String, String)>(
        "SELECT canonical_path, definition_path
         FROM symbols
         WHERE crate_version_id = $1
           AND index_source = 'rustdoc_json'
           AND visibility = 'public'
           AND canonical_path IS NOT NULL
           AND definition_path IS NOT NULL
           AND canonical_path <> definition_path
         ORDER BY canonical_path ASC, definition_path ASC
         LIMIT 1",
    )
    .bind(seeded.version_id)
    .fetch_optional(&context.state.db)
    .await
    .expect("failed to select tokio canonical re-export pair");

    let re_exports_response = context
        .mcp
        .call_tool(
            "crate_re_exports",
            json!({
                "crate_name": TOKIO_CRATE_NAME,
                "version": TOKIO_VERSION,
                "limit": 20
            }),
        )
        .await
        .expect("crate_re_exports call failed for tokio fixture");
    let re_exports_payload = common::structured_content(&re_exports_response);
    let re_exports = re_exports_payload
        .get("re_exports")
        .and_then(Value::as_array)
        .expect("crate_re_exports should return re_exports array");
    assert_eq!(
        re_exports_payload
            .get("count")
            .and_then(Value::as_u64),
        Some(re_exports.len() as u64)
    );

    if let Some((canonical_path, definition_path)) = canonical_pair {
        assert!(
            re_exports
                .iter()
                .any(|entry| {
                    entry
                        .get("canonical_path")
                        .and_then(Value::as_str)
                        == Some(canonical_path.as_str())
                        && entry
                            .get("original_definition_path")
                            .and_then(Value::as_str)
                            == Some(definition_path.as_str())
                }),
            "expected crate.re_exports to include canonical/definition pair {} => {}: \
             {re_exports_payload}",
            canonical_path,
            definition_path
        );
    }
}

/// Exercises the full rustdoc indexing pipeline with two real tokio versions
/// from the workspace `rustdoc-json/` fixtures, including compat patching for
/// older format_versions, and verifies `crate.api_diff` across the pair.
#[tokio::test]
async fn index_refresh_rustdoc_json_multi_version_with_api_diff() {
    let fixture_dir = materialize_workspace_rustdoc_fixtures_zst(&[
        (TOKIO_RUSTDOC_FIXTURE_148, TOKIO_CRATE_NAME, TOKIO_VERSION_148),
        (TOKIO_RUSTDOC_FIXTURE, TOKIO_CRATE_NAME, TOKIO_VERSION),
    ])
    .expect("failed to materialize multi-version tokio rustdoc fixtures");

    let context = mock_index_sync_context_with_rustdoc_dir(Some(
        fixture_dir
            .path()
            .to_path_buf(),
    ))
    .await
    .expect("failed to build rustdoc index context");

    let tokio_v1_48 = seed_crate_release(
        &context.state.db,
        TOKIO_CRATE_NAME,
        TOKIO_VERSION_148,
        10_000_000,
        Some("2026-01-01T00:00:00Z"),
    )
    .await
    .expect("failed to seed tokio 1.48.0 release");
    let tokio_v1_49 = seed_crate_release(
        &context.state.db,
        TOKIO_CRATE_NAME,
        TOKIO_VERSION,
        11_000_000,
        Some("2026-02-01T00:00:00Z"),
    )
    .await
    .expect("failed to seed tokio 1.49.0 release");

    let response = context
        .mcp
        .call_tool(
            "index_refresh",
            json!({
                "scope": "rustdoc_json",
                "crate_name": TOKIO_CRATE_NAME,
                "page": 1,
                "per_page": 10
            }),
        )
        .await
        .expect("index_refresh rustdoc_json call failed");
    let payload = common::structured_content(&response);

    assert!(matches!(
        payload
            .get("status")
            .and_then(Value::as_str),
        Some("completed" | "completed_with_errors")
    ));

    let tokio_148_symbols = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*)::BIGINT FROM symbols WHERE crate_version_id = $1 AND index_source = \
         'rustdoc_json'",
    )
    .bind(tokio_v1_48.version_id)
    .fetch_one(&context.state.db)
    .await
    .expect("failed to count tokio 1.48.0 rustdoc symbols");
    let tokio_149_symbols = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*)::BIGINT FROM symbols WHERE crate_version_id = $1 AND index_source = \
         'rustdoc_json'",
    )
    .bind(tokio_v1_49.version_id)
    .fetch_one(&context.state.db)
    .await
    .expect("failed to count tokio 1.49.0 rustdoc symbols");

    assert!(
        tokio_148_symbols > 0,
        "tokio 1.48.0 rustdoc symbols were not indexed\nrefresh response: {payload:#}",
    );
    assert!(
        tokio_149_symbols > 0,
        "tokio 1.49.0 rustdoc symbols were not indexed\nrefresh response: {payload:#}",
    );

    let diff_response = context
        .mcp
        .call_tool(
            "crate_api_diff",
            json!({
                "crate_name": TOKIO_CRATE_NAME,
                "from_version": TOKIO_VERSION_148,
                "to_version": TOKIO_VERSION,
                "limit": 50
            }),
        )
        .await
        .expect("crate_api_diff call failed after rustdoc indexing");
    let diff_payload = common::structured_content(&diff_response);

    assert!(
        diff_payload
            .get("changes")
            .and_then(Value::as_array)
            .is_some_and(|changes| !changes.is_empty()),
        "expected api_diff changes from indexed rustdoc versions"
    );
}
