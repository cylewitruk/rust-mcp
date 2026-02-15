//! Integration tests for `crate.*` MCP tools.

mod common;

use rust_mcp_testing::fixtures::{seed_crate_version, seed_source_file, seed_symbol};
use serde_json::{Value, json};

#[tokio::test]
async fn crate_search_returns_seeded_crates_without_freshness_probe() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    let response = context
        .mcp
        .call_tool("crate.search", json!({"query": "serde", "limit": 10}))
        .await
        .expect("crate.search call failed");
    let payload = common::structured_content(&response);

    let count = payload
        .get("count")
        .and_then(Value::as_u64)
        .expect("count should be a u64");
    assert!(count >= 2);
    assert_eq!(
        payload
            .get("freshness_checks_performed")
            .and_then(Value::as_u64),
        Some(0)
    );
    assert_eq!(
        payload
            .get("refresh_jobs_enqueued")
            .and_then(Value::as_u64),
        Some(0)
    );
}

#[tokio::test]
async fn crate_features_resolves_default_and_dependency_backed_feature() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    let response = context
        .mcp
        .call_tool("crate.features", json!({"crate_name": "serde_json"}))
        .await
        .expect("crate.features call failed");
    let payload = common::structured_content(&response);

    assert_eq!(
        payload
            .get("feature_count")
            .and_then(Value::as_u64),
        Some(2)
    );

    let default_features = payload
        .get("default_features")
        .and_then(Value::as_array)
        .expect("default_features should be an array");
    assert!(
        default_features
            .iter()
            .any(|feature| feature.as_str() == Some("preserve_order"))
    );

    let preserve_order = payload
        .get("features")
        .and_then(Value::as_array)
        .and_then(|features| {
            features
                .iter()
                .find(|feature| {
                    feature
                        .get("name")
                        .and_then(Value::as_str)
                        == Some("preserve_order")
                })
        })
        .expect("expected preserve_order feature entry");

    let enabled_dependencies = preserve_order
        .get("enables_dependencies")
        .and_then(Value::as_array)
        .expect("enables_dependencies should be an array");
    assert!(
        enabled_dependencies
            .iter()
            .any(|dep| dep.as_str() == Some("indexmap"))
    );
}

#[tokio::test]
async fn crate_graph_dependencies_contains_seeded_edges() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    let response = context
        .mcp
        .call_tool(
            "crate.graph",
            json!({
                "crate_name": "serde_json",
                "direction": "dependencies",
                "depth": 2
            }),
        )
        .await
        .expect("crate.graph call failed");
    let payload = common::structured_content(&response);

    let edges = payload
        .get("edges")
        .and_then(Value::as_array)
        .expect("edges should be an array");
    assert!(edges.len() >= 2);
    assert!(edges.iter().any(|edge| {
        edge.get("to_crate")
            .and_then(Value::as_str)
            == Some("serde")
    }));
    assert!(edges.iter().any(|edge| {
        edge.get("to_crate")
            .and_then(Value::as_str)
            == Some("indexmap")
            && edge
                .get("optional")
                .and_then(Value::as_bool)
                == Some(true)
    }));
}

#[tokio::test]
async fn crate_api_and_versions_report_seeded_symbol_timeline() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    let api_response = context
        .mcp
        .call_tool("crate.api", json!({"crate_name": "serde_json", "limit": 20}))
        .await
        .expect("crate.api call failed");
    let api_payload = common::structured_content(&api_response);

    let symbols = api_payload
        .get("symbols")
        .and_then(Value::as_array)
        .expect("symbols should be an array");
    assert!(symbols.iter().any(|symbol| {
        symbol
            .get("name")
            .and_then(Value::as_str)
            == Some("from_str")
    }));

    let versions_response = context
        .mcp
        .call_tool("crate.versions", json!({"crate_name": "serde_json", "limit": 10}))
        .await
        .expect("crate.versions call failed");
    let versions_payload = common::structured_content(&versions_response);
    let versions = versions_payload
        .get("versions")
        .and_then(Value::as_array)
        .expect("versions should be an array");

    assert!(!versions.is_empty());
    assert!(
        versions
            .iter()
            .any(|version| {
                version
                    .get("is_latest")
                    .and_then(Value::as_bool)
                    == Some(true)
            })
    );
}

