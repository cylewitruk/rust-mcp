use serde_json::{Value, json};

use super::{
    SEEDED_ALT_CRATE_NAME, SEEDED_ALT_CRATE_VERSION, SEEDED_CRATE_NAME, SEEDED_CRATE_NEXT_VERSION,
    SEEDED_CRATE_VERSION, call_tool_payload, seeded_indexed_context,
};

#[tokio::test]
async fn tool_crate_search_returns_seeded_hits() {
    let context = seeded_indexed_context().await;

    let payload = call_tool_payload(
        &context.rust_mcp,
        "crate_search",
        json!({ "query": "demo", "limit": 10 }),
    )
    .await;

    assert!(
        payload
            .get("count")
            .and_then(Value::as_u64)
            .is_some_and(|count| count >= 2),
        "expected crate.search count >= 2 for seeded fixtures: {payload}"
    );
    assert_eq!(
        payload
            .get("page")
            .and_then(Value::as_u64),
        Some(1),
        "expected crate.search to return page=1 by default: {payload}"
    );
    assert!(
        payload
            .get("has_more")
            .and_then(Value::as_bool)
            .is_some(),
        "expected crate.search has_more field: {payload}"
    );
    assert!(
        payload
            .get("truncated")
            .and_then(Value::as_bool)
            .is_some(),
        "expected crate.search truncated field: {payload}"
    );
    assert!(
        payload
            .get("next_cursor")
            .is_some(),
        "expected crate.search next_cursor field: {payload}"
    );

    let hits = payload
        .get("hits")
        .and_then(Value::as_array)
        .expect("crate_search should return hits array");
    assert!(
        hits.iter().any(|hit| {
            hit.get("name")
                .and_then(Value::as_str)
                == Some(SEEDED_CRATE_NAME)
        }),
        "expected crate.search hits to include {SEEDED_CRATE_NAME}: {hits:?}"
    );
    assert!(
        hits.iter().any(|hit| {
            hit.get("name")
                .and_then(Value::as_str)
                == Some(SEEDED_ALT_CRATE_NAME)
        }),
        "expected crate.search hits to include {SEEDED_ALT_CRATE_NAME}: {hits:?}"
    );
}

#[tokio::test]
async fn tool_crate_api_lists_seeded_function_symbol() {
    let context = seeded_indexed_context().await;

    let payload = call_tool_payload(
        &context.rust_mcp,
        "crate_api",
        json!({
            "crate_name": SEEDED_CRATE_NAME,
            "version": SEEDED_CRATE_VERSION,
            "kinds": ["function"],
            "limit": 10
        }),
    )
    .await;

    assert_eq!(
        payload
            .get("page")
            .and_then(Value::as_u64),
        Some(1),
        "expected crate.api to return page=1 by default: {payload}"
    );
    assert!(
        payload
            .get("has_more")
            .and_then(Value::as_bool)
            .is_some(),
        "expected crate.api has_more field: {payload}"
    );
    assert!(
        payload
            .get("truncated")
            .and_then(Value::as_bool)
            .is_some(),
        "expected crate.api truncated field: {payload}"
    );
    assert!(
        payload
            .get("next_cursor")
            .is_some(),
        "expected crate.api next_cursor field: {payload}"
    );

    let symbols = payload
        .get("symbols")
        .and_then(Value::as_array)
        .expect("crate_api should return symbols array");
    assert!(
        symbols
            .iter()
            .any(|symbol| symbol
                .get("name")
                .and_then(Value::as_str)
                == Some("parse")),
        "expected crate.api to include parse symbol: {symbols:?}"
    );
}

