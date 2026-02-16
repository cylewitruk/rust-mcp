use super::{
    BTreeSet, Value, initialize_raw_session, json, jsonrpc_request, notify_initialized,
    parse_mcp_response_from_http, post_mcp_jsonrpc, start_ready_rust_mcp,
};

#[tokio::test]
async fn rust_mcp_container_serves_health_and_ready_endpoints() {
    let rust_mcp = start_ready_rust_mcp().await;

    let client = reqwest::Client::new();

    let healthz = client
        .get(format!("{}/healthz", rust_mcp.base_url()))
        .send()
        .await
        .expect("healthz request failed");
    assert!(healthz.status().is_success());

    let readyz = client
        .get(format!("{}/readyz", rust_mcp.base_url()))
        .send()
        .await
        .expect("readyz request failed");
    assert!(readyz.status().is_success());

    let mcp_response = client
        .post(rust_mcp.mcp_url())
        .body("{}")
        .send()
        .await
        .expect("mcp endpoint request failed");
    assert!(
        mcp_response
            .status()
            .is_client_error()
            || mcp_response
                .status()
                .is_success()
    );
}

#[tokio::test]
async fn rust_mcp_container_supports_initialize_and_ping_tool() {
    let rust_mcp = start_ready_rust_mcp().await;

    let initialize = rust_mcp
        .initialize_mcp()
        .await
        .expect("MCP initialize failed");
    assert!(
        initialize
            .get("result")
            .is_some()
    );

    let ping = rust_mcp
        .call_tool("ping", json!({ "message": "e2e-smoke" }))
        .await
        .expect("MCP tools/call ping failed");
    assert!(ping.get("result").is_some());
}

#[tokio::test]
async fn rust_mcp_container_initialize_response_matches_expected_contract() {
    let rust_mcp = start_ready_rust_mcp().await;

    let initialize = rust_mcp
        .initialize_only_mcp()
        .await
        .expect("MCP initialize failed");
    let result = initialize
        .get("result")
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("initialize response was missing result object: {initialize}"));

    let protocol_version = result
        .get("protocolVersion")
        .and_then(Value::as_str)
        .expect("initialize result.protocolVersion should be present");
    assert_eq!(
        protocol_version, "2025-03-26",
        "initialize should negotiate the server-supported MCP protocol version"
    );

    assert!(
        result
            .get("capabilities")
            .and_then(Value::as_object)
            .and_then(|caps| caps.get("tools"))
            .and_then(Value::as_object)
            .is_some(),
        "initialize result.capabilities.tools should be present and object-like: {initialize}"
    );

    let server_info = result
        .get("serverInfo")
        .and_then(Value::as_object)
        .expect("initialize result.serverInfo should be present");
    assert!(
        server_info
            .get("name")
            .and_then(Value::as_str)
            .is_some_and(|name| !name.trim().is_empty()),
        "initialize result.serverInfo.name should be a non-empty string: {initialize}"
    );
    assert!(
        server_info
            .get("version")
            .and_then(Value::as_str)
            .is_some_and(|version| !version.trim().is_empty()),
        "initialize result.serverInfo.version should be a non-empty string: {initialize}"
    );
    assert!(
        result
            .get("instructions")
            .and_then(Value::as_str)
            .is_some_and(|instructions| !instructions.trim().is_empty()),
        "initialize result.instructions should be present and non-empty: {initialize}"
    );
}

#[tokio::test]
async fn rust_mcp_container_negotiates_initialize_from_newer_protocol_version() {
    let rust_mcp = start_ready_rust_mcp().await;

    let requested_protocol_version = "2099-12-31";
    let client = reqwest::Client::new();
    let (session_id, initialize_payload) =
        initialize_raw_session(&client, rust_mcp.mcp_url(), 1, requested_protocol_version).await;

    let negotiated_protocol_version = initialize_payload
        .get("result")
        .and_then(|result| result.get("protocolVersion"))
        .and_then(Value::as_str)
        .expect("initialize result.protocolVersion should be present");
    assert_ne!(
        negotiated_protocol_version, requested_protocol_version,
        "initialize should not echo unsupported client protocol version"
    );
    assert_eq!(
        negotiated_protocol_version, "2025-03-26",
        "initialize should negotiate to server-supported MCP protocol version"
    );

    let initialized_notification =
        notify_initialized(&client, rust_mcp.mcp_url(), &session_id).await;
    assert!(
        initialized_notification
            .status()
            .is_success()
    );

    let ping_response = post_mcp_jsonrpc(
        &client,
        rust_mcp.mcp_url(),
        Some(&session_id),
        jsonrpc_request(
            2,
            "tools/call",
            json!({
                "name": "ping",
                "arguments": {"message": "negotiated-protocol"}
            }),
        ),
        "raw tools/call ping request failed",
    )
    .await;
    assert!(
        ping_response
            .status()
            .is_success()
    );
    let ping_payload =
        parse_mcp_response_from_http(ping_response, "failed to read tools/call ping response body")
            .await;
    assert!(
        ping_payload
            .get("result")
            .is_some()
    );
}

