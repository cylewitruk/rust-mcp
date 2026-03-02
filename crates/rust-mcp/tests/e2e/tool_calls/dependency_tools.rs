use serde_json::{Value, json};

use super::{
    SEEDED_ALT_CRATE_NAME, SEEDED_CRATE_NAME, SEEDED_CRATE_NEXT_VERSION, call_tool_payload,
    seeded_indexed_context, seeded_indexed_manifest_context,
};

#[tokio::test]
async fn tool_dependency_feature_impact_reports_per_feature_entry() {
    let context = seeded_indexed_context().await;

    let payload = call_tool_payload(
        &context.rust_mcp,
        "dependency_feature_impact",
        json!({
            "crate_name": SEEDED_CRATE_NAME,
            "version": SEEDED_CRATE_NEXT_VERSION,
            "features": ["std"],
            "heavy_threshold": 1
        }),
    )
    .await;

    assert!(
        payload
            .get("combined_dependency_count")
            .and_then(Value::as_u64)
            .zip(
                payload
                    .get("baseline_dependency_count")
                    .and_then(Value::as_u64)
            )
            .is_some_and(|(combined, baseline)| combined >= baseline),
        "expected combined dependency count to be >= baseline: {payload}"
    );

    let per_feature = payload
        .get("per_feature")
        .and_then(Value::as_array)
        .expect("dependency_feature_impact should return per_feature array");
    assert!(
        per_feature
            .iter()
            .any(|entry| entry
                .get("feature")
                .and_then(Value::as_str)
                == Some("std")),
        "expected per_feature to include std entry: {per_feature:?}"
    );
}

#[tokio::test]
async fn tool_dependency_resolve_resolves_seeded_dependencies() {
    let context = seeded_indexed_context().await;

    let payload = call_tool_payload(
        &context.rust_mcp,
        "dependency_resolve",
        json!({
            "dependencies": [
                {"name": SEEDED_CRATE_NAME, "version_req": "^1.2"},
                {"name": SEEDED_ALT_CRATE_NAME, "version_req": "^0.9"}
            ],
            "check_features": true,
            "limit": 5
        }),
    )
    .await;

    assert_eq!(
        payload
            .get("resolvable")
            .and_then(Value::as_bool),
        Some(true)
    );

    let resolved_versions = payload
        .get("resolved_versions")
        .and_then(Value::as_array)
        .expect("dependency_resolve should return resolved_versions array");
    assert!(
        resolved_versions
            .iter()
            .any(|entry| entry
                .get("name")
                .and_then(Value::as_str)
                == Some(SEEDED_CRATE_NAME)),
        "expected resolved_versions to include {SEEDED_CRATE_NAME}: {resolved_versions:?}"
    );
    assert!(
        resolved_versions
            .iter()
            .any(|entry| {
                entry
                    .get("name")
                    .and_then(Value::as_str)
                    == Some(SEEDED_ALT_CRATE_NAME)
            }),
        "expected resolved_versions to include {SEEDED_ALT_CRATE_NAME}: {resolved_versions:?}"
    );
}

#[tokio::test]
async fn tool_dependency_audit_reports_seeded_manifest_dependencies() {
    let context = seeded_indexed_manifest_context().await;

    let payload = call_tool_payload(
        &context.rust_mcp,
        "dependency_audit",
        json!({ "cargo_toml_path": context.manifest_path }),
    )
    .await;

    assert_eq!(
        payload
            .get("dependency_count")
            .and_then(Value::as_u64),
        Some(2)
    );

    let dependencies = payload
        .get("dependencies")
        .and_then(Value::as_array)
        .expect("dependency_audit should return dependencies array");
    assert!(
        dependencies
            .iter()
            .any(|dependency| {
                dependency
                    .get("dependency_name")
                    .and_then(Value::as_str)
                    == Some(SEEDED_CRATE_NAME)
            }),
        "expected dependency.audit dependencies to include {SEEDED_CRATE_NAME}: {dependencies:?}"
    );
    assert!(
        dependencies
            .iter()
            .any(|dependency| {
                dependency
                    .get("dependency_name")
                    .and_then(Value::as_str)
                    == Some(SEEDED_ALT_CRATE_NAME)
            }),
        "expected dependency.audit dependencies to include {SEEDED_ALT_CRATE_NAME}: \
         {dependencies:?}"
    );
}
