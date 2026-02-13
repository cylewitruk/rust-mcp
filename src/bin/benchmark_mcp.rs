//! Benchmark harness for MCP tool latency/correctness over HTTP transport.

use std::collections::HashMap;
use std::time::Instant;

use clap::Parser;
use reqwest::Client;
use serde_json::{Value, json};

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
    id: u64,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    let payload = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    });

    let response = client
        .post(endpoint)
        .header("content-type", "application/json")
        .header("accept", "application/json")
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;

    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| format!("failed to read response body: {e}"))?;
    if !status.is_success() {
        return Err(format!("http status {status}: {body}"));
    }

    serde_json::from_str::<Value>(&body).map_err(|e| format!("invalid JSON response: {e}"))
}

fn is_success_result(value: &Value) -> bool {
    value.get("error").is_none() && value.get("result").is_some()
}

#[tokio::main]
async fn main() -> Result<(), String> {
    let args = Args::parse();
    let client = Client::new();

    let init = rpc_call(
        &client,
        &args.endpoint,
        1,
        "initialize",
        json!({
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": {"name": "benchmark-mcp", "version": "0.1.0"}
        }),
    )
    .await?;

    if !is_success_result(&init) {
        return Err(format!("initialize failed: {init}"));
    }

    let tools = vec![
        (
            "crate.search",
            json!({
                "name": "crate.search",
                "arguments": {
                    "query": args.crate_name,
                    "limit": 10
                }
            }),
        ),
        (
            "crate.intel",
            json!({
                "name": "crate.intel",
                "arguments": {
                    "crate_name": args.crate_name
                }
            }),
        ),
        (
            "symbol.search",
            json!({
                "name": "symbol.search",
                "arguments": {
                    "query": "Serializer",
                    "crate_name": args.crate_name,
                    "limit": 25
                }
            }),
        ),
        (
            "docs.search",
            json!({
                "name": "docs.search",
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
            let result = rpc_call(&client, &args.endpoint, id, "tools/call", params.clone()).await;
            let elapsed_ms = started
                .elapsed()
                .as_secs_f64()
                * 1000.0;
            id = id.saturating_add(1);

            let ok = match result {
                Ok(value) => is_success_result(&value),
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
