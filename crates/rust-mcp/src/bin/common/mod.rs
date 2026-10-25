//! Stateless MCP JSON-RPC client shared by the benchmark and load-test
//! binaries.

use reqwest::Client;
use rust_mcp_types::protocol::{
    LATEST_MCP_PROTOCOL_VERSION, insert_stateless_request_meta, stateless_request_headers,
};
use serde_json::{Value, json};

const MCP_ACCEPT_HEADER: &str = "application/json, text/event-stream";
const CLIENT_VERSION: &str = "0.1.0";

/// Sends one stateless JSON-RPC request and returns the final JSON-RPC
/// response.
pub async fn rpc_call(
    client: &Client,
    endpoint: &str,
    client_name: &str,
    id: u64,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    let mut payload = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    });
    insert_stateless_request_meta(&mut payload, client_name, CLIENT_VERSION);

    let mut request = client
        .post(endpoint)
        .header("content-type", "application/json")
        .header("accept", MCP_ACCEPT_HEADER);
    for (name, value) in stateless_request_headers(&payload, LATEST_MCP_PROTOCOL_VERSION) {
        request = request.header(name, value);
    }

    let response = request
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;

    let status = response.status();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let body = response
        .text()
        .await
        .map_err(|e| format!("failed to read response body: {e}"))?;
    if !status.is_success() {
        return Err(format!("http status {status}: {body}"));
    }

    if content_type.contains("text/event-stream") {
        parse_last_sse_json_data(&body)
    } else {
        serde_json::from_str::<Value>(&body).map_err(|e| format!("invalid JSON response: {e}"))
    }
}

/// Whether a JSON-RPC response carries a result rather than an error.
pub fn is_success_result(value: &Value) -> bool {
    value.get("error").is_none() && value.get("result").is_some()
}

fn parse_last_sse_json_data(body: &str) -> Result<Value, String> {
    body.lines()
        .rev()
        .filter_map(|line| line.strip_prefix("data:"))
        .find_map(|payload| serde_json::from_str::<Value>(payload.trim()).ok())
        .ok_or_else(|| format!("no JSON data events found in SSE body: {body}"))
}
