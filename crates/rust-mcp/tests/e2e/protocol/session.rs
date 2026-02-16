use super::{
    DEFAULT_TEST_PROTOCOL_VERSION, StatusCode, Value, initialize_raw_session, json,
    jsonrpc_request, notify_initialized, parse_mcp_response_from_http, post_mcp_jsonrpc,
    start_ready_rust_mcp,
};

#[tokio::test]
async fn rust_mcp_container_preserves_jsonrpc_ids_across_initialize_and_tools_requests() {
    let rust_mcp = start_ready_rust_mcp().await;

    let initialize_id = 101_u64;
    let tools_list_id = 102_u64;
    let ping_id = 103_u64;
    let client = reqwest::Client::new();

    let (session_id, initialize_payload) = initialize_raw_session(
        &client,
        rust_mcp.mcp_url(),
        initialize_id,
        DEFAULT_TEST_PROTOCOL_VERSION,
    )
    .await;
    assert_eq!(initialize_payload.get("id"), Some(&json!(initialize_id)));
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
        jsonrpc_request(tools_list_id, "tools/list", json!({})),
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
    assert_eq!(tools_list_payload.get("id"), Some(&json!(tools_list_id)));
    assert!(
        tools_list_payload
            .get("result")
            .and_then(|result| result.get("tools"))
            .and_then(Value::as_array)
            .is_some_and(|tools| !tools.is_empty())
    );

    let ping_response = post_mcp_jsonrpc(
        &client,
        rust_mcp.mcp_url(),
        Some(&session_id),
        jsonrpc_request(
            ping_id,
            "tools/call",
            json!({
                "name": "ping",
                "arguments": {"message": "id-correlation"}
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
    assert_eq!(ping_payload.get("id"), Some(&json!(ping_id)));
    assert!(
        ping_payload
            .get("result")
            .is_some()
    );
}

#[tokio::test]
async fn rust_mcp_container_supports_concurrent_requests_on_same_session() {
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

    let tools_list_id = 21_u64;
    let ping_id = 22_u64;
    let tools_list_request = post_mcp_jsonrpc(
        &client,
        rust_mcp.mcp_url(),
        Some(&session_id),
        jsonrpc_request(tools_list_id, "tools/list", json!({})),
        "concurrent tools/list request failed",
    );
    let ping_request = post_mcp_jsonrpc(
        &client,
        rust_mcp.mcp_url(),
        Some(&session_id),
        jsonrpc_request(
            ping_id,
            "tools/call",
            json!({
                "name": "ping",
                "arguments": {"message": "concurrent"}
            }),
        ),
        "concurrent ping request failed",
    );
    let (tools_list_response, ping_response) = tokio::join!(tools_list_request, ping_request);

    assert!(
        tools_list_response
            .status()
            .is_success()
    );
    let tools_list_payload = parse_mcp_response_from_http(
        tools_list_response,
        "failed to read concurrent tools/list response body",
    )
    .await;
    assert_eq!(tools_list_payload.get("id"), Some(&json!(tools_list_id)));
    assert!(
        tools_list_payload
            .get("result")
            .and_then(|result| result.get("tools"))
            .and_then(Value::as_array)
            .is_some_and(|tools| !tools.is_empty())
    );

    assert!(
        ping_response
            .status()
            .is_success()
    );
    let ping_payload =
        parse_mcp_response_from_http(ping_response, "failed to read concurrent ping response body")
            .await;
    assert_eq!(ping_payload.get("id"), Some(&json!(ping_id)));
    assert!(
        ping_payload
            .get("result")
            .is_some()
    );
}

#[tokio::test]
async fn rust_mcp_container_closes_session_via_delete_and_rejects_follow_up_requests() {
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

    let ping_before_close = post_mcp_jsonrpc(
        &client,
        rust_mcp.mcp_url(),
        Some(&session_id),
        jsonrpc_request(
            2,
            "tools/call",
            json!({
                "name": "ping",
                "arguments": {"message": "before-close"}
            }),
        ),
        "raw tools/call ping request failed before close",
    )
    .await;
    assert!(
        ping_before_close
            .status()
            .is_success()
    );

    let close_session_response = client
        .delete(rust_mcp.mcp_url())
        .header("accept", "application/json, text/event-stream")
        .header("mcp-session-id", &session_id)
        .send()
        .await
        .expect("DELETE /mcp close-session request failed");
    assert_eq!(
        close_session_response.status(),
        StatusCode::ACCEPTED,
        "expected DELETE /mcp to acknowledge session termination"
    );

    let post_close_tools_list = post_mcp_jsonrpc(
        &client,
        rust_mcp.mcp_url(),
        Some(&session_id),
        jsonrpc_request(3, "tools/list", json!({})),
        "raw tools/list request failed after session close",
    )
    .await;
    assert_eq!(
        post_close_tools_list.status(),
        StatusCode::UNAUTHORIZED,
        "expected closed session id to be rejected by subsequent requests"
    );
}

#[tokio::test]
async fn rust_mcp_container_rejects_unknown_session_id_header() {
    let rust_mcp = start_ready_rust_mcp().await;

    let client = reqwest::Client::new();
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

    let unknown_session_id = format!("{session_id}-unknown");
    let unknown_session_response = post_mcp_jsonrpc(
        &client,
        rust_mcp.mcp_url(),
        Some(&unknown_session_id),
        jsonrpc_request(
            2,
            "tools/call",
            json!({
                "name": "ping",
                "arguments": {"message": "wrong-session"}
            }),
        ),
        "raw tools/call request with unknown session failed",
    )
    .await;
    assert_eq!(
        unknown_session_response.status(),
        StatusCode::UNAUTHORIZED,
        "expected unauthorized status for unknown session id"
    );

    let ping_response = post_mcp_jsonrpc(
        &client,
        rust_mcp.mcp_url(),
        Some(&session_id),
        jsonrpc_request(
            3,
            "tools/call",
            json!({
                "name": "ping",
                "arguments": {"message": "good-session"}
            }),
        ),
        "raw tools/call request with valid session failed",
    )
    .await;
    assert!(
        ping_response
            .status()
            .is_success()
    );
    let ping_payload =
        parse_mcp_response_from_http(ping_response, "failed to read tools/call response body")
            .await;
    assert!(
        ping_payload
            .get("result")
            .is_some()
    );
}
