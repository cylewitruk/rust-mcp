use test_log::test as test_log;
use tracing::debug;

use super::{
    Value, common, json, seed_crate_version, seed_source_file, seed_symbol,
    write_fixture_source_to_registry,
};

#[tokio::test]
async fn crate_usage_patterns_returns_seeded_symbol_matches() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    let response = context
        .mcp
        .call_tool(
            "crate_usage_patterns",
            json!({
                "crate_name": "serde",
                "symbol_name": "from_str",
                "limit": 10
            }),
        )
        .await
        .expect("crate_usage_patterns call failed");
    let payload = common::structured_content(&response);

    assert!(
        payload
            .get("count")
            .and_then(Value::as_u64)
            .unwrap_or_default()
            >= 1
    );
    assert_eq!(
        payload
            .get("page")
            .and_then(Value::as_u64),
        Some(1)
    );
    assert!(
        payload
            .get("has_more")
            .and_then(Value::as_bool)
            .is_some()
    );
    assert!(
        payload
            .get("truncated")
            .and_then(Value::as_bool)
            .is_some()
    );
    assert!(
        payload
            .get("next_cursor")
            .is_some()
    );
    assert!(
        payload
            .get("confidence")
            .is_some()
    );
    assert!(
        payload
            .get("confidence_assessment")
            .is_some()
    );
}

#[tokio::test]
async fn crate_hotspots_detects_unsafe_in_seeded_source() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    let hotspot_content = "pub unsafe fn read(ptr: *const u8) -> u8 { unsafe { *ptr } }";
    write_fixture_source_to_registry(
        &context.registry_dir,
        "serde_json",
        "1.0.145",
        "src/hotspots.rs",
        hotspot_content,
    );
    seed_source_file(
        &context.state.db,
        context
            .fixture
            .dependent
            .version_id,
        "src/hotspots.rs",
        Some("rust"),
        hotspot_content,
    )
    .await
    .expect("failed to seed hotspot source file");

    let response = context
        .mcp
        .call_tool(
            "crate_hotspots",
            json!({
                "crate_name": "serde_json",
                "include_unsafe": true,
                "include_concurrency": false
            }),
        )
        .await
        .expect("crate_hotspots call failed");
    let payload = common::structured_content(&response);

    let hotspots = payload
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
    assert_eq!(
        payload
            .get("page")
            .and_then(Value::as_u64),
        Some(1)
    );
    assert!(
        payload
            .get("has_more")
            .and_then(Value::as_bool)
            .is_some()
    );
    assert!(
        payload
            .get("truncated")
            .and_then(Value::as_bool)
            .is_some()
    );
    assert!(
        payload
            .get("next_cursor")
            .is_some()
    );
    assert!(
        payload
            .get("confidence")
            .is_some()
    );
    assert!(
        payload
            .get("confidence_assessment")
            .is_some()
    );
}

#[tokio::test]
async fn crate_migration_path_reports_removed_symbol() {
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

    let response = context
        .mcp
        .call_tool(
            "crate_migration_path",
            json!({
                "crate_name": "serde_json",
                "from_version": "1.0.144",
                "to_version": "1.0.145"
            }),
        )
        .await
        .expect("crate_migration_path call failed");
    let payload = common::structured_content(&response);

    assert_eq!(
        payload
            .get("breaking_changes_detected")
            .and_then(Value::as_bool),
        Some(true)
    );

    let actions = payload
        .get("migration_actions")
        .and_then(Value::as_array)
        .expect("migration_actions should be an array");
    assert!(actions.iter().any(|action| {
        action
            .get("affected_symbol")
            .and_then(Value::as_str)
            == Some("parse_legacy")
    }));
    assert!(
        payload
            .get("confidence")
            .is_some()
    );
    assert!(
        payload
            .get("confidence_assessment")
            .is_some()
    );
}

