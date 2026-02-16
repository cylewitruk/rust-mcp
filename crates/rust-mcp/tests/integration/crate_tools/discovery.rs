use super::{Value, common, json, seed_source_file};

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

#[tokio::test]
async fn crate_api_and_versions_report_seeded_symbol_timeline() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    let api_response = context
        .mcp
        .call_tool("crate.api", json!({"crate_name": "serde_json", "limit": 20}))
        .await
        .expect("crate.api call failed");
    let api_payload = common::structured_content(&api_response);

    let symbols = api_payload
        .get("symbols")
        .and_then(Value::as_array)
        .expect("symbols should be an array");
    assert!(symbols.iter().any(|symbol| {
        symbol
            .get("name")
            .and_then(Value::as_str)
            == Some("from_str")
    }));

    let versions_response = context
        .mcp
        .call_tool("crate.versions", json!({"crate_name": "serde_json", "limit": 10}))
        .await
        .expect("crate.versions call failed");
    let versions_payload = common::structured_content(&versions_response);
    let versions = versions_payload
        .get("versions")
        .and_then(Value::as_array)
        .expect("versions should be an array");

    assert!(!versions.is_empty());
    assert!(
        versions
            .iter()
            .any(|version| {
                version
                    .get("is_latest")
                    .and_then(Value::as_bool)
                    == Some(true)
            })
    );
}

#[tokio::test]
async fn crate_api_prefers_rustdoc_json_symbols_when_available() {
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
            index_source
         ) VALUES (
            $1, $2, 'from_rustdoc', 'function', 'fn from_rustdoc() -> bool', 'public', 1, 1, \
         'rustdoc_json'
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
    .expect("failed to seed rustdoc-json symbol");

    let api_response = context
        .mcp
        .call_tool("crate.api", json!({"crate_name": "serde_json", "limit": 20}))
        .await
        .expect("crate.api call failed");
    let api_payload = common::structured_content(&api_response);

    let symbols = api_payload
        .get("symbols")
        .and_then(Value::as_array)
        .expect("symbols should be an array");
    assert!(!symbols.is_empty());
    assert!(symbols.iter().all(|symbol| {
        symbol
            .get("index_source")
            .and_then(Value::as_str)
            == Some("rustdoc_json")
    }));
    assert!(symbols.iter().any(|symbol| {
        symbol
            .get("name")
            .and_then(Value::as_str)
            == Some("from_rustdoc")
    }));
}

#[tokio::test]
async fn crate_intel_summarizes_dependencies_for_seeded_crate() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    let response = context
        .mcp
        .call_tool("crate.intel", json!({"crate_name": "serde_json"}))
        .await
        .expect("crate.intel call failed");
    let payload = common::structured_content(&response);

    let dependencies = payload
        .get("dependencies")
        .and_then(Value::as_array)
        .expect("dependencies should be an array");
    assert!(
        dependencies
            .iter()
            .any(|dep| {
                dep.get("crate_name")
                    .and_then(Value::as_str)
                    == Some("serde")
            })
    );
    assert!(
        dependencies
            .iter()
            .any(|dep| {
                dep.get("crate_name")
                    .and_then(Value::as_str)
                    == Some("indexmap")
                    && dep
                        .get("optional")
                        .and_then(Value::as_bool)
                        == Some(true)
            })
    );
}