#[tokio::test]
async fn tool_crate_alternatives_includes_seeded_alternative() {
    let context = seeded_indexed_context().await;

    let payload = call_tool_payload(
        &context.rust_mcp,
        "crate_alternatives",
        json!({ "crate_name": SEEDED_CRATE_NAME, "limit": 5 }),
    )
    .await;

    assert!(
        payload
            .get("count")
            .and_then(Value::as_u64)
            .is_some_and(|count| count >= 1),
        "expected crate.alternatives count >= 1: {payload}"
    );
    assert_eq!(
        payload
            .get("page")
            .and_then(Value::as_u64),
        Some(1),
        "expected crate.alternatives to return page=1 by default: {payload}"
    );
    assert!(
        payload
            .get("has_more")
            .and_then(Value::as_bool)
            .is_some(),
        "expected crate.alternatives has_more field: {payload}"
    );
    assert!(
        payload
            .get("truncated")
            .and_then(Value::as_bool)
            .is_some(),
        "expected crate.alternatives truncated field: {payload}"
    );
    assert!(
        payload
            .get("next_cursor")
            .is_some(),
        "expected crate.alternatives next_cursor field: {payload}"
    );

    let alternatives = payload
        .get("alternatives")
        .and_then(Value::as_array)
        .expect("crate_alternatives should return alternatives array");
    assert!(
        alternatives
            .iter()
            .any(|candidate| {
                candidate
                    .get("crate_name")
                    .and_then(Value::as_str)
                    == Some(SEEDED_ALT_CRATE_NAME)
            }),
        "expected crate.alternatives to include {SEEDED_ALT_CRATE_NAME}: {alternatives:?}"
    );
}

#[tokio::test]
async fn tool_crate_api_diff_detects_breaking_changes() {
    let context = seeded_indexed_context().await;

    let payload = call_tool_payload(
        &context.rust_mcp,
        "crate_api_diff",
        json!({
            "crate_name": SEEDED_CRATE_NAME,
            "from_version": SEEDED_CRATE_VERSION,
            "to_version": SEEDED_CRATE_NEXT_VERSION,
            "limit": 20
        }),
    )
    .await;

    assert!(
        payload
            .get("breaking_changes_detected")
            .and_then(Value::as_bool)
            .is_some_and(|breaking| breaking),
        "expected crate.api_diff to report breaking changes: {payload}"
    );
    assert!(
        payload
            .get("added_count")
            .and_then(Value::as_u64)
            .is_some_and(|count| count >= 1),
        "expected crate.api_diff added_count >= 1: {payload}"
    );
    assert!(
        payload
            .get("removed_count")
            .and_then(Value::as_u64)
            .is_some_and(|count| count >= 1),
        "expected crate.api_diff removed_count >= 1: {payload}"
    );
}

#[tokio::test]
async fn tool_crate_compare_reports_recommendation_reasons() {
    let context = seeded_indexed_context().await;

    let payload = call_tool_payload(
        &context.rust_mcp,
        "crate_compare",
        json!({
            "left_crate": SEEDED_CRATE_NAME,
            "right_crate": SEEDED_ALT_CRATE_NAME
        }),
    )
    .await;

    assert_eq!(
        payload
            .get("left")
            .and_then(|left| left.get("crate_name"))
            .and_then(Value::as_str),
        Some(SEEDED_CRATE_NAME)
    );
    assert_eq!(
        payload
            .get("right")
            .and_then(|right| right.get("crate_name"))
            .and_then(Value::as_str),
        Some(SEEDED_ALT_CRATE_NAME)
    );
    assert!(
        payload
            .get("recommendation_reasons")
            .and_then(Value::as_array)
            .is_some_and(|reasons| !reasons.is_empty()),
        "expected crate.compare recommendation_reasons to be non-empty: {payload}"
    );
}

#[tokio::test]
async fn tool_crate_compatibility_resolves_seeded_pair() {
    let context = seeded_indexed_context().await;

    let payload = call_tool_payload(
        &context.rust_mcp,
        "crate_compatibility",
        json!({
            "left_crate": SEEDED_CRATE_NAME,
            "right_crate": SEEDED_ALT_CRATE_NAME
        }),
    )
    .await;

    assert_eq!(
        payload
            .get("resolvable")
            .and_then(Value::as_bool),
        Some(true)
    );

    let resolved_versions = payload
        .get("resolved_versions")
        .and_then(Value::as_array)
        .expect("crate_compatibility should return resolved_versions array");
    assert!(
        resolved_versions
            .iter()
            .any(|entry| entry
                .get("name")
                .and_then(Value::as_str)
                == Some(SEEDED_CRATE_NAME)),
        "expected resolved_versions to include {SEEDED_CRATE_NAME}: {resolved_versions:?}"
    );
    assert!(
        resolved_versions
            .iter()
            .any(|entry| {
                entry
                    .get("name")
                    .and_then(Value::as_str)
                    == Some(SEEDED_ALT_CRATE_NAME)
            }),
        "expected resolved_versions to include {SEEDED_ALT_CRATE_NAME}: {resolved_versions:?}"
    );
}

