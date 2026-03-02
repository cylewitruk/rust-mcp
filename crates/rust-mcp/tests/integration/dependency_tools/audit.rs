use super::{Value, common, json, write_temp_manifest};

#[tokio::test]
async fn dependency_audit_flags_unresolved_manifest_dependencies() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");
    let manifest_path = write_temp_manifest(
        r#"[package]
name = "integration-audit"
version = "0.1.0"
edition = "2021"
rust-version = "1.70"

[dependencies]
serde_json = "1.0"
definitely_missing_crate = "1.0"
"#,
    );

    let response = context
        .mcp
        .call_tool("dependency_audit", json!({ "cargo_toml_path": manifest_path }))
        .await
        .expect("dependency_audit call failed");
    let payload = common::structured_content(&response);

    assert_eq!(
        payload
            .get("dependency_count")
            .and_then(Value::as_u64),
        Some(2)
    );
    assert!(
        payload
            .get("issue_count")
            .and_then(Value::as_u64)
            .unwrap_or_default()
            >= 1
    );

    let issues = payload
        .get("issues")
        .and_then(Value::as_array)
        .expect("issues should be an array");
    assert!(issues.iter().any(|issue| {
        issue
            .get("dependency_name")
            .and_then(Value::as_str)
            == Some("definitely_missing_crate")
            && issue
                .get("category")
                .and_then(Value::as_str)
                == Some("unresolved")
    }));
}
