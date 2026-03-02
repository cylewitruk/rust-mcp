use super::{
    DEFAULT_TEST_PROTOCOL_VERSION, MockRegistryServer, SEEDED_CRATE_NAME, Value,
    initialize_raw_session, json, jsonrpc_request, mock_registry_env, notify_initialized,
    parse_mcp_response_events_from_http, post_mcp_jsonrpc, start_ready_rust_mcp_with_env,
};

#[tokio::test]
async fn rust_mcp_container_emits_progress_notifications_for_progress_tokenized_tools_call() {
    let mock_server = MockRegistryServer::start()
        .await
        .expect("failed to start mock crates/docs server");
    let rust_mcp = start_ready_rust_mcp_with_env(mock_registry_env(&mock_server)).await;

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

    let request_id = 2_u64;
    let progress_token = "e2e-progress-sync";
    let tools_call = post_mcp_jsonrpc(
        &client,
        rust_mcp.mcp_url(),
        Some(&session_id),
        jsonrpc_request(
            request_id,
            "tools/call",
            json!({
                "name": "index_sync_crates",
                "_meta": {
                    "progressToken": progress_token
                },
                "arguments": {
                    "query": SEEDED_CRATE_NAME,
                    "page": 1,
                    "per_page": 10,
                    "include_dependencies": false
                }
            }),
        ),
        "raw tools/call index.sync_crates request failed",
    )
    .await;
    assert!(
        tools_call
            .status()
            .is_success()
    );

    let events = parse_mcp_response_events_from_http(
        tools_call,
        "failed to read tools/call index.sync_crates response body",
    )
    .await;

    let progress_events = events
        .iter()
        .filter(|event| {
            event
                .get("method")
                .and_then(Value::as_str)
                == Some("notifications/progress")
        })
        .filter(|event| {
            event
                .get("params")
                .and_then(|params| params.get("progressToken"))
                .and_then(Value::as_str)
                == Some(progress_token)
        })
        .collect::<Vec<_>>();
    assert!(
        !progress_events.is_empty(),
        "expected at least one notifications/progress event with matching progress token; got \
         events: {events:?}"
    );
    assert!(
        progress_events
            .iter()
            .any(|event| {
                event
                    .get("params")
                    .and_then(|params| params.get("progress"))
                    .and_then(Value::as_f64)
                    .is_some_and(|value| (value - 1.0).abs() < f64::EPSILON)
            }),
        "expected a completion progress event (progress=1.0); got events: {events:?}"
    );

    let final_response = events
        .iter()
        .find(|event| event.get("id") == Some(&json!(request_id)))
        .unwrap_or_else(|| {
            panic!("missing JSON-RPC response event for tools/call id {request_id}")
        });
    assert!(
        final_response
            .get("result")
            .is_some(),
        "tools/call response was missing result payload: {final_response}"
    );
}
