use super::{Value, common, json};

#[tokio::test]
async fn dependency_resolve_reports_seeded_crates_as_resolvable() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    let response = context
        .mcp
        .call_tool(
            "dependency_resolve",
            json!({
                "dependencies": [
                    { "name": "serde_json", "version_req": "^1.0" },
                    { "name": "serde", "version_req": "^1.0" }
                ],
                "check_features": true
            }),
        )
        .await
        .expect("dependency_resolve call failed");
    let payload = common::structured_content(&response);

    assert_eq!(
        payload
            .get("resolvable")
            .and_then(Value::as_bool),
        Some(true)
    );

    let resolved_versions = payload
        .get("resolved_versions")
        .and_then(Value::as_array)
        .expect("resolved_versions should be an array");
    assert!(
        resolved_versions
            .iter()
            .any(|entry| {
                entry
                    .get("name")
                    .and_then(Value::as_str)
                    == Some("serde_json")
            })
    );
    assert!(
        resolved_versions
            .iter()
            .any(|entry| {
                entry
                    .get("name")
                    .and_then(Value::as_str)
                    == Some("serde")
            })
    );

    let feature_summary = payload
        .get("feature_unification_summary")
        .and_then(Value::as_object)
        .expect("feature_unification_summary should be present when check_features is true");
    assert!(
        feature_summary
            .get("dependency_edges_evaluated")
            .and_then(Value::as_u64)
            .unwrap_or_default()
            >= 2
    );
}