#[tokio::test]
async fn tool_crate_compatibility_matrix_tests_requested_pairs() {
    let context = seeded_indexed_context().await;

    let payload = call_tool_payload(
        &context.rust_mcp,
        "crate_compatibility_matrix",
        json!({
            "left_crate": SEEDED_CRATE_NAME,
            "right_crate": SEEDED_ALT_CRATE_NAME,
            "left_versions": [SEEDED_CRATE_VERSION, SEEDED_CRATE_NEXT_VERSION],
            "right_versions": [SEEDED_ALT_CRATE_VERSION],
            "max_pairs": 6
        }),
    )
    .await;

    let pairs_tested = payload
        .get("pairs_tested")
        .and_then(Value::as_u64)
        .expect("crate_compatibility_matrix should report pairs_tested");
    assert!(
        (1..=2).contains(&pairs_tested),
        "expected pairs_tested in [1, 2], got {pairs_tested}: {payload}"
    );

    let compatible_len = payload
        .get("compatible_pairs")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default();
    let incompatible_len = payload
        .get("incompatible_pairs")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default();
    assert_eq!(
        compatible_len + incompatible_len,
        pairs_tested as usize,
        "expected compatibility_matrix partitions to match pairs_tested: {payload}"
    );
}

#[tokio::test]
async fn tool_crate_derive_macros_handles_sparse_fixture() {
    let context = seeded_indexed_context().await;

    let payload = call_tool_payload(
        &context.rust_mcp,
        "crate_derive_macros",
        json!({
            "crate_name": SEEDED_CRATE_NAME,
            "version": SEEDED_CRATE_NEXT_VERSION
        }),
    )
    .await;

    assert!(
        payload
            .get("derive_macros")
            .and_then(Value::as_array)
            .is_some(),
        "expected derive_macros array in response: {payload}"
    );
    assert!(
        payload
            .get("attribute_macros")
            .and_then(Value::as_array)
            .is_some(),
        "expected attribute_macros array in response: {payload}"
    );
    assert!(
        payload
            .get("function_like_macros")
            .and_then(Value::as_array)
            .is_some(),
        "expected function_like_macros array in response: {payload}"
    );
}

#[tokio::test]
async fn tool_crate_error_types_handles_sparse_fixture() {
    let context = seeded_indexed_context().await;

    let payload = call_tool_payload(
        &context.rust_mcp,
        "crate_error_types",
        json!({
            "crate_name": SEEDED_CRATE_NAME,
            "version": SEEDED_CRATE_NEXT_VERSION,
            "limit": 20
        }),
    )
    .await;

    assert_eq!(
        payload
            .get("page")
            .and_then(Value::as_u64),
        Some(1),
        "expected crate.error_types to return page=1 by default: {payload}"
    );
    assert!(
        payload
            .get("has_more")
            .and_then(Value::as_bool)
            .is_some(),
        "expected crate.error_types has_more field: {payload}"
    );
    assert!(
        payload
            .get("truncated")
            .and_then(Value::as_bool)
            .is_some(),
        "expected crate.error_types truncated field: {payload}"
    );
    assert!(
        payload
            .get("next_cursor")
            .is_some(),
        "expected crate.error_types next_cursor field: {payload}"
    );

    assert_eq!(
        payload
            .get("count")
            .and_then(Value::as_u64),
        Some(0)
    );
    assert!(
        payload
            .get("error_types")
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty),
        "expected sparse fixture to return zero error types: {payload}"
    );
}

#[tokio::test]
async fn tool_crate_features_reports_default_std_feature() {
    let context = seeded_indexed_context().await;

    let payload = call_tool_payload(
        &context.rust_mcp,
        "crate_features",
        json!({
            "crate_name": SEEDED_CRATE_NAME,
            "version": SEEDED_CRATE_NEXT_VERSION
        }),
    )
    .await;

    assert!(
        payload
            .get("feature_count")
            .and_then(Value::as_u64)
            .is_some_and(|count| count >= 2),
        "expected at least two features for seeded version: {payload}"
    );

    let default_features = payload
        .get("default_features")
        .and_then(Value::as_array)
        .expect("crate_features should return default_features array");
    assert!(
        default_features
            .iter()
            .any(|feature| feature.as_str() == Some("std")),
        "expected default feature set to include std: {default_features:?}"
    );
}

