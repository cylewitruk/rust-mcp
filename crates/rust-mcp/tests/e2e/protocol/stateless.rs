use super::{
    StatusCode, Value, json, jsonrpc_request, parse_mcp_response_from_http, post_stateless_jsonrpc,
    start_ready_rust_mcp,
};

#[tokio::test]
async fn rust_mcp_container_discover_response_matches_expected_contract() {
    let rust_mcp = start_ready_rust_mcp().await;

    let discover = rust_mcp
        .connect_mcp()
        .await
        .expect("MCP server/discover failed");
    let result = discover
        .get("result")
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("server/discover response was missing result: {discover}"));

    let supported_versions = result
        .get("supportedVersions")
        .and_then(Value::as_array)
        .expect("server/discover result.supportedVersions should be present");
    for version in [rust_mcp::LATEST_MCP_PROTOCOL_VERSION, rust_mcp::LEGACY_MCP_PROTOCOL_VERSION] {
        assert!(
            supported_versions.contains(&json!(version)),
            "server/discover should advertise {version}: {discover}"
        );
    }
    assert!(
        result
            .get("capabilities")
            .and_then(|caps| caps.get("tools"))
            .is_some_and(Value::is_object),
        "server/discover result.capabilities.tools should be present: {discover}"
    );
    assert!(
        result
            .get("instructions")
            .and_then(Value::as_str)
            .is_some_and(|instructions| !instructions.trim().is_empty()),
        "server/discover result.instructions should be present and non-empty: {discover}"
    );
}

#[tokio::test]
async fn rust_mcp_container_serves_stateless_tool_calls_without_session() {
    let rust_mcp = start_ready_rust_mcp().await;

    let response = post_stateless_jsonrpc(
        &reqwest::Client::new(),
        rust_mcp.mcp_url(),
        jsonrpc_request(
            1,
            "tools/call",
            json!({ "name": "ping", "arguments": { "message": "stateless" } }),
        ),
        "stateless tools/call ping request failed",
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response
            .headers()
            .get("mcp-session-id")
            .is_none(),
        "stateless requests must not be assigned a session"
    );

    let payload =
        parse_mcp_response_from_http(response, "failed to read stateless ping response").await;
    let result = payload
        .get("result")
        .unwrap_or_else(|| panic!("stateless ping returned no result: {payload}"));
    assert_eq!(result.get("resultType"), Some(&json!("complete")), "{payload}");
}

#[tokio::test]
async fn rust_mcp_container_rejects_stateless_request_without_standard_headers() {
    let rust_mcp = start_ready_rust_mcp().await;

    let response = reqwest::Client::new()
        .post(rust_mcp.mcp_url())
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", rust_mcp::LATEST_MCP_PROTOCOL_VERSION)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
            "params": {
                "_meta": {
                    "io.modelcontextprotocol/protocolVersion": rust_mcp::LATEST_MCP_PROTOCOL_VERSION,
                    "io.modelcontextprotocol/clientCapabilities": {}
                }
            }
        }))
        .send()
        .await
        .expect("stateless tools/list request failed");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let payload =
        parse_mcp_response_from_http(response, "failed to read header rejection response").await;
    assert_eq!(
        payload
            .get("error")
            .and_then(|error| error.get("code")),
        Some(&json!(-32020)),
        "missing Mcp-Method should be a header-mismatch error: {payload}"
    );
}
