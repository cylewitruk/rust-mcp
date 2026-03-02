use super::{
    DEFAULT_TEST_PROTOCOL_VERSION, Value, initialize_raw_session, json, jsonrpc_request,
    notify_initialized, parse_mcp_response, post_mcp_jsonrpc, start_ready_rust_mcp,
};

#[tokio::test]
async fn rust_mcp_container_reports_invalid_params_for_tools_call() {
    let rust_mcp = start_ready_rust_mcp().await;

    let client = reqwest::Client::new();
    let (session_id, _) =
        initialize_raw_session(&client, rust_mcp.mcp_url(), 1, DEFAULT_TEST_PROTOCOL_VERSION).await;
    let initialized_notification =
        notify_initialized(&client, rust_mcp.mcp_url(), &session_id).await;
    assert!(
        initialized_notification
            .status()
            .is_success()
    );

    let invalid_params_response = post_mcp_jsonrpc(
        &client,
        rust_mcp.mcp_url(),
        Some(&session_id),
        jsonrpc_request(
            2,
            "tools/call",
            json!({
                "arguments": {}
            }),
        ),
        "raw tools/call invalid-params request failed",
    )
    .await;

    let status = invalid_params_response.status();
    let content_type = invalid_params_response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let body = invalid_params_response
        .text()
        .await
        .expect("failed to read invalid-params response body");

    if status.is_success() {
        let payload = parse_mcp_response(&content_type, &body);
        assert!(
            payload.get("error").is_some(),
            "expected JSON-RPC error payload for invalid params, got: {payload}"
        );
    } else {
        assert!(
            status.is_client_error(),
            "expected invalid params to produce client error or JSON-RPC error, got status \
             {status}"
        );
        assert!(
            body.to_ascii_lowercase()
                .contains("error")
                || body
                    .to_ascii_lowercase()
                    .contains("invalid")
                || body
                    .to_ascii_lowercase()
                    .contains("name"),
            "unexpected invalid params error body: {body}"
        );
    }
}

#[tokio::test]
async fn rust_mcp_container_reports_tool_validation_failure_as_error_result() {
    let rust_mcp = start_ready_rust_mcp().await;
    let _ = rust_mcp
        .initialize_mcp()
        .await
        .expect("MCP initialize failed");

    let invalid_tool_response = rust_mcp
        .call_tool(
            "dependency_feature_impact",
            json!({
                "crate_name": "serde",
                "features": []
            }),
        )
        .await
        .expect("tool validation failure should still return MCP tool result envelope");

    let result = invalid_tool_response
        .get("result")
        .expect("MCP tools/call dependency.feature_impact returned no result payload");
    let is_error = result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    assert!(
        is_error,
        "expected tool-level isError=true for invalid dependency.feature_impact request: {result}"
    );
}

#[tokio::test]
async fn rust_mcp_container_reports_error_for_unknown_tool_call() {
    let rust_mcp = start_ready_rust_mcp().await;
    let _ = rust_mcp
        .initialize_mcp()
        .await
        .expect("MCP initialize failed");

    let unknown_tool = rust_mcp
        .call_tool("tool.does_not_exist", json!({}))
        .await;
    assert!(unknown_tool.is_err(), "unknown tool unexpectedly succeeded: {unknown_tool:?}");
    let error = unknown_tool
        .expect_err("unknown tool should have failed")
        .to_string();
    assert!(
        error.contains("tools/call") && error.contains("error payload"),
        "unexpected unknown tool error payload: {error}"
    );
}

#[tokio::test]
async fn rust_mcp_container_reports_error_for_unknown_jsonrpc_method() {
    let rust_mcp = start_ready_rust_mcp().await;

    let client = reqwest::Client::new();
    let (session_id, _) =
        initialize_raw_session(&client, rust_mcp.mcp_url(), 1, DEFAULT_TEST_PROTOCOL_VERSION).await;
    let initialized_notification =
        notify_initialized(&client, rust_mcp.mcp_url(), &session_id).await;
    assert!(
        initialized_notification
            .status()
            .is_success()
    );

    let unknown_method_response = post_mcp_jsonrpc(
        &client,
        rust_mcp.mcp_url(),
        Some(&session_id),
        jsonrpc_request(2, "foo/bar_baz", json!({})),
        "raw unknown-method request failed",
    )
    .await;
    let status = unknown_method_response.status();
    let content_type = unknown_method_response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let body = unknown_method_response
        .text()
        .await
        .expect("failed to read unknown-method response body");

    if status.is_success() {
        let payload = parse_mcp_response(&content_type, &body);
        assert!(
            payload.get("error").is_some(),
            "expected JSON-RPC error payload for unknown method, got: {payload}"
        );
    } else {
        assert!(
            status.is_client_error(),
            "expected unknown method to produce client error or JSON-RPC error, got status \
             {status}"
        );
        assert!(
            body.to_ascii_lowercase()
                .contains("error")
                || body
                    .to_ascii_lowercase()
                    .contains("method"),
            "unexpected unknown method error body: {body}"
        );
    }
}
