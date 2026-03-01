use super::{
    DEFAULT_TEST_PROTOCOL_VERSION, StatusCode, Value, initialize_raw_session, json,
    jsonrpc_request, notify_initialized, parse_mcp_response, parse_mcp_response_from_http,
    post_mcp_jsonrpc, start_ready_rust_mcp,
};

fn assert_jsonrpc_error_envelope(payload: &Value, expected_id: &Value) {
    assert_eq!(
        payload
            .get("jsonrpc")
            .and_then(Value::as_str),
        Some("2.0"),
        "error payload should include jsonrpc=2.0: {payload}"
    );
    assert_eq!(
        payload.get("id"),
        Some(expected_id),
        "error payload should preserve request id: {payload}"
    );

    let error = payload
        .get("error")
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("error payload should include error object: {payload}"));
    assert!(
        error
            .get("code")
            .and_then(Value::as_i64)
            .is_some(),
        "error payload should include numeric error.code: {payload}"
    );
    assert!(
        error
            .get("message")
            .and_then(Value::as_str)
            .is_some_and(|message| !message.trim().is_empty()),
        "error payload should include non-empty error.message: {payload}"
    );
}

#[tokio::test]
async fn rust_mcp_container_rejects_tools_requests_without_session_header_after_initialize() {
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

    let tools_list_without_session = post_mcp_jsonrpc(
        &client,
        rust_mcp.mcp_url(),
        None,
        jsonrpc_request(2, "tools/list", json!({})),
        "raw tools/list request without session header failed",
    )
    .await;
    assert!(
        matches!(
            tools_list_without_session.status(),
            StatusCode::BAD_REQUEST | StatusCode::UNAUTHORIZED | StatusCode::UNPROCESSABLE_ENTITY
        ),
        "expected bad request/unauthorized/unprocessable for tools/list without session header, \
         got {}",
        tools_list_without_session.status()
    );

    let tools_call_without_session = post_mcp_jsonrpc(
        &client,
        rust_mcp.mcp_url(),
        None,
        jsonrpc_request(
            3,
            "tools/call",
            json!({
                "name": "ping",
                "arguments": {"message": "no-session"}
            }),
        ),
        "raw tools/call ping request without session header failed",
    )
    .await;
    assert!(
        matches!(
            tools_call_without_session.status(),
            StatusCode::BAD_REQUEST | StatusCode::UNAUTHORIZED | StatusCode::UNPROCESSABLE_ENTITY
        ),
        "expected bad request/unauthorized/unprocessable for tools/call without session header, \
         got {}",
        tools_call_without_session.status()
    );
}

#[tokio::test]
async fn rust_mcp_container_reports_jsonrpc_error_envelope_for_invalid_request_shapes() {
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

    let invalid_cases: [(u64, Value, &str); 3] = [
        (11, json!({"jsonrpc": "2.0", "id": 11, "params": {}}), "missing method"),
        (
            12,
            json!({"jsonrpc": "1.0", "id": 12, "method": "tools/list", "params": {}}),
            "invalid jsonrpc version",
        ),
        (
            13,
            json!({"jsonrpc": "2.0", "id": 13, "method": 123, "params": {}}),
            "non-string method field",
        ),
    ];

    for (request_id, payload, case_name) in invalid_cases {
        let response = post_mcp_jsonrpc(
            &client,
            rust_mcp.mcp_url(),
            Some(&session_id),
            payload,
            &format!("raw request for invalid-case `{case_name}` failed"),
        )
        .await;

        let status = response.status();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_string();
        let body = response
            .text()
            .await
            .expect("failed to read invalid JSON-RPC shape response body");

        if status.is_success() {
            let parsed = parse_mcp_response(&content_type, &body);
            assert_jsonrpc_error_envelope(&parsed, &json!(request_id));
        } else {
            assert!(
                status.is_client_error(),
                "expected client-error status for invalid JSON-RPC request shape `{case_name}`, \
                 got {status}"
            );
            let lower = body.to_ascii_lowercase();
            assert!(
                lower.contains("error")
                    || lower.contains("invalid")
                    || lower.contains("jsonrpc")
                    || lower.contains("method")
                    || lower.contains("unsupported")
                    || lower.contains("media"),
                "unexpected invalid-shape body for `{case_name}`: {body}"
            );
        }
    }
}

#[tokio::test]
async fn rust_mcp_container_rejects_malformed_json_request_body() {
    let rust_mcp = start_ready_rust_mcp().await;

    let response = reqwest::Client::new()
        .post(rust_mcp.mcp_url())
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .body("{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":")
        .send()
        .await
        .expect("malformed JSON request failed");

    let status = response.status();
    let body = response
        .text()
        .await
        .expect("failed to read malformed JSON response body");

    assert!(
        status.is_client_error(),
        "expected client-error status for malformed JSON request body, got {status}"
    );
    let lower = body.to_ascii_lowercase();
    assert!(
        lower.contains("error")
            || lower.contains("invalid")
            || lower.contains("json")
            || lower.contains("deserialize")
            || lower.contains("parse")
            || lower.contains("eof"),
        "unexpected malformed JSON rejection body: {body}"
    );
}

#[tokio::test]
async fn rust_mcp_container_negotiates_initialize_for_all_recognized_protocol_versions() {
    let rust_mcp = start_ready_rust_mcp().await;
    let client = reqwest::Client::new();

    let requested_versions = ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"];

    for (index, requested_protocol_version) in requested_versions
        .into_iter()
        .enumerate()
    {
        let request_id = 100_u64 + index as u64;
        let (session_id, initialize_payload) = initialize_raw_session(
            &client,
            rust_mcp.mcp_url(),
            request_id,
            requested_protocol_version,
        )
        .await;

        let negotiated_protocol_version = initialize_payload
            .get("result")
            .and_then(|result| result.get("protocolVersion"))
            .and_then(Value::as_str)
            .expect("initialize result.protocolVersion should be present");

        let expected_negotiated =
            if requested_protocol_version <= rust_mcp::SUPPORTED_MCP_PROTOCOL_VERSION {
                requested_protocol_version
            } else {
                rust_mcp::SUPPORTED_MCP_PROTOCOL_VERSION
            };
        assert_eq!(
            negotiated_protocol_version, expected_negotiated,
            "unexpected negotiated protocol version for requested {requested_protocol_version}"
        );

        let initialized_notification =
            notify_initialized(&client, rust_mcp.mcp_url(), &session_id).await;
        assert!(
            initialized_notification
                .status()
                .is_success(),
            "notifications/initialized failed for requested {requested_protocol_version}"
        );

        let tools_list = post_mcp_jsonrpc(
            &client,
            rust_mcp.mcp_url(),
            Some(&session_id),
            jsonrpc_request(200_u64 + index as u64, "tools/list", json!({})),
            &format!(
                "tools/list request failed after protocol negotiation for \
                 {requested_protocol_version}"
            ),
        )
        .await;
        assert!(
            tools_list
                .status()
                .is_success(),
            "tools/list HTTP status not successful for requested {requested_protocol_version}"
        );
        let tools_list_payload = parse_mcp_response_from_http(
            tools_list,
            &format!(
                "failed to read tools/list response body for requested \
                 {requested_protocol_version}"
            ),
        )
        .await;
        assert!(
            tools_list_payload
                .get("result")
                .and_then(|result| result.get("tools"))
                .and_then(Value::as_array)
                .is_some_and(|tools| !tools.is_empty()),
            "tools/list should succeed after negotiated initialize for requested \
             {requested_protocol_version}: {tools_list_payload}"
        );
    }
}