#[tokio::test]
async fn crate_derive_macros_parses_proc_macro_source() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    let macros_content = "use proc_macro::TokenStream;\n#[proc_macro_derive(MyDerive, \
                          attributes(my_attr))]\npub fn my_derive(_input: TokenStream) -> \
                          TokenStream { TokenStream::new() }\n#[proc_macro_attribute]\npub fn \
                          my_attr(_attr: TokenStream, item: TokenStream) -> TokenStream { item \
                          }\n#[proc_macro]\npub fn my_macro(_input: TokenStream) -> TokenStream { \
                          TokenStream::new() }";
    write_fixture_source_to_registry(
        &context.registry_dir,
        "serde_json",
        "1.0.145",
        "src/macros.rs",
        macros_content,
    );
    seed_source_file(
        &context.state.db,
        context
            .fixture
            .dependent
            .version_id,
        "src/macros.rs",
        Some("rust"),
        macros_content,
    )
    .await
    .expect("failed to seed macro source");

    let response = context
        .mcp
        .call_tool("crate_derive_macros", json!({"crate_name": "serde_json"}))
        .await
        .expect("crate_derive_macros call failed");
    let payload = common::structured_content(&response);

    let derive_macros = payload
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
    assert!(
        payload
            .get("attribute_macros")
            .and_then(Value::as_array)
            .is_some()
    );
    assert!(
        payload
            .get("function_like_macros")
            .and_then(Value::as_array)
            .is_some()
    );
    assert!(
        payload
            .get("confidence")
            .is_some()
    );
    assert!(
        payload
            .get("confidence_assessment")
            .is_some()
    );
}

