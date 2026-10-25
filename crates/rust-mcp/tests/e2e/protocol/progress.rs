use super::{
    LEGACY_TEST_PROTOCOL_VERSION, MockRegistryServer, SEEDED_CRATE_NAME, Value,
    initialize_raw_session, json, jsonrpc_request, mock_registry_env, notify_initialized,
    parse_mcp_response_events_from_http, post_mcp_jsonrpc, post_stateless_jsonrpc,
    start_ready_rust_mcp_with_env,
};

const REQUEST_ID: u64 = 2;

fn index_crates_request(progress_token: &str) -> Value {
    jsonrpc_request(
        REQUEST_ID,
        "tools/call",
        json!({
            "name": "index_crates",
            "_meta": {
                "progressToken": progress_token
            },
            "arguments": {
                "crates": [{ "name": SEEDED_CRATE_NAME }],
                "include_dependencies": false
            }
        }),
    )
}

/// Asserts the response stream carries progress notifications for
/// `progress_token`, including completion, followed by the final result.
fn assert_progress_events(events: &[Value], progress_token: &str) {
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
        .find(|event| event.get("id") == Some(&json!(REQUEST_ID)))
        .unwrap_or_else(|| {
            panic!("missing JSON-RPC response event for tools/call id {REQUEST_ID}")
        });
    assert!(
        final_response
            .get("result")
            .is_some(),
        "tools/call response was missing result payload: {final_response}"
    );
}

#[tokio::test]
async fn rust_mcp_container_emits_progress_notifications_for_stateless_tools_call() {
    let mock_server = MockRegistryServer::start()
        .await
        .expect("failed to start mock crates/docs server");
    let rust_mcp = start_ready_rust_mcp_with_env(mock_registry_env(&mock_server)).await;

    let progress_token = "e2e-progress-stateless";
    let tools_call = post_stateless_jsonrpc(
        &reqwest::Client::new(),
        rust_mcp.mcp_url(),
        index_crates_request(progress_token),
        "stateless tools/call index_crates request failed",
    )
    .await;
    assert!(
        tools_call
            .status()
            .is_success()
    );

    let events = parse_mcp_response_events_from_http(
        tools_call,
        "failed to read stateless tools/call index_crates response body",
    )
    .await;
    assert_progress_events(&events, progress_token);
}

#[tokio::test]
async fn rust_mcp_container_emits_progress_notifications_for_legacy_tools_call() {
    let mock_server = MockRegistryServer::start()
        .await
        .expect("failed to start mock crates/docs server");
    let rust_mcp = start_ready_rust_mcp_with_env(mock_registry_env(&mock_server)).await;

    let client = reqwest::Client::new();
    let (session_id, _) =
        initialize_raw_session(&client, rust_mcp.mcp_url(), 1, LEGACY_TEST_PROTOCOL_VERSION).await;
    let initialized_notification =
        notify_initialized(&client, rust_mcp.mcp_url(), &session_id).await;
    assert!(
        initialized_notification
            .status()
            .is_success()
    );

    let progress_token = "e2e-progress-legacy";
    let tools_call = post_mcp_jsonrpc(
        &client,
        rust_mcp.mcp_url(),
        Some(&session_id),
        index_crates_request(progress_token),
        "legacy tools/call index_crates request failed",
    )
    .await;
    assert!(
        tools_call
            .status()
            .is_success()
    );

    let events = parse_mcp_response_events_from_http(
        tools_call,
        "failed to read legacy tools/call index_crates response body",
    )
    .await;
    assert_progress_events(&events, progress_token);
}
