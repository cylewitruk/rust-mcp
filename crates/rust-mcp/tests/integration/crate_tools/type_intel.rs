use super::{Value, common, json, seed_type_intelligence_fixture};

#[tokio::test]
async fn crate_type_info_returns_seeded_type_definition() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");
    seed_type_intelligence_fixture(&context).await;

    let response = context
        .mcp
        .call_tool(
            "crate_type_info",
            json!({
                "crate_name": "serde_json",
                "type_name": "ParseError",
                "include_methods": true,
                "include_trait_impls": true
            }),
        )
        .await
        .expect("crate_type_info call failed");
    let payload = common::structured_content(&response);

    let type_definition = payload
        .get("type_definition")
        .and_then(Value::as_object)
        .expect("type_definition should be present");
    assert_eq!(
        type_definition
            .get("type_name")
            .and_then(Value::as_str),
        Some("ParseError")
    );
    assert_eq!(
        type_definition
            .get("kind")
            .and_then(Value::as_str),
        Some("struct")
    );

    let inherent_methods = payload
        .get("inherent_methods")
        .and_then(Value::as_array)
        .expect("inherent_methods should be an array");
    assert!(
        inherent_methods
            .iter()
            .any(|method| {
                method
                    .get("name")
                    .and_then(Value::as_str)
                    == Some("new")
            })
    );

    let conversions = payload
        .get("conversions")
        .and_then(Value::as_array)
        .expect("conversions should be an array");
    assert!(
        conversions
            .iter()
            .any(|conversion| {
                conversion
                    .get("trait_name")
                    .and_then(Value::as_str)
                    == Some("From")
                    && conversion
                        .get("source_type")
                        .and_then(Value::as_str)
                        == Some("String")
            })
    );

    let trait_definitions = payload
        .get("trait_definitions")
        .and_then(Value::as_array)
        .expect("trait_definitions should be an array");
    assert!(
        trait_definitions
            .iter()
            .any(|definition| {
                definition
                    .get("trait_name")
                    .and_then(Value::as_str)
                    == Some("Display")
            })
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
    assert!(
        payload
            .get("suggested_next_tools")
            .is_some()
    );
    assert!(
        payload
            .get("provenance")
            .is_some()
    );
}