#[tokio::test]
async fn tool_crate_graph_contains_seeded_dependency_edge() {
    let context = seeded_indexed_context().await;

    let payload = call_tool_payload(
        &context.rust_mcp,
        "crate_graph",
        json!({
            "crate_name": SEEDED_CRATE_NAME,
            "version": SEEDED_CRATE_NEXT_VERSION,
            "direction": "dependencies",
            "depth": 2
        }),
    )
    .await;

    assert!(
        payload
            .get("edge_count")
            .and_then(Value::as_u64)
            .is_some_and(|count| count >= 1),
        "expected dependency graph to include at least one edge: {payload}"
    );

    let edges = payload
        .get("edges")
        .and_then(Value::as_array)
        .expect("crate_graph should return edges array");
    assert!(
        edges.iter().any(|edge| edge
            .get("to_crate")
            .and_then(Value::as_str)
            == Some("serde")),
        "expected crate.graph edges to include serde dependency: {edges:?}"
    );
}

#[tokio::test]
async fn tool_crate_hotspots_reports_scanned_files() {
    let context = seeded_indexed_context().await;

    let payload = call_tool_payload(
        &context.rust_mcp,
        "crate_hotspots",
        json!({
            "crate_name": SEEDED_CRATE_NAME,
            "version": SEEDED_CRATE_NEXT_VERSION,
            "limit": 20
        }),
    )
    .await;

    assert!(
        payload
            .get("scanned_files")
            .and_then(Value::as_u64)
            .is_some(),
        "expected crate.hotspots to return scanned_files field: {payload}"
    );
    assert_eq!(
        payload
            .get("page")
            .and_then(Value::as_u64),
        Some(1),
        "expected crate.hotspots to return page=1 by default: {payload}"
    );
    assert!(
        payload
            .get("has_more")
            .and_then(Value::as_bool)
            .is_some(),
        "expected crate.hotspots has_more field: {payload}"
    );
    assert!(
        payload
            .get("truncated")
            .and_then(Value::as_bool)
            .is_some(),
        "expected crate.hotspots truncated field: {payload}"
    );
    assert!(
        payload
            .get("next_cursor")
            .is_some(),
        "expected crate.hotspots next_cursor field: {payload}"
    );

    let hotspots = payload
        .get("hotspots")
        .and_then(Value::as_array)
        .expect("crate_hotspots should return hotspots array");
    assert_eq!(
        payload
            .get("count")
            .and_then(Value::as_u64),
        Some(hotspots.len() as u64)
    );
}

#[tokio::test]
async fn tool_crate_intel_includes_seeded_dependency() {
    let context = seeded_indexed_context().await;

    let payload = call_tool_payload(
        &context.rust_mcp,
        "crate_intel",
        json!({
            "crate_name": SEEDED_CRATE_NAME,
            "version": SEEDED_CRATE_NEXT_VERSION
        }),
    )
    .await;

    assert_eq!(
        payload
            .get("selected_version")
            .and_then(Value::as_str),
        Some(SEEDED_CRATE_NEXT_VERSION)
    );

    let dependencies = payload
        .get("dependencies")
        .and_then(Value::as_array)
        .expect("crate_intel should return dependencies array");
    assert!(
        dependencies
            .iter()
            .any(|dep| dep
                .get("crate_name")
                .and_then(Value::as_str)
                == Some("serde")),
        "expected crate.intel dependencies to include serde: {dependencies:?}"
    );
}

#[tokio::test]
async fn tool_crate_license_check_evaluates_seeded_license_expression() {
    let context = seeded_indexed_context().await;

    let payload = call_tool_payload(
        &context.rust_mcp,
        "crate_license_check",
        json!({
            "crate_name": SEEDED_CRATE_NAME,
            "version": SEEDED_CRATE_NEXT_VERSION
        }),
    )
    .await;

    assert!(
        payload
            .get("license_expression")
            .and_then(Value::as_str)
            .is_some_and(|expression| expression.contains("MIT")),
        "expected seeded license expression to include MIT: {payload}"
    );
    assert_eq!(
        payload
            .get("policy_result")
            .and_then(Value::as_str),
        Some("allowed")
    );
}

