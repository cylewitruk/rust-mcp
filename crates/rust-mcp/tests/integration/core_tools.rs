//! Integration tests for namespace-free MCP tools.

use rust_mcp_types::tools as tool_descriptors;
use serde_json::{Value, json};

use super::common;

#[tokio::test]
async fn ping_tool_returns_db_ready() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    let response = context
        .mcp
        .call_tool("ping", json!({"message": "integration"}))
        .await
        .expect("ping call failed");
    let text = common::first_content_text(&response);

    assert!(text.contains("pong (db_ready) integration"));
}

#[tokio::test]
async fn schema_get_returns_contract_for_known_tool() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    let response = context
        .mcp
        .call_tool("schema_get", json!({"tool_name": "ping"}))
        .await
        .expect("schema_get call failed");
    let payload = common::structured_content(&response);

    assert_eq!(
        payload
            .get("total_tools")
            .and_then(Value::as_u64),
        Some(1)
    );

    let first_schema = payload
        .get("schemas")
        .and_then(Value::as_array)
        .and_then(|schemas| schemas.first())
        .expect("schema_get should return one schema entry");
    assert_eq!(
        first_schema
            .get("tool_name")
            .and_then(Value::as_str),
        Some("ping")
    );
    assert_eq!(
        first_schema
            .get("request")
            .and_then(|schema| schema.get("type"))
            .and_then(Value::as_str),
        Some("object")
    );
}

/// Names needing Base64 header encoding must pass rmcp's standard-header
/// validation; the call then fails only because the tool is unknown.
#[tokio::test]
async fn stateless_call_with_encoded_tool_name_passes_header_validation() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    let error = context
        .mcp
        .call_tool("ツール", json!({}))
        .await
        .expect_err("unknown tool should fail")
        .to_string();

    assert!(!error.contains("-32020"), "Mcp-Name header was rejected: {error}");
    assert!(error.contains("ツール"), "expected an unknown-tool error: {error}");
}

/// Agents commonly send `crate` instead of `crate_name`.
#[tokio::test]
async fn crate_is_accepted_as_alias_for_crate_name() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    let response = context
        .mcp
        .call_tool(
            "source_read",
            json!({"crate": "serde_json", "version": "1.0.145", "path": "src/lib.rs"}),
        )
        .await
        .expect("source_read with `crate` alias should succeed");
    assert!(common::first_content_text(&response).contains("from_str"));
}

#[tokio::test]
async fn invalid_arguments_report_expected_parameters() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    let error = context
        .mcp
        .call_tool("source_read", json!({"crate_nam": "serde_json"}))
        .await
        .expect_err("missing required arguments should fail")
        .to_string();

    assert!(
        error.contains("Parameters for `source_read`: crate_name (required), path (required)"),
        "expected a schema-derived parameter summary: {error}"
    );
}

/// Served descriptions must be the shared descriptors, not divergent copies.
#[tokio::test]
async fn tool_descriptions_match_shared_descriptors() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    let tools_list = context
        .mcp
        .rpc_call("tools/list", json!({}))
        .await
        .expect("tools/list");
    let tools = tools_list
        .pointer("/result/tools")
        .and_then(Value::as_array)
        .expect("tools array");

    assert_eq!(tools.len(), tool_descriptors::ALL_TOOLS.len());
    for descriptor in tool_descriptors::ALL_TOOLS {
        let served = tools
            .iter()
            .find(|tool| tool.get("name") == Some(&json!(descriptor.name)))
            .unwrap_or_else(|| panic!("tool `{}` not served", descriptor.name));
        assert_eq!(served.get("description"), Some(&json!(descriptor.description)));
    }
}
