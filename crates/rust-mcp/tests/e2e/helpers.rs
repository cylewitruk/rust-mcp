//! Shared helpers for e2e MCP HTTP assertions.

use std::time::Duration;

use rust_mcp_testing::rust_mcp::RustMcpTestContainer;
use serde_json::{Value, json};

const MCP_CONTENT_TYPE_JSON: &str = "application/json";
const MCP_ACCEPT_STREAMABLE: &str = "application/json, text/event-stream";
const E2E_CLIENT_NAME: &str = "rust-mcp-e2e";
const E2E_CLIENT_VERSION: &str = "0.1.0";

pub const DEFAULT_TEST_PROTOCOL_VERSION: &str = rust_mcp::LATEST_MCP_PROTOCOL_VERSION;
const DEFAULT_CONTAINER_READY_TIMEOUT: Duration = Duration::from_secs(120);

async fn wait_until_container_ready(rust_mcp: &RustMcpTestContainer) {
    rust_mcp
        .wait_until_ready(DEFAULT_CONTAINER_READY_TIMEOUT)
        .await
        .expect("container did not become ready");
}

pub async fn start_ready_rust_mcp() -> RustMcpTestContainer {
    let rust_mcp = RustMcpTestContainer::start()
        .await
        .expect("failed to start rust-mcp container");
    wait_until_container_ready(&rust_mcp).await;
    rust_mcp
}

pub async fn start_ready_rust_mcp_with_env<K, V, I>(env_vars: I) -> RustMcpTestContainer
where
    K: Into<String>,
    V: Into<String>,
    I: IntoIterator<Item = (K, V)>,
{
    let rust_mcp = RustMcpTestContainer::start_with_env(env_vars)
        .await
        .expect("failed to start rust-mcp container");
    wait_until_container_ready(&rust_mcp).await;
    rust_mcp
}

pub async fn start_ready_rust_mcp_with_env_and_mounts<K, V, I, P, Q, M>(
    env_vars: I,
    mounts: M,
) -> RustMcpTestContainer
where
    K: Into<String>,
    V: Into<String>,
    I: IntoIterator<Item = (K, V)>,
    P: Into<String>,
    Q: Into<String>,
    M: IntoIterator<Item = (P, Q)>,
{
    let rust_mcp = RustMcpTestContainer::start_with_env_and_mounts(env_vars, mounts)
        .await
        .expect("failed to start rust-mcp container");
    wait_until_container_ready(&rust_mcp).await;
    rust_mcp
}

pub async fn initialize_mcp_session(rust_mcp: &RustMcpTestContainer) -> Value {
    let initialize = rust_mcp
        .initialize_mcp()
        .await
        .expect("MCP initialize failed");
    assert!(
        initialize
            .get("result")
            .is_some(),
        "MCP initialize returned no result payload: {initialize}"
    );
    initialize
}

pub fn tool_result<'a>(response: &'a Value, tool_name: &str) -> &'a Value {
    let result = response
        .get("result")
        .unwrap_or_else(|| panic!("MCP tools/call {tool_name} returned no result payload"));
    let is_error = result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    assert!(!is_error, "MCP tools/call {tool_name} returned isError=true: {result}");
    result
}

pub fn structured_content<'a>(result: &'a Value, tool_name: &str) -> &'a Value {
    result
        .get("structuredContent")
        .or_else(|| result.get("structured_content"))
        .unwrap_or_else(|| panic!("MCP tools/call {tool_name} returned no structured content"))
}

pub fn first_content_text<'a>(response: &'a Value, tool_name: &str) -> &'a str {
    response
        .get("result")
        .and_then(|result| result.get("content"))
        .and_then(Value::as_array)
        .and_then(|content| content.first())
        .and_then(|first| first.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("MCP tools/call {tool_name} returned no text content"))
}

pub fn parse_last_sse_json_data(body: &str) -> Option<Value> {
    parse_sse_json_data_events(body)
        .into_iter()
        .last()
}