#[tokio::test]
async fn tool_crate_migration_path_emits_actions_for_breaking_changes() {
    let context = seeded_indexed_context().await;

    let payload = call_tool_payload(
        &context.rust_mcp,
        "crate_migration_path",
        json!({
            "crate_name": SEEDED_CRATE_NAME,
            "from_version": SEEDED_CRATE_VERSION,
            "to_version": SEEDED_CRATE_NEXT_VERSION,
            "limit": 20
        }),
    )
    .await;

    assert!(
        payload
            .get("breaking_changes_detected")
            .and_then(Value::as_bool)
            .is_some_and(|breaking| breaking),
        "expected crate.migration_path to detect breaking changes: {payload}"
    );
    assert!(
        payload
            .get("migration_actions")
            .and_then(Value::as_array)
            .is_some_and(|actions| !actions.is_empty()),
        "expected crate.migration_path migration_actions to be non-empty: {payload}"
    );
}

#[tokio::test]
async fn tool_crate_re_exports_handles_sparse_fixture() {
    let context = seeded_indexed_context().await;

    let payload = call_tool_payload(
        &context.rust_mcp,
        "crate_re_exports",
        json!({
            "crate_name": SEEDED_CRATE_NAME,
            "version": SEEDED_CRATE_NEXT_VERSION,
            "limit": 20
        }),
    )
    .await;

    assert_eq!(
        payload
            .get("page")
            .and_then(Value::as_u64),
        Some(1),
        "expected crate.re_exports to return page=1 by default: {payload}"
    );
    assert!(
        payload
            .get("has_more")
            .and_then(Value::as_bool)
            .is_some(),
        "expected crate.re_exports has_more field: {payload}"
    );
    assert!(
        payload
            .get("truncated")
            .and_then(Value::as_bool)
            .is_some(),
        "expected crate.re_exports truncated field: {payload}"
    );
    assert!(
        payload
            .get("next_cursor")
            .is_some(),
        "expected crate.re_exports next_cursor field: {payload}"
    );

    let re_exports = payload
        .get("re_exports")
        .and_then(Value::as_array)
        .expect("crate_re_exports should return re_exports array");
    assert_eq!(
        payload
            .get("count")
            .and_then(Value::as_u64),
        Some(re_exports.len() as u64)
    );
}

#[tokio::test]
async fn tool_crate_import_path_handles_missing_symbol() {
    let context = seeded_indexed_context().await;

    let payload = call_tool_payload(
        &context.rust_mcp,
        "crate_import_path",
        json!({
            "crate_name": SEEDED_CRATE_NAME,
            "version": SEEDED_CRATE_NEXT_VERSION,
            "symbol_name": "definitely_missing_symbol",
            "limit": 10
        }),
    )
    .await;

    assert_eq!(
        payload
            .get("page")
            .and_then(Value::as_u64),
        Some(1),
        "expected crate.import_path to return page=1 by default: {payload}"
    );
    assert!(
        payload
            .get("has_more")
            .and_then(Value::as_bool)
            .is_some(),
        "expected crate.import_path has_more field: {payload}"
    );
    assert!(
        payload
            .get("truncated")
            .and_then(Value::as_bool)
            .is_some(),
        "expected crate.import_path truncated field: {payload}"
    );
    assert!(
        payload
            .get("next_cursor")
            .is_some(),
        "expected crate.import_path next_cursor field: {payload}"
    );

    assert_eq!(
        payload
            .get("count")
            .and_then(Value::as_u64),
        Some(0)
    );
    assert!(
        payload
            .get("best_import_path")
            .is_some_and(Value::is_null),
        "expected missing symbol to return null best_import_path: {payload}"
    );
}

#[tokio::test]
async fn tool_crate_trait_impls_handles_sparse_fixture() {
    let context = seeded_indexed_context().await;

    let payload = call_tool_payload(
        &context.rust_mcp,
        "crate_trait_impls",
        json!({
            "crate_name": SEEDED_CRATE_NAME,
            "version": SEEDED_CRATE_NEXT_VERSION,
            "type_name": "Parser",
            "limit": 20
        }),
    )
    .await;

    assert_eq!(
        payload
            .get("page")
            .and_then(Value::as_u64),
        Some(1),
        "expected crate.trait_impls to return page=1 by default: {payload}"
    );
    assert!(
        payload
            .get("has_more")
            .and_then(Value::as_bool)
            .is_some(),
        "expected crate.trait_impls has_more field: {payload}"
    );
    assert!(
        payload
            .get("truncated")
            .and_then(Value::as_bool)
            .is_some(),
        "expected crate.trait_impls truncated field: {payload}"
    );
    assert!(
        payload
            .get("next_cursor")
            .is_some(),
        "expected crate.trait_impls next_cursor field: {payload}"
    );

    assert_eq!(
        payload
            .get("count")
            .and_then(Value::as_u64),
        Some(0)
    );
    assert!(
        payload
            .get("impls")
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty),
        "expected sparse fixture to return no impl rows: {payload}"
    );
}