#[tokio::test]
async fn crate_intel_summarizes_dependencies_for_seeded_crate() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    let response = context
        .mcp
        .call_tool("crate.intel", json!({"crate_name": "serde_json"}))
        .await
        .expect("crate.intel call failed");
    let payload = common::structured_content(&response);

    let dependencies = payload
        .get("dependencies")
        .and_then(Value::as_array)
        .expect("dependencies should be an array");
    assert!(
        dependencies
            .iter()
            .any(|dep| {
                dep.get("crate_name")
                    .and_then(Value::as_str)
                    == Some("serde")
            })
    );
    assert!(
        dependencies
            .iter()
            .any(|dep| {
                dep.get("crate_name")
                    .and_then(Value::as_str)
                    == Some("indexmap")
                    && dep
                        .get("optional")
                        .and_then(Value::as_bool)
                        == Some(true)
            })
    );
}

#[tokio::test]
async fn crate_compare_and_compatibility_resolve_for_seeded_crates() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    let compare_response = context
        .mcp
        .call_tool(
            "crate.compare",
            json!({
                "left_crate": "indexmap",
                "right_crate": "serde"
            }),
        )
        .await
        .expect("crate.compare call failed");
    let compare_payload = common::structured_content(&compare_response);
    let recommendation = compare_payload
        .get("recommendation")
        .and_then(Value::as_str);
    let recommendation_reasons = compare_payload
        .get("recommendation_reasons")
        .and_then(Value::as_array)
        .expect("recommendation_reasons should be an array");
    assert!(!recommendation_reasons.is_empty());

    match recommendation {
        Some(winner) => {
            assert!(matches!(winner, "serde" | "indexmap"));
        }
        None => {
            assert!(
                recommendation_reasons
                    .iter()
                    .any(|reason| {
                        reason
                            .as_str()
                            .is_some_and(|text| text.contains("score similarly"))
                    })
            );
        }
    }

    let compatibility_response = context
        .mcp
        .call_tool(
            "crate.compatibility",
            json!({
                "left_crate": "serde_json",
                "left_version": "1.0.145",
                "right_crate": "serde",
                "right_version": "1.0.228",
                "check_features": true
            }),
        )
        .await
        .expect("crate.compatibility call failed");
    let compatibility_payload = common::structured_content(&compatibility_response);
    assert_eq!(
        compatibility_payload
            .get("resolvable")
            .and_then(Value::as_bool),
        Some(true)
    );

    let matrix_response = context
        .mcp
        .call_tool(
            "crate.compatibility_matrix",
            json!({
                "left_crate": "serde_json",
                "right_crate": "serde",
                "left_versions": ["1.0.145"],
                "right_versions": ["1.0.228"],
                "check_features": true
            }),
        )
        .await
        .expect("crate.compatibility_matrix call failed");
    let matrix_payload = common::structured_content(&matrix_response);
    assert_eq!(
        matrix_payload
            .get("pairs_tested")
            .and_then(Value::as_u64),
        Some(1)
    );
    assert_eq!(
        matrix_payload
            .get("compatible_pairs")
            .and_then(Value::as_array)
            .map(|pairs| pairs.len()),
        Some(1)
    );
}

