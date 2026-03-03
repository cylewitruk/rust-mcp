use super::{Value, common, json};

#[tokio::test]
async fn crate_compatibility_resolves_seeded_pair() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    let response = context
        .mcp
        .call_tool(
            "crate_compatibility",
            json!({
                "left_crate": "serde_json",
                "left_version": "1.0.145",
                "right_crate": "serde",
                "right_version": "1.0.228",
                "check_features": true
            }),
        )
        .await
        .expect("crate_compatibility call failed");
    let payload = common::structured_content(&response);

    assert_eq!(
        payload
            .get("resolvable")
            .and_then(Value::as_bool),
        Some(true)
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
}

#[tokio::test]
async fn crate_compatibility_matrix_tests_seeded_version_pair() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    let response = context
        .mcp
        .call_tool(
            "crate_compatibility_matrix",
            json!({
                "left_crate": "serde_json",
                "right_crate": "serde",
                "left_versions": ["1.0.145"],
                "right_versions": ["1.0.228"],
                "check_features": true
            }),
        )
        .await
        .expect("crate_compatibility_matrix call failed");
    let payload = common::structured_content(&response);

    assert_eq!(
        payload
            .get("pairs_tested")
            .and_then(Value::as_u64),
        Some(1)
    );
    assert_eq!(
        payload
            .get("compatible_pairs")
            .and_then(Value::as_array)
            .map(|pairs| pairs.len()),
        Some(1)
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
async fn crate_license_check_returns_policy_for_seeded_crate() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    let response = context
        .mcp
        .call_tool("crate_license_check", json!({"crate_name": "serde_json"}))
        .await
        .expect("crate_license_check call failed");
    let payload = common::structured_content(&response);

    assert_eq!(
        payload
            .get("policy_result")
            .and_then(Value::as_str),
        Some("unknown")
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
}

#[tokio::test]
async fn crate_alternatives_returns_expected_shape() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    let response = context
        .mcp
        .call_tool("crate_alternatives", json!({"crate_name": "serde_json", "limit": 5}))
        .await
        .expect("crate_alternatives call failed");
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
async fn crate_compare_and_compatibility_resolve_for_seeded_crates() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    let compare_response = context
        .mcp
        .call_tool(
            "crate_compare",
            json!({
                "left_crate": "indexmap",
                "right_crate": "serde"
            }),
        )
        .await
        .expect("crate_compare call failed");
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
            "crate_compatibility",
            json!({
                "left_crate": "serde_json",
                "left_version": "1.0.145",
                "right_crate": "serde",
                "right_version": "1.0.228",
                "check_features": true
            }),
        )
        .await
        .expect("crate_compatibility call failed");
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
            "crate_compatibility_matrix",
            json!({
                "left_crate": "serde_json",
                "right_crate": "serde",
                "left_versions": ["1.0.145"],
                "right_versions": ["1.0.228"],
                "check_features": true
            }),
        )
        .await
        .expect("crate_compatibility_matrix call failed");
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
        .call_tool("crate_license_check", json!({"crate_name": "serde_json"}))
        .await
        .expect("crate_license_check call failed");
    let license_payload = common::structured_content(&license_response);
    assert_eq!(
        license_payload
            .get("policy_result")
            .and_then(Value::as_str),
        Some("unknown")
    );

    let alternatives_response = context
        .mcp
        .call_tool("crate_alternatives", json!({"crate_name": "serde_json", "limit": 5}))
        .await
        .expect("crate_alternatives call failed");
    let alternatives_payload = common::structured_content(&alternatives_response);
    assert!(
        alternatives_payload
            .get("count")
            .and_then(Value::as_u64)
            .unwrap_or_default()
            >= 1
    );
    assert_eq!(
        alternatives_payload
            .get("page")
            .and_then(Value::as_u64),
        Some(1)
    );
    assert!(
        alternatives_payload
            .get("has_more")
            .and_then(Value::as_bool)
            .is_some()
    );
    assert!(
        alternatives_payload
            .get("truncated")
            .and_then(Value::as_bool)
            .is_some()
    );
    assert!(
        alternatives_payload
            .get("next_cursor")
            .is_some()
    );
}
