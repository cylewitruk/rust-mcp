//! Integration tests for `dependency.*` MCP tools.

mod common;

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

fn write_temp_manifest(contents: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("rust-mcp-dep-audit-{nanos}"));
    std::fs::create_dir_all(&dir).expect("failed to create temporary manifest directory");
    let manifest_path = dir.join("Cargo.toml");
    std::fs::write(&manifest_path, contents).expect("failed to write temporary Cargo.toml");
    manifest_path
}

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

#[tokio::test]
async fn dependency_resolve_reports_seeded_crates_as_resolvable() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    let response = context
        .mcp
        .call_tool(
            "dependency.resolve",
            json!({
                "dependencies": [
                    { "name": "serde_json", "version_req": "^1.0" },
                    { "name": "serde", "version_req": "^1.0" }
                ],
                "check_features": true
            }),
        )
        .await
        .expect("dependency.resolve call failed");
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
        .call_tool("dependency.audit", json!({ "cargo_toml_path": manifest_path }))
        .await
        .expect("dependency.audit call failed");
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