#[tokio::test]
async fn crate_license_and_alternatives_return_expected_policy_shapes() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    let license_response = context
        .mcp
        .call_tool("crate.license_check", json!({"crate_name": "serde_json"}))
        .await
        .expect("crate.license_check call failed");
    let license_payload = common::structured_content(&license_response);
    assert_eq!(
        license_payload
            .get("policy_result")
            .and_then(Value::as_str),
        Some("unknown")
    );

    let alternatives_response = context
        .mcp
        .call_tool("crate.alternatives", json!({"crate_name": "serde_json", "limit": 5}))
        .await
        .expect("crate.alternatives call failed");
    let alternatives_payload = common::structured_content(&alternatives_response);
    assert!(
        alternatives_payload
            .get("count")
            .and_then(Value::as_u64)
            .unwrap_or_default()
            >= 1
    );
}

#[tokio::test]
async fn crate_usage_patterns_and_hotspots_return_matches_from_seeded_sources() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    seed_source_file(
        &context.state.db,
        context
            .fixture
            .dependent
            .version_id,
        "src/hotspots.rs",
        Some("rust"),
        "pub unsafe fn read(ptr: *const u8) -> u8 { unsafe { *ptr } }",
    )
    .await
    .expect("failed to seed hotspot source file");

    let usage_response = context
        .mcp
        .call_tool(
            "crate.usage_patterns",
            json!({
                "crate_name": "serde",
                "symbol_name": "from_str",
                "limit": 10
            }),
        )
        .await
        .expect("crate.usage_patterns call failed");
    let usage_payload = common::structured_content(&usage_response);
    assert!(
        usage_payload
            .get("count")
            .and_then(Value::as_u64)
            .unwrap_or_default()
            >= 1
    );

    let hotspots_response = context
        .mcp
        .call_tool(
            "crate.hotspots",
            json!({
                "crate_name": "serde_json",
                "include_unsafe": true,
                "include_concurrency": false
            }),
        )
        .await
        .expect("crate.hotspots call failed");
    let hotspots_payload = common::structured_content(&hotspots_response);

    let hotspots = hotspots_payload
        .get("hotspots")
        .and_then(Value::as_array)
        .expect("hotspots should be an array");
    assert!(
        hotspots
            .iter()
            .any(|hotspot| {
                hotspot
                    .get("kind")
                    .and_then(Value::as_str)
                    == Some("unsafe")
            })
    );
}

#[tokio::test]
async fn crate_api_diff_and_migration_path_detect_removed_symbol() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    let previous_version_id = seed_crate_version(
        &context.state.db,
        context
            .fixture
            .dependent
            .crate_id,
        "1.0.144",
        580_000_000,
        Some("2025-12-31T00:00:00Z"),
    )
    .await
    .expect("failed to seed previous crate version");
    let previous_source_id = seed_source_file(
        &context.state.db,
        previous_version_id,
        "src/lib.rs",
        Some("rust"),
        "pub fn parse_legacy() -> bool { true }",
    )
    .await
    .expect("failed to seed previous version source file");
    seed_symbol(
        &context.state.db,
        previous_version_id,
        previous_source_id,
        "parse_legacy",
        "function",
        1,
        1,
    )
    .await
    .expect("failed to seed previous version symbol");

    let diff_response = context
        .mcp
        .call_tool(
            "crate.api_diff",
            json!({
                "crate_name": "serde_json",
                "from_version": "1.0.144",
                "to_version": "1.0.145"
            }),
        )
        .await
        .expect("crate.api_diff call failed");
    let diff_payload = common::structured_content(&diff_response);

    assert_eq!(
        diff_payload
            .get("breaking_changes_detected")
            .and_then(Value::as_bool),
        Some(true)
    );
    assert!(
        diff_payload
            .get("removed_count")
            .and_then(Value::as_u64)
            .unwrap_or_default()
            >= 1
    );
    assert!(
        diff_payload
            .get("added_count")
            .and_then(Value::as_u64)
            .unwrap_or_default()
            >= 1
    );

    let migration_response = context
        .mcp
        .call_tool(
            "crate.migration_path",
            json!({
                "crate_name": "serde_json",
                "from_version": "1.0.144",
                "to_version": "1.0.145"
            }),
        )
        .await
        .expect("crate.migration_path call failed");
    let migration_payload = common::structured_content(&migration_response);
    assert_eq!(
        migration_payload
            .get("breaking_changes_detected")
            .and_then(Value::as_bool),
        Some(true)
    );

    let actions = migration_payload
        .get("migration_actions")
        .and_then(Value::as_array)
        .expect("migration_actions should be an array");
    assert!(actions.iter().any(|action| {
        action
            .get("affected_symbol")
            .and_then(Value::as_str)
            == Some("parse_legacy")
    }));
}

