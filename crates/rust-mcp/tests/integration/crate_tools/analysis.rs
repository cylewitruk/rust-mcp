use super::{Value, common, json, seed_crate_version, seed_source_file, seed_symbol};

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