#[tokio::test]
async fn crate_usage_patterns_and_hotspots_return_matches_from_seeded_sources() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    let hotspot_content = "pub unsafe fn read(ptr: *const u8) -> u8 { unsafe { *ptr } }";
    write_fixture_source_to_registry(
        &context.registry_dir,
        "serde_json",
        "1.0.145",
        "src/hotspots.rs",
        hotspot_content,
    );
    seed_source_file(
        &context.state.db,
        context
            .fixture
            .dependent
            .version_id,
        "src/hotspots.rs",
        Some("rust"),
        hotspot_content,
    )
    .await
    .expect("failed to seed hotspot source file");

    let usage_response = context
        .mcp
        .call_tool(
            "crate_usage_patterns",
            json!({
                "crate_name": "serde",
                "symbol_name": "from_str",
                "limit": 10
            }),
        )
        .await
        .expect("crate_usage_patterns call failed");
    let usage_payload = common::structured_content(&usage_response);
    assert!(
        usage_payload
            .get("count")
            .and_then(Value::as_u64)
            .unwrap_or_default()
            >= 1
    );
    assert_eq!(
        usage_payload
            .get("page")
            .and_then(Value::as_u64),
        Some(1)
    );
    assert!(
        usage_payload
            .get("has_more")
            .and_then(Value::as_bool)
            .is_some()
    );
    assert!(
        usage_payload
            .get("truncated")
            .and_then(Value::as_bool)
            .is_some()
    );
    assert!(
        usage_payload
            .get("next_cursor")
            .is_some()
    );

    let hotspots_response = context
        .mcp
        .call_tool(
            "crate_hotspots",
            json!({
                "crate_name": "serde_json",
                "include_unsafe": true,
                "include_concurrency": false
            }),
        )
        .await
        .expect("crate_hotspots call failed");
    let hotspots_payload = common::structured_content(&hotspots_response);

    let hotspots = hotspots_payload
        .get("hotspots")
        .and_then(Value::as_array)
        .expect("hotspots should be an array");
    assert_eq!(
        hotspots_payload
            .get("page")
            .and_then(Value::as_u64),
        Some(1)
    );
    assert!(
        hotspots_payload
            .get("has_more")
            .and_then(Value::as_bool)
            .is_some()
    );
    assert!(
        hotspots_payload
            .get("truncated")
            .and_then(Value::as_bool)
            .is_some()
    );
    assert!(
        hotspots_payload
            .get("next_cursor")
            .is_some()
    );
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
#[test_log]
async fn crate_api_diff_and_migration_path_detect_removed_symbol() {
    debug!("Creating common seeded MCP context");
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    debug!("Seeding crate version");
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

    debug!("Seeding source file");
    let previous_source_id = seed_source_file(
        &context.state.db,
        previous_version_id,
        "src/lib.rs",
        Some("rust"),
        "pub fn parse_legacy() -> bool { true }",
    )
    .await
    .expect("failed to seed previous version source file");

    debug!("Seeding symbol");
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
            "crate_api_diff",
            json!({
                "crate_name": "serde_json",
                "from_version": "1.0.144",
                "to_version": "1.0.145"
            }),
        )
        .await
        .expect("crate_api_diff call failed");

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
            "crate_migration_path",
            json!({
                "crate_name": "serde_json",
                "from_version": "1.0.144",
                "to_version": "1.0.145"
            }),
        )
        .await
        .expect("crate_migration_path call failed");
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

    let lib_content =
        "pub use crate::from_str;\npub fn from_str<T>() -> Result<T, Error> { todo!() }";
    write_fixture_source_to_registry(
        &context.registry_dir,
        "serde_json",
        "1.0.145",
        "src/lib.rs",
        lib_content,
    );
    seed_source_file(
        &context.state.db,
        context
            .fixture
            .dependent
            .version_id,
        "src/lib.rs",
        Some("rust"),
        lib_content,
    )
    .await
    .expect("failed to seed re-export source");

    let macros_content = "use proc_macro::TokenStream;\n#[proc_macro_derive(MyDerive, \
                          attributes(my_attr))]\npub fn my_derive(_input: TokenStream) -> \
                          TokenStream { TokenStream::new() }\n#[proc_macro_attribute]\npub fn \
                          my_attr(_attr: TokenStream, item: TokenStream) -> TokenStream { item \
                          }\n#[proc_macro]\npub fn my_macro(_input: TokenStream) -> TokenStream { \
                          TokenStream::new() }";
    write_fixture_source_to_registry(
        &context.registry_dir,
        "serde_json",
        "1.0.145",
        "src/macros.rs",
        macros_content,
    );
    seed_source_file(
        &context.state.db,
        context
            .fixture
            .dependent
            .version_id,
        "src/macros.rs",
        Some("rust"),
        macros_content,
    )
    .await
    .expect("failed to seed macro source");

    let re_exports_response = context
        .mcp
        .call_tool("crate_re_exports", json!({"crate_name": "serde_json", "limit": 10}))
        .await
        .expect("crate_re_exports call failed");
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
        .call_tool("crate_derive_macros", json!({"crate_name": "serde_json"}))
        .await
        .expect("crate_derive_macros call failed");
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
async fn crate_re_exports_prefers_rustdoc_canonical_paths_when_available() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    let rustdoc_source_file_id = seed_source_file(
        &context.state.db,
        context
            .fixture
            .dependent
            .version_id,
        "rustdoc-json/serde_json-1.0.145.json",
        Some("rustdoc_json"),
        "{\"mock\":\"rustdoc\"}",
    )
    .await
    .expect("failed to seed rustdoc-json source file");

    sqlx::query(
        "INSERT INTO symbols (
            crate_version_id,
            source_file_id,
            name,
            kind,
            signature,
            visibility,
            start_line,
            end_line,
            index_source,
            rustdoc_item_id,
            canonical_path,
            definition_path
         ) VALUES (
            $1, $2, 'Parser', 'struct', 'struct Parser', 'public', 1, 1, 'rustdoc_json', 42, \
         'serde_json::Parser', 'serde_json::internal::Parser'
         )",
    )
    .bind(
        context
            .fixture
            .dependent
            .version_id,
    )
    .bind(rustdoc_source_file_id)
    .execute(&context.state.db)
    .await
    .expect("failed to seed rustdoc re-export symbol");

    let re_exports_response = context
        .mcp
        .call_tool(
            "crate_re_exports",
            json!({
                "crate_name": "serde_json",
                "path_prefix": "serde_json::",
                "limit": 10
            }),
        )
        .await
        .expect("crate_re_exports call failed");
    let re_exports_payload = common::structured_content(&re_exports_response);
    assert!(
        re_exports_payload
            .get("count")
            .and_then(Value::as_u64)
            .unwrap_or_default()
            >= 1
    );

    let re_export_entries = re_exports_payload
        .get("re_exports")
        .and_then(Value::as_array)
        .expect("re_exports should be an array");
    assert!(
        re_export_entries
            .iter()
            .any(|entry| {
                entry
                    .get("canonical_path")
                    .and_then(Value::as_str)
                    == Some("serde_json::Parser")
                    && entry
                        .get("original_definition_path")
                        .and_then(Value::as_str)
                        == Some("serde_json::internal::Parser")
            })
    );
}