#[tokio::test]
async fn crate_re_exports_and_derive_macros_parse_seeded_source_files() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    seed_source_file(
        &context.state.db,
        context
            .fixture
            .dependent
            .version_id,
        "src/lib.rs",
        Some("rust"),
        "pub use crate::from_str;\npub fn from_str<T>() -> Result<T, Error> { todo!() }",
    )
    .await
    .expect("failed to seed re-export source");

    seed_source_file(
        &context.state.db,
        context
            .fixture
            .dependent
            .version_id,
        "src/macros.rs",
        Some("rust"),
        "use proc_macro::TokenStream;\n#[proc_macro_derive(MyDerive, attributes(my_attr))]\npub \
         fn my_derive(_input: TokenStream) -> TokenStream { TokenStream::new() \
         }\n#[proc_macro_attribute]\npub fn my_attr(_attr: TokenStream, item: TokenStream) -> \
         TokenStream { item }\n#[proc_macro]\npub fn my_macro(_input: TokenStream) -> TokenStream \
         { TokenStream::new() }",
    )
    .await
    .expect("failed to seed macro source");

    let re_exports_response = context
        .mcp
        .call_tool("crate.re_exports", json!({"crate_name": "serde_json", "limit": 10}))
        .await
        .expect("crate.re_exports call failed");
    let re_exports_payload = common::structured_content(&re_exports_response);
    assert!(
        re_exports_payload
            .get("count")
            .and_then(Value::as_u64)
            .unwrap_or_default()
            >= 1
    );

    let derive_response = context
        .mcp
        .call_tool("crate.derive_macros", json!({"crate_name": "serde_json"}))
        .await
        .expect("crate.derive_macros call failed");
    let derive_payload = common::structured_content(&derive_response);

    let derive_macros = derive_payload
        .get("derive_macros")
        .and_then(Value::as_array)
        .expect("derive_macros should be an array");
    assert!(
        derive_macros
            .iter()
            .any(|entry| {
                entry
                    .get("name")
                    .and_then(Value::as_str)
                    == Some("MyDerive")
            })
    );
}

#[tokio::test]
async fn crate_type_info_trait_impls_and_error_types_handle_sparse_index_data() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    let type_info_response = context
        .mcp
        .call_tool(
            "crate.type_info",
            json!({
                "crate_name": "serde_json",
                "type_name": "MissingType"
            }),
        )
        .await
        .expect("crate.type_info call failed");
    let type_info_payload = common::structured_content(&type_info_response);
    assert!(
        type_info_payload
            .get("type_definition")
            .is_some_and(Value::is_null)
    );

    let trait_impls_response = context
        .mcp
        .call_tool(
            "crate.trait_impls",
            json!({
                "crate_name": "serde_json",
                "type_name": "MissingType",
                "limit": 10
            }),
        )
        .await
        .expect("crate.trait_impls call failed");
    let trait_impls_payload = common::structured_content(&trait_impls_response);
    assert_eq!(
        trait_impls_payload
            .get("count")
            .and_then(Value::as_u64),
        Some(0)
    );

    let error_types_response = context
        .mcp
        .call_tool(
            "crate.error_types",
            json!({
                "crate_name": "serde_json",
                "limit": 10
            }),
        )
        .await
        .expect("crate.error_types call failed");
    let error_types_payload = common::structured_content(&error_types_response);
    assert_eq!(
        error_types_payload
            .get("count")
            .and_then(Value::as_u64),
        Some(0)
    );
}
