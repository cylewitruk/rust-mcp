//! Integration tests for `crate.*` MCP tools.

mod common;

use serde_json::{Value, json};

#[tokio::test]
async fn crate_search_returns_seeded_crates_without_freshness_probe() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    let response = context
        .mcp
        .call_tool("crate.search", json!({"query": "serde", "limit": 10}))
        .await
        .expect("crate.search call failed");
    let payload = common::structured_content(&response);

    let count = payload
        .get("count")
        .and_then(Value::as_u64)
        .expect("count should be a u64");
    assert!(count >= 2);
    assert_eq!(
        payload
            .get("freshness_checks_performed")
            .and_then(Value::as_u64),
        Some(0)
    );
    assert_eq!(
        payload
            .get("refresh_jobs_enqueued")
            .and_then(Value::as_u64),
        Some(0)
    );
}

#[tokio::test]
async fn crate_features_resolves_default_and_dependency_backed_feature() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    let response = context
        .mcp
        .call_tool("crate.features", json!({"crate_name": "serde_json"}))
        .await
        .expect("crate.features call failed");
    let payload = common::structured_content(&response);

    assert_eq!(
        payload
            .get("feature_count")
            .and_then(Value::as_u64),
        Some(2)
    );

    let default_features = payload
        .get("default_features")
        .and_then(Value::as_array)
        .expect("default_features should be an array");
    assert!(
        default_features
            .iter()
            .any(|feature| feature.as_str() == Some("preserve_order"))
    );

    let preserve_order = payload
        .get("features")
        .and_then(Value::as_array)
        .and_then(|features| {
            features
                .iter()
                .find(|feature| {
                    feature
                        .get("name")
                        .and_then(Value::as_str)
                        == Some("preserve_order")
                })
        })
        .expect("expected preserve_order feature entry");

    let enabled_dependencies = preserve_order
        .get("enables_dependencies")
        .and_then(Value::as_array)
        .expect("enables_dependencies should be an array");
    assert!(
        enabled_dependencies
            .iter()
            .any(|dep| dep.as_str() == Some("indexmap"))
    );
}

#[tokio::test]
async fn crate_graph_dependencies_contains_seeded_edges() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    let response = context
        .mcp
        .call_tool(
            "crate.graph",
            json!({
                "crate_name": "serde_json",
                "direction": "dependencies",
                "depth": 2
            }),
        )
        .await
        .expect("crate.graph call failed");
    let payload = common::structured_content(&response);

    let edges = payload
        .get("edges")
        .and_then(Value::as_array)
        .expect("edges should be an array");
    assert!(edges.len() >= 2);
    assert!(edges.iter().any(|edge| {
        edge.get("to_crate")
            .and_then(Value::as_str)
            == Some("serde")
    }));
    assert!(edges.iter().any(|edge| {
        edge.get("to_crate")
            .and_then(Value::as_str)
            == Some("indexmap")
            && edge
                .get("optional")
                .and_then(Value::as_bool)
                == Some(true)
    }));
}