#[tokio::test]
async fn crate_import_path_resolves_best_known_public_path() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    let rustdoc_source_file_id = seed_source_file(
        &context.state.db,
        context
            .fixture
            .dependent
            .version_id,
        "rustdoc-json/serde_json-1.0.145.json",
        Some("rustdoc_json"),
        "{\"mock\":\"rustdoc\"}",
    )
    .await
    .expect("failed to seed rustdoc-json source file");

    sqlx::query(
        "INSERT INTO symbols (
            crate_version_id,
            source_file_id,
            name,
            kind,
            signature,
            visibility,
            start_line,
            end_line,
            index_source,
            rustdoc_item_id,
            canonical_path,
            definition_path
         ) VALUES (
            $1, $2, 'Parser', 'struct', 'struct Parser', 'public', 1, 1, 'rustdoc_json', 43, \
         'serde_json::Parser', 'serde_json::internal::Parser'
         )",
    )
    .bind(
        context
            .fixture
            .dependent
            .version_id,
    )
    .bind(rustdoc_source_file_id)
    .execute(&context.state.db)
    .await
    .expect("failed to seed rustdoc import-path symbol");

    let response = context
        .mcp
        .call_tool(
            "crate_import_path",
            json!({
                "crate_name": "serde_json",
                "symbol_name": "Parser",
                "kind": "struct",
                "limit": 10
            }),
        )
        .await
        .expect("crate_import_path call failed");
    let payload = common::structured_content(&response);

    assert_eq!(
        payload
            .get("best_import_path")
            .and_then(Value::as_str),
        Some("serde_json::Parser")
    );
    assert!(
        payload
            .get("count")
            .and_then(Value::as_u64)
            .unwrap_or_default()
            >= 1
    );
}

#[tokio::test]
async fn crate_api_diff_prefers_rustdoc_rows_when_dual_source_symbols_exist() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    let previous_version_id = seed_crate_version(
        &context.state.db,
        context
            .fixture
            .dependent
            .crate_id,
        "1.0.143",
        570_000_000,
        Some("2025-12-30T00:00:00Z"),
    )
    .await
    .expect("failed to seed previous crate version");

    let from_local_source = seed_source_file(
        &context.state.db,
        previous_version_id,
        "src/lib.rs",
        Some("rust"),
        "pub fn dual_source() -> bool { true }",
    )
    .await
    .expect("failed to seed previous local source");

    let from_rustdoc_source = seed_source_file(
        &context.state.db,
        previous_version_id,
        "rustdoc-json/serde_json-1.0.143.json",
        Some("rustdoc_json"),
        "{\"mock\":\"rustdoc\"}",
    )
    .await
    .expect("failed to seed previous rustdoc source");

    sqlx::query(
        "INSERT INTO symbols (
            crate_version_id,
            source_file_id,
            name,
            kind,
            signature,
            visibility,
            start_line,
            end_line,
            index_source
         ) VALUES
            ($1, $2, 'dual_source', 'function', 'pub fn dual_source() -> bool', 'public', 1, 1, \
         'local_cache'),
            ($1, $3, 'dual_source', 'function', 'pub fn dual_source() -> bool', 'public', 1, 1, \
         'rustdoc_json')",
    )
    .bind(previous_version_id)
    .bind(from_local_source)
    .bind(from_rustdoc_source)
    .execute(&context.state.db)
    .await
    .expect("failed to seed previous symbols");

    let to_local_source = seed_source_file(
        &context.state.db,
        context
            .fixture
            .dependent
            .version_id,
        "src/lib.rs",
        Some("rust"),
        "pub fn dual_source() -> bool { true }",
    )
    .await
    .expect("failed to seed current local source");

    let to_rustdoc_source = seed_source_file(
        &context.state.db,
        context
            .fixture
            .dependent
            .version_id,
        "rustdoc-json/serde_json-1.0.145.json",
        Some("rustdoc_json"),
        "{\"mock\":\"rustdoc\"}",
    )
    .await
    .expect("failed to seed current rustdoc source");

    sqlx::query(
        "INSERT INTO symbols (
            crate_version_id,
            source_file_id,
            name,
            kind,
            signature,
            visibility,
            start_line,
            end_line,
            index_source
         ) VALUES
            ($1, $2, 'dual_source', 'function', 'pub fn dual_source() -> bool', 'public', 1, 1, \
         'local_cache'),
            ($1, $3, 'dual_source', 'function', 'pub fn dual_source(input: &str) -> bool', \
         'public', 1, 1, 'rustdoc_json')",
    )
    .bind(
        context
            .fixture
            .dependent
            .version_id,
    )
    .bind(to_local_source)
    .bind(to_rustdoc_source)
    .execute(&context.state.db)
    .await
    .expect("failed to seed current symbols");

    let response = context
        .mcp
        .call_tool(
            "crate_api_diff",
            json!({
                "crate_name": "serde_json",
                "from_version": "1.0.143",
                "to_version": "1.0.145",
                "limit": 100
            }),
        )
        .await
        .expect("crate_api_diff call failed");
    let payload = common::structured_content(&response);

    let changed_count = payload
        .get("changed_count")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    assert!(
        changed_count >= 1,
        "expected signature change from rustdoc-prioritized rows: {payload}"
    );
    assert!(
        payload
            .get("changes")
            .and_then(Value::as_array)
            .is_some_and(|changes| {
                changes.iter().any(|change| {
                    change
                        .get("name")
                        .and_then(Value::as_str)
                        == Some("dual_source")
                        && change
                            .get("change_type")
                            .and_then(Value::as_str)
                            == Some("signature_changed")
                })
            }),
        "expected dual_source signature_changed entry: {payload}"
    );
}

