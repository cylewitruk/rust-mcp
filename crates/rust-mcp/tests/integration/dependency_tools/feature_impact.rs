use super::{Value, common, json};

#[tokio::test]
async fn dependency_feature_impact_reports_optional_dependency_expansion() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    let response = context
        .mcp
        .call_tool(
            "dependency.feature_impact",
            json!({
                "crate_name": "serde_json",
                "features": ["preserve_order"],
                "heavy_threshold": 1
            }),
        )
        .await
        .expect("dependency.feature_impact call failed");
    let payload = common::structured_content(&response);

    assert_eq!(
        payload
            .get("baseline_dependency_count")
            .and_then(Value::as_u64),
        Some(1)
    );
    assert_eq!(
        payload
            .get("combined_dependency_count")
            .and_then(Value::as_u64),
        Some(2)
    );

    let per_feature = payload
        .get("per_feature")
        .and_then(Value::as_array)
        .expect("per_feature should be an array");
    let preserve_order = per_feature
        .iter()
        .find(|entry| {
            entry
                .get("feature")
                .and_then(Value::as_str)
                == Some("preserve_order")
        })
        .expect("preserve_order feature entry should exist");
    assert_eq!(
        preserve_order
            .get("additional_dependency_count")
            .and_then(Value::as_u64),
        Some(1)
    );
    let additional_dependencies = preserve_order
        .get("additional_dependencies")
        .and_then(Value::as_array)
        .expect("additional_dependencies should be an array");
    assert!(
        additional_dependencies
            .iter()
            .any(|dep| dep.as_str() == Some("indexmap"))
    );
}
