use super::{Value, common, json};

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