#[tokio::test]
async fn crate_deprecated_returns_seeded_deprecated_items() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    let source_file_id = seed_source_file(
        &context.state.db,
        context
            .fixture
            .dependent
            .version_id,
        "src/deprecated.rs",
        Some("rust"),
        "#[deprecated(since = \"1.0.100\", note = \"use new_parse instead\")]\npub fn old_parse() \
         -> bool { false }\n#[deprecated]\npub struct OldConfig {}",
    )
    .await
    .expect("failed to seed deprecated source file");

    // Seed a deprecated symbol.
    sqlx::query(
        "INSERT INTO symbols (
            crate_version_id,
            source_file_id,
            name,
            kind,
            signature,
            visibility,
            start_line,
            end_line,
            index_source,
            deprecated_since,
            deprecated_note
         ) VALUES (
            $1, $2, 'old_parse', 'function', 'pub fn old_parse() -> bool', 'public', 1, 1,
            'rustdoc_json', '1.0.100', 'use new_parse instead'
         )",
    )
    .bind(
        context
            .fixture
            .dependent
            .version_id,
    )
    .bind(source_file_id)
    .execute(&context.state.db)
    .await
    .expect("failed to seed deprecated symbol");

    // Seed a deprecated type.
    sqlx::query(
        "INSERT INTO crate_types (
            crate_version_id,
            source_file_id,
            type_name,
            kind,
            visibility,
            generic_params,
            fields,
            variants,
            start_line,
            end_line,
            index_source,
            deprecated_since,
            deprecated_note
         ) VALUES (
            $1, $2, 'OldConfig', 'struct', 'public', $3::JSONB, $4::JSONB, $5::JSONB, 3, 3,
            'rustdoc_json', NULL, 'use Config instead'
         )",
    )
    .bind(
        context
            .fixture
            .dependent
            .version_id,
    )
    .bind(source_file_id)
    .bind(json!([]))
    .bind(json!([]))
    .bind(json!([]))
    .execute(&context.state.db)
    .await
    .expect("failed to seed deprecated crate_type");

    let response = context
        .mcp
        .call_tool(
            "crate_deprecated",
            json!({
                "crate_name": "serde_json",
                "limit": 50
            }),
        )
        .await
        .expect("crate_deprecated call failed");
    let payload = common::structured_content(&response);

    let count = payload
        .get("count")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    assert!(count >= 2, "expected at least 2 deprecated items, got {count}: {payload}");

    let items = payload
        .get("deprecated_items")
        .and_then(Value::as_array)
        .expect("deprecated_items should be an array");

    assert!(
        items.iter().any(|item| {
            item.get("name")
                .and_then(Value::as_str)
                == Some("old_parse")
                && item
                    .get("deprecated_note")
                    .and_then(Value::as_str)
                    == Some("use new_parse instead")
        }),
        "expected old_parse with deprecation note in items: {items:?}"
    );
    assert!(
        items.iter().any(|item| {
            item.get("name")
                .and_then(Value::as_str)
                == Some("OldConfig")
        }),
        "expected OldConfig in deprecated items: {items:?}"
    );

    assert!(
        payload
            .get("has_more")
            .and_then(Value::as_bool)
            .is_some()
    );
    assert!(
        payload
            .get("confidence")
            .is_some()
    );
    assert!(
        payload
            .get("confidence_assessment")
            .is_some()
    );
}