pub fn parse_sse_json_data_events(body: &str) -> Vec<Value> {
    body.lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim)
        .filter(|payload| !payload.is_empty())
        .filter_map(|payload| serde_json::from_str::<Value>(payload).ok())
        .collect()
}

pub fn parse_mcp_response_events(content_type: &str, body: &str) -> Vec<Value> {
    if content_type.contains("text/event-stream") {
        let events = parse_sse_json_data_events(body);
        assert!(
            !events.is_empty(),
            "expected at least one JSON data event in SSE response body: {body}"
        );
        events
    } else {
        vec![
            serde_json::from_str::<Value>(body).unwrap_or_else(|error| {
                panic!("failed to parse MCP response JSON ({error}): {body}")
            }),
        ]
    }
}

pub fn parse_mcp_response(content_type: &str, body: &str) -> Value {
    if content_type.contains("text/event-stream") {
        parse_last_sse_json_data(body)
            .unwrap_or_else(|| panic!("expected JSON data event in SSE response body: {body}"))
    } else {
        serde_json::from_str::<Value>(body)
            .unwrap_or_else(|error| panic!("failed to parse MCP response JSON ({error}): {body}"))
    }
}

pub fn initialize_params(protocol_version: &str) -> Value {
    json!({
        "protocolVersion": protocol_version,
        "capabilities": {},
        "clientInfo": {"name": E2E_CLIENT_NAME, "version": E2E_CLIENT_VERSION},
    })
}

pub fn jsonrpc_request(request_id: u64, method: &str, params: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": request_id,
        "method": method,
        "params": params,
    })
}

pub fn jsonrpc_notification(method: &str, params: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
    })
}

pub async fn post_mcp_jsonrpc(
    client: &reqwest::Client,
    mcp_url: &str,
    session_id: Option<&str>,
    payload: Value,
    error_context: &str,
) -> reqwest::Response {
    let mut request = client
        .post(mcp_url)
        .header("content-type", MCP_CONTENT_TYPE_JSON)
        .header("accept", MCP_ACCEPT_STREAMABLE);

    if let Some(session_id) = session_id {
        request = request.header("mcp-session-id", session_id);
    }

    request
        .json(&payload)
        .send()
        .await
        .unwrap_or_else(|error| panic!("{error_context}: {error}"))
}

pub fn response_session_id(response: &reqwest::Response) -> String {
    response
        .headers()
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
        .expect("initialize response did not include mcp-session-id header")
        .to_string()
}

pub async fn parse_mcp_response_from_http(
    response: reqwest::Response,
    error_context: &str,
) -> Value {
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let body = response
        .text()
        .await
        .unwrap_or_else(|error| panic!("{error_context}: {error}"));
    parse_mcp_response(&content_type, &body)
}

pub async fn parse_mcp_response_events_from_http(
    response: reqwest::Response,
    error_context: &str,
) -> Vec<Value> {
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let body = response
        .text()
        .await
        .unwrap_or_else(|error| panic!("{error_context}: {error}"));
    parse_mcp_response_events(&content_type, &body)
}

pub async fn initialize_raw_session(
    client: &reqwest::Client,
    mcp_url: &str,
    request_id: u64,
    protocol_version: &str,
) -> (String, Value) {
    let initialize_response = post_mcp_jsonrpc(
        client,
        mcp_url,
        None,
        jsonrpc_request(request_id, "initialize", initialize_params(protocol_version)),
        "raw initialize request failed",
    )
    .await;
    assert!(
        initialize_response
            .status()
            .is_success()
    );
    let session_id = response_session_id(&initialize_response);
    let initialize_payload = parse_mcp_response_from_http(
        initialize_response,
        "failed to read initialize response body",
    )
    .await;
    (session_id, initialize_payload)
}

pub async fn notify_initialized(
    client: &reqwest::Client,
    mcp_url: &str,
    session_id: &str,
) -> reqwest::Response {
    post_mcp_jsonrpc(
        client,
        mcp_url,
        Some(session_id),
        jsonrpc_notification("notifications/initialized", json!({})),
        "raw notifications/initialized failed",
    )
    .await
}