#[tokio::test]
async fn rust_mcp_container_supports_tools_list_after_initialize() {
    let rust_mcp = start_ready_rust_mcp().await;
    let _ = rust_mcp
        .initialize_mcp()
        .await
        .expect("MCP initialize failed");

    let tools_list = rust_mcp
        .list_tools_mcp()
        .await
        .expect("MCP tools/list failed");
    let tools = tools_list
        .get("result")
        .and_then(|result| result.get("tools"))
        .and_then(Value::as_array)
        .expect("MCP tools/list did not include result.tools");
    let listed_names = tools
        .iter()
        .filter_map(|tool| tool.get("name"))
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
    let expected_names = [
        "ping",
        "index.sync_crates",
        "index.status",
        "index.refresh",
        "crate.search",
        "crate.intel",
        "crate.features",
        "crate.api_diff",
        "crate.api",
        "crate.type_info",
        "crate.trait_impls",
        "crate.re_exports",
        "crate.error_types",
        "crate.derive_macros",
        "crate.compare",
        "crate.compatibility",
        "crate.compatibility_matrix",
        "crate.migration_path",
        "crate.license_check",
        "crate.alternatives",
        "crate.versions",
        "crate.graph",
        "crate.hotspots",
        "dependency.audit",
        "dependency.resolve",
        "dependency.feature_impact",
        "source.search",
        "source.read",
        "source.context",
        "symbol.search",
        "docs.search",
        "crate.usage_patterns",
    ]
    .into_iter()
    .map(str::to_string)
    .collect::<BTreeSet<_>>();
    assert_eq!(
        listed_names, expected_names,
        "tools/list contract changed; update expected tool set if intentional"
    );
}

#[tokio::test]
async fn rust_mcp_container_tools_list_entries_include_descriptions_and_input_schemas() {
    let rust_mcp = start_ready_rust_mcp().await;
    let _ = rust_mcp
        .initialize_mcp()
        .await
        .expect("MCP initialize failed");

    let tools_list = rust_mcp
        .list_tools_mcp()
        .await
        .expect("MCP tools/list failed");
    let tools = tools_list
        .get("result")
        .and_then(|result| result.get("tools"))
        .and_then(Value::as_array)
        .expect("MCP tools/list did not include result.tools");
    assert!(!tools.is_empty(), "tools/list returned no tool entries");

    let listed_names = tools
        .iter()
        .filter_map(|tool| tool.get("name"))
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect::<Vec<_>>();
    let unique_names = listed_names
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    assert_eq!(
        listed_names.len(),
        unique_names.len(),
        "tools/list should not include duplicate tool names"
    );

    for tool in tools {
        let name = tool
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("tool entry missing name: {tool}"));
        assert!(!name.trim().is_empty(), "tool entry has empty name: {tool}");

        let description = tool
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("tool `{name}` missing description: {tool}"));
        assert!(!description.trim().is_empty(), "tool `{name}` has empty description");

        let input_schema = tool
            .get("inputSchema")
            .or_else(|| tool.get("input_schema"))
            .and_then(Value::as_object)
            .unwrap_or_else(|| panic!("tool `{name}` missing object input schema: {tool}"));
        assert!(
            input_schema.contains_key("type")
                || input_schema.contains_key("oneOf")
                || input_schema.contains_key("anyOf")
                || input_schema.contains_key("allOf"),
            "tool `{name}` input schema is missing expected schema keys: {input_schema:?}"
        );
        if let Some(schema_type) = input_schema
            .get("type")
            .and_then(Value::as_str)
        {
            assert_eq!(schema_type, "object", "tool `{name}` expected object input schema");
        }
    }
}

#[tokio::test]
async fn rust_mcp_container_rejects_tools_list_before_initialize() {
    let rust_mcp = start_ready_rust_mcp().await;

    let pre_initialize_tools_list = rust_mcp
        .list_tools_mcp()
        .await;
    assert!(
        pre_initialize_tools_list.is_err(),
        "tools/list unexpectedly succeeded before initialize: {pre_initialize_tools_list:?}"
    );
}
