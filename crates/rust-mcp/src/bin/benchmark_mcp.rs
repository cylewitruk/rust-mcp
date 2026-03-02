//! Benchmark harness for MCP tool latency/correctness over HTTP transport.

use std::collections::HashMap;
use std::time::Instant;

use clap::Parser;
use reqwest::Client;
use serde_json::{Value, json};

const MCP_SESSION_ID_HEADER: &str = "mcp-session-id";
const MCP_ACCEPT_HEADER: &str = "application/json, text/event-stream";

#[derive(Debug, Parser)]
#[command(name = "benchmark-mcp", about = "Benchmark rust-mcp tools over HTTP MCP")]
struct Args {
    #[arg(long, default_value = "http://127.0.0.1:43173/mcp")]
    endpoint: String,

    #[arg(long, default_value = "serde")]
    crate_name: String,

    #[arg(long, default_value_t = 10)]
    iterations: u32,
}

#[derive(Debug, Default)]
struct ToolStats {
    count: u32,
    failures: u32,
    latencies_ms: Vec<f64>,
}

impl ToolStats {
    fn record(&mut self, latency_ms: f64, ok: bool) {
        self.count += 1;
        if !ok {
            self.failures += 1;
        }
        self.latencies_ms
            .push(latency_ms);
    }

    fn average_ms(&self) -> f64 {
        if self.latencies_ms.is_empty() {
            return 0.0;
        }
        self.latencies_ms
            .iter()
            .sum::<f64>()
            / (self.latencies_ms.len() as f64)
    }

    fn p95_ms(&self) -> f64 {
        if self.latencies_ms.is_empty() {
            return 0.0;
        }
        let mut values = self.latencies_ms.clone();
        values.sort_by(|a, b| {
            a.partial_cmp(b)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let idx = ((values.len() as f64) * 0.95).ceil() as usize;
        values[idx
            .saturating_sub(1)
            .min(values.len() - 1)]
    }
}

async fn rpc_call(
    client: &Client,
    endpoint: &str,
    session_id: Option<&str>,
    id: u64,
    method: &str,
    params: Value,
) -> Result<(Value, Option<String>), String> {
    let payload = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    });

    let mut request = client
        .post(endpoint)
        .header("content-type", "application/json")
        .header("accept", MCP_ACCEPT_HEADER)
        .json(&payload);
    if let Some(sid) = session_id {
        request = request.header(MCP_SESSION_ID_HEADER, sid);
    }

    let response = request
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
    let response_session_id = response
        .headers()
        .get(MCP_SESSION_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(ToString::to_string);
    let body = response
        .text()
        .await
        .map_err(|e| format!("failed to read response body: {e}"))?;
    if !status.is_success() {
        return Err(format!("http status {status}: {body}"));
    }

    let parsed = if content_type.contains("text/event-stream") {
        parse_last_sse_json_data(&body)?
    } else {
        serde_json::from_str::<Value>(&body).map_err(|e| format!("invalid JSON response: {e}"))?
    };

    Ok((parsed, response_session_id))
}

async fn rpc_notify(
    client: &Client,
    endpoint: &str,
    session_id: &str,
    method: &str,
    params: Value,
) -> Result<Option<String>, String> {
    let payload = json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
    });

    let response = client
        .post(endpoint)
        .header("content-type", "application/json")
        .header("accept", MCP_ACCEPT_HEADER)
        .header(MCP_SESSION_ID_HEADER, session_id)
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("notification request failed: {e}"))?;

    let status = response.status();
    let response_session_id = response
        .headers()
        .get(MCP_SESSION_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(ToString::to_string);
    let body = response
        .text()
        .await
        .map_err(|e| format!("failed to read notification response body: {e}"))?;
    if !status.is_success() {
        return Err(format!("notification `{method}` failed with http status {status}: {body}"));
    }

    Ok(response_session_id)
}

fn parse_last_sse_json_data(body: &str) -> Result<Value, String> {
    let mut last_json: Option<Value> = None;

    for line in body.lines() {
        let Some(payload) = line.strip_prefix("data:") else {
            continue;
        };
        let payload = payload.trim();
        if payload.is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(payload) {
            last_json = Some(value);
        }
    }

    last_json.ok_or_else(|| format!("no JSON data events found in SSE body: {body}"))
}

fn is_success_result(value: &Value) -> bool {
    value.get("error").is_none() && value.get("result").is_some()
}

#[tokio::main]
async fn main() -> Result<(), String> {
    let args = Args::parse();
    let client = Client::new();
    let mut session_id: Option<String> = None;

    let (init, init_session_id) = rpc_call(
        &client,
        &args.endpoint,
        None,
        1,
        "initialize",
        json!({
            "protocolVersion": rust_mcp::LATEST_MCP_PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "benchmark-mcp", "version": "0.1.0"}
        }),
    )
    .await?;
    session_id = init_session_id.or(session_id);

    if !is_success_result(&init) {
        return Err(format!("initialize failed: {init}"));
    }
    let sid = session_id
        .as_deref()
        .ok_or_else(|| "initialize response missing mcp-session-id header".to_string())?;
    session_id = rpc_notify(&client, &args.endpoint, sid, "notifications/initialized", json!({}))
        .await?
        .or(session_id);

    let tools = vec![
        (
            "crate_search",
            json!({
                "name": "crate_search",
                "arguments": {
                    "query": args.crate_name,
                    "limit": 10
                }
            }),
        ),
        (
            "crate_intel",
            json!({
                "name": "crate_intel",
                "arguments": {
                    "crate_name": args.crate_name
                }
            }),
        ),
        (
            "symbol_search",
            json!({
                "name": "symbol_search",
                "arguments": {
                    "query": "Serializer",
                    "crate_name": args.crate_name,
                    "limit": 25
                }
            }),
        ),
        (
            "docs_search",
            json!({
                "name": "docs_search",
                "arguments": {
                    "query": "serialize",
                    "crate_name": args.crate_name,
                    "limit": 10
                }
            }),
        ),
    ];

    let mut stats = HashMap::<String, ToolStats>::new();
    let mut id = 2_u64;

    for _ in 0..args.iterations {
        for (tool_name, params) in &tools {
            let started = Instant::now();
            let result = rpc_call(
                &client,
                &args.endpoint,
                session_id.as_deref(),
                id,
                "tools/call",
                params.clone(),
            )
            .await;
            let elapsed_ms = started
                .elapsed()
                .as_secs_f64()
                * 1000.0;
            id = id.saturating_add(1);

            let ok = match result {
                Ok((value, response_session_id)) => {
                    if let Some(sid) = response_session_id {
                        session_id = Some(sid);
                    }
                    is_success_result(&value)
                }
                Err(_) => false,
            };

            stats
                .entry((*tool_name).to_string())
                .or_default()
                .record(elapsed_ms, ok);
        }
    }

    println!("Benchmark results for {} (iterations={})", args.endpoint, args.iterations);
    println!("tool\tcount\tfailures\tavg_ms\tp95_ms");

    let mut tool_names = stats
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    tool_names.sort();
    for tool_name in tool_names {
        let metric = &stats[&tool_name];
        println!(
            "{}\t{}\t{}\t{:.2}\t{:.2}",
            tool_name,
            metric.count,
            metric.failures,
            metric.average_ms(),
            metric.p95_ms()
        );
    }

    Ok(())
}