#[tokio::test]
async fn crate_trait_impls_returns_seeded_impl_rows() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");
    seed_type_intelligence_fixture(&context).await;

    let response = context
        .mcp
        .call_tool(
            "crate_trait_impls",
            json!({
                "crate_name": "serde_json",
                "type_name": "ParseError",
                "limit": 20
            }),
        )
        .await
        .expect("crate_trait_impls call failed");
    let payload = common::structured_content(&response);

    assert!(
        payload
            .get("count")
            .and_then(Value::as_u64)
            .unwrap_or_default()
            >= 3
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

    let trait_definitions = payload
        .get("trait_definitions")
        .and_then(Value::as_array)
        .expect("trait_definitions should be an array");
    assert!(
        trait_definitions
            .iter()
            .any(|definition| {
                definition
                    .get("trait_name")
                    .and_then(Value::as_str)
                    == Some("Display")
            })
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
async fn crate_error_types_returns_seeded_error_type() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");
    seed_type_intelligence_fixture(&context).await;

    let response = context
        .mcp
        .call_tool(
            "crate_error_types",
            json!({
                "crate_name": "serde_json",
                "type_name": "ParseError",
                "limit": 10
            }),
        )
        .await
        .expect("crate_error_types call failed");
    let payload = common::structured_content(&response);

    assert_eq!(
        payload
            .get("count")
            .and_then(Value::as_u64),
        Some(1)
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

    let error_entry = payload
        .get("error_types")
        .and_then(Value::as_array)
        .and_then(|entries| entries.first())
        .expect("expected one error_types entry");
    assert_eq!(
        error_entry
            .get("type_name")
            .and_then(Value::as_str),
        Some("ParseError")
    );
    assert!(
        error_entry
            .get("from_conversions")
            .and_then(Value::as_array)
            .is_some_and(|values| values
                .iter()
                .any(|value| value.as_str() == Some("String")))
    );
    assert!(
        error_entry
            .get("returned_by")
            .and_then(Value::as_array)
            .is_some_and(|values| {
                values.iter().any(|value| {
                    value
                        .as_str()
                        .is_some_and(|text| text.contains("parse"))
                })
            })
    );
    assert!(
        error_entry
            .get("display_patterns")
            .and_then(Value::as_array)
            .is_some_and(|values| {
                values.iter().any(|value| {
                    value
                        .as_str()
                        .is_some_and(|text| text.contains("parse error"))
                })
            })
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
async fn crate_type_info_trait_impls_and_error_types_handle_sparse_index_data() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    let type_info_response = context
        .mcp
        .call_tool(
            "crate_type_info",
            json!({
                "crate_name": "serde_json",
                "type_name": "MissingType"
            }),
        )
        .await
        .expect("crate_type_info call failed");
    let type_info_payload = common::structured_content(&type_info_response);
    assert!(
        type_info_payload
            .get("type_definition")
            .is_some_and(Value::is_null)
    );
    assert!(
        type_info_payload
            .get("trait_definitions")
            .and_then(Value::as_array)
            .is_some_and(|definitions| definitions.is_empty())
    );

    let trait_impls_response = context
        .mcp
        .call_tool(
            "crate_trait_impls",
            json!({
                "crate_name": "serde_json",
                "type_name": "MissingType",
                "limit": 10
            }),
        )
        .await
        .expect("crate_trait_impls call failed");
    let trait_impls_payload = common::structured_content(&trait_impls_response);
    assert_eq!(
        trait_impls_payload
            .get("page")
            .and_then(Value::as_u64),
        Some(1)
    );
    assert!(
        trait_impls_payload
            .get("has_more")
            .and_then(Value::as_bool)
            .is_some()
    );
    assert!(
        trait_impls_payload
            .get("truncated")
            .and_then(Value::as_bool)
            .is_some()
    );
    assert!(
        trait_impls_payload
            .get("next_cursor")
            .is_some()
    );
    assert_eq!(
        trait_impls_payload
            .get("count")
            .and_then(Value::as_u64),
        Some(0)
    );
    assert!(
        trait_impls_payload
            .get("trait_definitions")
            .and_then(Value::as_array)
            .is_some_and(|definitions| definitions.is_empty())
    );

    let error_types_response = context
        .mcp
        .call_tool(
            "crate_error_types",
            json!({
                "crate_name": "serde_json",
                "limit": 10
            }),
        )
        .await
        .expect("crate_error_types call failed");
    let error_types_payload = common::structured_content(&error_types_response);
    assert_eq!(
        error_types_payload
            .get("page")
            .and_then(Value::as_u64),
        Some(1)
    );
    assert!(
        error_types_payload
            .get("has_more")
            .and_then(Value::as_bool)
            .is_some()
    );
    assert!(
        error_types_payload
            .get("truncated")
            .and_then(Value::as_bool)
            .is_some()
    );
    assert!(
        error_types_payload
            .get("next_cursor")
            .is_some()
    );
    assert_eq!(
        error_types_payload
            .get("count")
            .and_then(Value::as_u64),
        Some(0)
    );
}

#[tokio::test]
async fn crate_type_info_trait_impls_and_error_types_return_seeded_type_intelligence() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");
    seed_type_intelligence_fixture(&context).await;

    let type_info_response = context
        .mcp
        .call_tool(
            "crate_type_info",
            json!({
                "crate_name": "serde_json",
                "type_name": "ParseError",
                "include_methods": true,
                "include_trait_impls": true
            }),
        )
        .await
        .expect("crate_type_info call failed");
    let type_info_payload = common::structured_content(&type_info_response);

    let type_definition = type_info_payload
        .get("type_definition")
        .and_then(Value::as_object)
        .expect("type_definition should be present");
    assert_eq!(
        type_definition
            .get("type_name")
            .and_then(Value::as_str),
        Some("ParseError")
    );
    assert_eq!(
        type_definition
            .get("kind")
            .and_then(Value::as_str),
        Some("struct")
    );

    let inherent_methods = type_info_payload
        .get("inherent_methods")
        .and_then(Value::as_array)
        .expect("inherent_methods should be an array");
    assert!(
        inherent_methods
            .iter()
            .any(|method| {
                method
                    .get("name")
                    .and_then(Value::as_str)
                    == Some("new")
            })
    );

    let conversions = type_info_payload
        .get("conversions")
        .and_then(Value::as_array)
        .expect("conversions should be an array");
    assert!(
        conversions
            .iter()
            .any(|conversion| {
                conversion
                    .get("trait_name")
                    .and_then(Value::as_str)
                    == Some("From")
                    && conversion
                        .get("source_type")
                        .and_then(Value::as_str)
                        == Some("String")
                    && conversion
                        .get("target_type")
                        .and_then(Value::as_str)
                        == Some("ParseError")
            })
    );

    let type_info_trait_definitions = type_info_payload
        .get("trait_definitions")
        .and_then(Value::as_array)
        .expect("trait_definitions should be an array");
    assert!(
        type_info_trait_definitions
            .iter()
            .any(|definition| {
                definition
                    .get("trait_name")
                    .and_then(Value::as_str)
                    == Some("Display")
                    && definition
                        .get("is_dyn_compatible")
                        .and_then(Value::as_bool)
                        == Some(true)
            })
    );

    let trait_impls_response = context
        .mcp
        .call_tool(
            "crate_trait_impls",
            json!({
                "crate_name": "serde_json",
                "type_name": "ParseError",
                "limit": 20
            }),
        )
        .await
        .expect("crate_trait_impls call failed");
    let trait_impls_payload = common::structured_content(&trait_impls_response);
    assert_eq!(
        trait_impls_payload
            .get("page")
            .and_then(Value::as_u64),
        Some(1)
    );
    assert!(
        trait_impls_payload
            .get("has_more")
            .and_then(Value::as_bool)
            .is_some()
    );
    assert!(
        trait_impls_payload
            .get("truncated")
            .and_then(Value::as_bool)
            .is_some()
    );
    assert!(
        trait_impls_payload
            .get("next_cursor")
            .is_some()
    );
    assert!(
        trait_impls_payload
            .get("count")
            .and_then(Value::as_u64)
            .unwrap_or_default()
            >= 3
    );

    let trait_impls_definitions = trait_impls_payload
        .get("trait_definitions")
        .and_then(Value::as_array)
        .expect("trait_definitions should be an array");
    assert!(
        trait_impls_definitions
            .iter()
            .any(|definition| {
                definition
                    .get("trait_name")
                    .and_then(Value::as_str)
                    == Some("Display")
            })
    );

    let error_types_response = context
        .mcp
        .call_tool(
            "crate_error_types",
            json!({
                "crate_name": "serde_json",
                "type_name": "ParseError",
                "limit": 10
            }),
        )
        .await
        .expect("crate_error_types call failed");
    let error_types_payload = common::structured_content(&error_types_response);
    assert_eq!(
        error_types_payload
            .get("page")
            .and_then(Value::as_u64),
        Some(1)
    );
    assert!(
        error_types_payload
            .get("has_more")
            .and_then(Value::as_bool)
            .is_some()
    );
    assert!(
        error_types_payload
            .get("truncated")
            .and_then(Value::as_bool)
            .is_some()
    );
    assert!(
        error_types_payload
            .get("next_cursor")
            .is_some()
    );
    assert_eq!(
        error_types_payload
            .get("count")
            .and_then(Value::as_u64),
        Some(1)
    );

    let error_entry = error_types_payload
        .get("error_types")
        .and_then(Value::as_array)
        .and_then(|entries| entries.first())
        .expect("expected one error_types entry");
    assert_eq!(
        error_entry
            .get("type_name")
            .and_then(Value::as_str),
        Some("ParseError")
    );
    assert!(
        error_entry
            .get("from_conversions")
            .and_then(Value::as_array)
            .is_some_and(|values| values
                .iter()
                .any(|value| value.as_str() == Some("String")))
    );
    assert!(
        error_entry
            .get("returned_by")
            .and_then(Value::as_array)
            .is_some_and(|values| {
                values.iter().any(|value| {
                    value
                        .as_str()
                        .is_some_and(|text| text.contains("parse"))
                })
            })
    );
    assert!(
        error_entry
            .get("display_patterns")
            .and_then(Value::as_array)
            .is_some_and(|values| {
                values.iter().any(|value| {
                    value
                        .as_str()
                        .is_some_and(|text| text.contains("parse error"))
                })
            })
    );
}

/// Verifies that `crate_type_info` handles rows where JSONB columns contain
/// JSON `null` (as opposed to `[]`). This reproduces the bug where syn-indexed
/// types used `..Default::default()` which set JSONB fields to
/// `serde_json::Value::Null`.
#[tokio::test]
async fn crate_type_info_handles_json_null_in_where_clauses() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    let source_file_id = rust_mcp_testing::fixtures::seed_source_file(
        &context.state.db,
        context
            .fixture
            .dependent
            .version_id,
        "src/null_test.rs",
        Some("rust"),
        "pub struct NullWhereType;",
    )
    .await
    .expect("failed to seed source file");

    // Insert a type row with explicit JSON null for where_clauses, auto_traits,
    // and generic_params — reproducing the pre-fix `Default` behaviour.
    sqlx::query(
        "INSERT INTO crate_types (
            crate_version_id, source_file_id, type_name, kind,
            visibility, generic_params, fields, variants,
            start_line, end_line, index_source,
            where_clauses, auto_traits
         ) VALUES (
            $1, $2, 'NullWhereType', 'struct', 'public',
            'null'::jsonb, '[]'::jsonb, '[]'::jsonb,
            1, 1, 'syn',
            'null'::jsonb, 'null'::jsonb
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
    .expect("failed to seed type with null JSONB columns");

    // This call previously failed with:
    // "error occurred while decoding column 'where_clauses': invalid type: null"
    let response = context
        .mcp
        .call_tool(
            "crate_type_info",
            json!({
                "crate_name": "serde_json",
                "type_name": "NullWhereType"
            }),
        )
        .await
        .expect("crate_type_info should handle null JSONB gracefully");
    let payload = common::structured_content(&response);

    let type_def = payload
        .get("type_definition")
        .and_then(Value::as_object)
        .expect("type_definition should be present");
    assert_eq!(
        type_def
            .get("type_name")
            .and_then(Value::as_str),
        Some("NullWhereType")
    );
}