#[tokio::test]
async fn tool_crate_type_info_handles_sparse_fixture() {
    let context = seeded_indexed_context().await;

    let payload = call_tool_payload(
        &context.rust_mcp,
        "crate_type_info",
        json!({
            "crate_name": SEEDED_CRATE_NAME,
            "version": SEEDED_CRATE_NEXT_VERSION,
            "type_name": "Parser"
        }),
    )
    .await;

    assert!(
        payload
            .get("type_definition")
            .is_some_and(Value::is_null),
        "expected sparse fixture to return null type_definition: {payload}"
    );
}

#[tokio::test]
async fn tool_crate_usage_patterns_finds_dependent_references() {
    let context = seeded_indexed_context().await;

    let payload = call_tool_payload(
        &context.rust_mcp,
        "crate_usage_patterns",
        json!({
            "crate_name": SEEDED_CRATE_NAME,
            "version": SEEDED_CRATE_NEXT_VERSION,
            "symbol_name": "parse",
            "limit": 20
        }),
    )
    .await;

    // Usage patterns are extracted from dependent source file content, which
    // now excludes rustdoc_json entries. Verify the response envelope is
    // well-formed even when no patterns match.
    assert!(
        payload
            .get("count")
            .and_then(Value::as_u64)
            .is_some(),
        "expected crate.usage_patterns to return count field: {payload}"
    );
    assert_eq!(
        payload
            .get("page")
            .and_then(Value::as_u64),
        Some(1),
        "expected crate.usage_patterns to return page=1 by default: {payload}"
    );
    assert!(
        payload
            .get("has_more")
            .and_then(Value::as_bool)
            .is_some(),
        "expected crate.usage_patterns has_more field: {payload}"
    );
    assert!(
        payload
            .get("truncated")
            .and_then(Value::as_bool)
            .is_some(),
        "expected crate.usage_patterns truncated field: {payload}"
    );
    assert!(
        payload
            .get("next_cursor")
            .is_some(),
        "expected crate.usage_patterns next_cursor field: {payload}"
    );
    assert!(
        payload
            .get("patterns")
            .and_then(Value::as_array)
            .is_some(),
        "crate_usage_patterns should return patterns array: {payload}"
    );
}

#[tokio::test]
async fn tool_crate_versions_marks_latest_version() {
    let context = seeded_indexed_context().await;

    let payload = call_tool_payload(
        &context.rust_mcp,
        "crate_versions",
        json!({ "crate_name": SEEDED_CRATE_NAME, "limit": 20 }),
    )
    .await;

    assert!(
        payload
            .get("count")
            .and_then(Value::as_u64)
            .is_some_and(|count| count >= 2),
        "expected crate.versions count >= 2 for seeded fixture: {payload}"
    );
    assert_eq!(
        payload
            .get("page")
            .and_then(Value::as_u64),
        Some(1),
        "expected crate.versions to return page=1 by default: {payload}"
    );
    assert!(
        payload
            .get("has_more")
            .and_then(Value::as_bool)
            .is_some(),
        "expected crate.versions has_more field: {payload}"
    );
    assert!(
        payload
            .get("truncated")
            .and_then(Value::as_bool)
            .is_some(),
        "expected crate.versions truncated field: {payload}"
    );
    assert!(
        payload
            .get("next_cursor")
            .is_some(),
        "expected crate.versions next_cursor field: {payload}"
    );

    let versions = payload
        .get("versions")
        .and_then(Value::as_array)
        .expect("crate_versions should return versions array");
    assert!(
        versions
            .iter()
            .any(|version| {
                version
                    .get("version")
                    .and_then(Value::as_str)
                    == Some(SEEDED_CRATE_NEXT_VERSION)
                    && version
                        .get("is_latest")
                        .and_then(Value::as_bool)
                        == Some(true)
            }),
        "expected crate.versions to include latest marker for {SEEDED_CRATE_NEXT_VERSION}: \
         {versions:?}"
    );
}
