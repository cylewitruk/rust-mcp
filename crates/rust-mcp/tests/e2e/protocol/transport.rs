use super::{
    DEFAULT_TEST_PROTOCOL_VERSION, StatusCode, Value, initialize_params, initialize_raw_session,
    json, jsonrpc_notification, jsonrpc_request, notify_initialized, parse_mcp_response_from_http,
    post_mcp_jsonrpc, start_ready_rust_mcp, start_ready_rust_mcp_with_env,
};

#[tokio::test]
async fn rust_mcp_container_accepts_json_only_accept_for_initialize() {
    let rust_mcp = start_ready_rust_mcp().await;

    let client = reqwest::Client::new();

    let json_only_initialize = client
        .post(rust_mcp.mcp_url())
        .header("content-type", "application/json")
        .header("accept", "application/json")
        .json(&jsonrpc_request(1, "initialize", initialize_params(DEFAULT_TEST_PROTOCOL_VERSION)))
        .send()
        .await
        .expect("raw initialize request with JSON-only accept failed");
    assert!(
        json_only_initialize
            .status()
            .is_success(),
        "expected success for JSON-only accept, got {}",
        json_only_initialize.status()
    );
    let warning = json_only_initialize
        .headers()
        .get("warning")
        .and_then(|value| value.to_str().ok());
    assert!(
        warning.is_some_and(|value| value.contains("Non-conformant Accept header rewritten")),
        "expected Warning header for relaxed Accept rewrite, got: {warning:?}"
    );

    let (session_id, initialize_payload) =
        initialize_raw_session(&client, rust_mcp.mcp_url(), 1, DEFAULT_TEST_PROTOCOL_VERSION).await;
    assert!(
        initialize_payload
            .get("result")
            .is_some()
    );

    let initialized_notification =
        notify_initialized(&client, rust_mcp.mcp_url(), &session_id).await;
    assert!(
        initialized_notification
            .status()
            .is_success()
    );

    let tools_list_response = post_mcp_jsonrpc(
        &client,
        rust_mcp.mcp_url(),
        Some(&session_id),
        jsonrpc_request(2, "tools/list", json!({})),
        "raw tools/list request failed",
    )
    .await;
    assert!(
        tools_list_response
            .status()
            .is_success()
    );
    let tools_list_payload = parse_mcp_response_from_http(
        tools_list_response,
        "failed to read tools/list response body",
    )
    .await;
    assert!(
        tools_list_payload
            .get("result")
            .and_then(|result| result.get("tools"))
            .and_then(Value::as_array)
            .is_some_and(|tools| !tools.is_empty()),
        "tools/list returned unexpected payload: {tools_list_payload}"
    );
}

#[tokio::test]
async fn rust_mcp_container_rejects_json_only_accept_in_strict_mode() {
    let rust_mcp = start_ready_rust_mcp_with_env([("MCP_STRICT_ACCEPT", "true")]).await;

    let response = reqwest::Client::new()
        .post(rust_mcp.mcp_url())
        .header("content-type", "application/json")
        .header("accept", "application/json")
        .json(&jsonrpc_request(1, "initialize", initialize_params(DEFAULT_TEST_PROTOCOL_VERSION)))
        .send()
        .await
        .expect("strict-mode initialize request failed");

    assert_eq!(
        response.status(),
        StatusCode::NOT_ACCEPTABLE,
        "strict-mode JSON-only Accept should be rejected"
    );

    let body = response
        .text()
        .await
        .expect("failed to read strict-mode rejection body");
    assert!(
        body.contains("must accept both application/json and text/event-stream"),
        "unexpected strict-mode rejection body: {body}"
    );
}

#[tokio::test]
async fn rust_mcp_container_rejects_initialized_notification_before_initialize() {
    let rust_mcp = start_ready_rust_mcp().await;

    let client = reqwest::Client::new();
    let notification_response = post_mcp_jsonrpc(
        &client,
        rust_mcp.mcp_url(),
        None,
        jsonrpc_notification("notifications/initialized", json!({})),
        "raw notifications/initialized request failed",
    )
    .await;
    assert!(
        notification_response
            .status()
            .is_client_error(),
        "expected client error when notifying initialized before initialize, got {}",
        notification_response.status()
    );
}

#[tokio::test]
async fn rust_mcp_container_initialized_notification_returns_accepted_without_jsonrpc_payload() {
    let rust_mcp = start_ready_rust_mcp().await;

    let client = reqwest::Client::new();
    let (session_id, _) =
        initialize_raw_session(&client, rust_mcp.mcp_url(), 1, DEFAULT_TEST_PROTOCOL_VERSION).await;
    let initialized_notification =
        notify_initialized(&client, rust_mcp.mcp_url(), &session_id).await;
    assert_eq!(
        initialized_notification.status(),
        StatusCode::ACCEPTED,
        "notifications/initialized should be accepted as a notification (no response payload)"
    );
    let body = initialized_notification
        .text()
        .await
        .expect("failed to read notifications/initialized response body");
    assert!(
        body.trim().is_empty(),
        "notifications/initialized should not return JSON-RPC payload body, got: {body}"
    );
}

#[tokio::test]
async fn rust_mcp_container_allows_tool_calls_after_initialize_before_initialized_notification() {
    let rust_mcp = start_ready_rust_mcp().await;

    let pre_initialize = rust_mcp
        .call_tool("ping", json!({ "message": "pre-init" }))
        .await;
    assert!(
        pre_initialize.is_err(),
        "tools/call ping unexpectedly succeeded before initialize: {pre_initialize:?}"
    );

    rust_mcp.clear_session().await;

    let initialize = rust_mcp
        .initialize_only_mcp()
        .await
        .expect("MCP initialize failed");
    assert!(
        initialize
            .get("result")
            .is_some()
    );

    let before_initialized_notification = rust_mcp
        .call_tool("ping", json!({ "message": "pre-notification" }))
        .await
        .expect("MCP tools/call ping failed after initialize but before notifications/initialized");
    assert!(
        before_initialized_notification
            .get("result")
            .is_some(),
        "tools/call ping should succeed after initialize even before notifications/initialized: \
         {before_initialized_notification:?}"
    );

    rust_mcp.clear_session().await;

    let _ = rust_mcp
        .initialize_mcp()
        .await
        .expect("MCP re-initialize failed");

    let ping = rust_mcp
        .call_tool("ping", json!({ "message": "post-init" }))
        .await
        .expect("MCP tools/call ping failed after notifications/initialized");
    assert!(ping.get("result").is_some());
}

#[tokio::test]
async fn rust_mcp_container_rejects_jsonrpc_batch_requests() {
    let rust_mcp = start_ready_rust_mcp().await;

    let response = reqwest::Client::new()
        .post(rust_mcp.mcp_url())
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .json(&json!([
            {
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": initialize_params(DEFAULT_TEST_PROTOCOL_VERSION)
            },
            {
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/list",
                "params": {}
            }
        ]))
        .send()
        .await
        .expect("raw batch request failed");

    let status = response.status();
    let body = response
        .text()
        .await
        .expect("failed to read batch request response body");
    assert!(
        status.is_client_error(),
        "expected client error for JSON-RPC batch request, got {status}"
    );
    assert!(
        body.to_ascii_lowercase()
            .contains("deserialize")
            || body
                .to_ascii_lowercase()
                .contains("unexpected"),
        "unexpected batch rejection body: {body}"
    );
}
