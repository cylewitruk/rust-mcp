//! Concurrent load/backpressure harness for MCP tool calls over HTTP.

use std::sync::Arc;
use std::time::Instant;

use clap::Parser;
use reqwest::Client;
use serde_json::{Value, json};
use tokio::sync::Mutex;

#[derive(Debug, Parser)]
#[command(name = "load-test-mcp", about = "Run concurrent MCP tool load tests")]
struct Args {
    #[arg(long, default_value = "http://127.0.0.1:43173/mcp")]
    endpoint: String,

    #[arg(long, default_value = "serde")]
    crate_name: String,

    #[arg(long, default_value_t = 8)]
    concurrency: u32,

    #[arg(long, default_value_t = 20)]
    requests_per_worker: u32,
}

#[derive(Debug, Default)]
struct SharedMetrics {
    total_requests: u64,
    failures: u64,
    latencies_ms: Vec<f64>,
}

async fn rpc_call(
    client: &Client,
    endpoint: &str,
    id: u64,
    params: Value,
) -> Result<Value, String> {
    let payload = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
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

fn p95(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| {
        a.partial_cmp(b)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let idx = ((sorted.len() as f64) * 0.95).ceil() as usize;
    sorted[idx
        .saturating_sub(1)
        .min(sorted.len() - 1)]
}

#[tokio::main]
async fn main() -> Result<(), String> {
    let args = Args::parse();
    let client = Client::new();

    let init_payload = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": {"name": "load-test-mcp", "version": "0.1.0"}
        }
    });
    let init_response = client
        .post(&args.endpoint)
        .header("content-type", "application/json")
        .header("accept", "application/json")
        .json(&init_payload)
        .send()
        .await
        .map_err(|e| format!("initialize request failed: {e}"))?;
    if !init_response
        .status()
        .is_success()
    {
        return Err(format!("initialize failed with status {}", init_response.status()));
    }

    let metrics = Arc::new(Mutex::new(SharedMetrics::default()));

    let mut tasks = Vec::new();
    for worker in 0..args.concurrency {
        let client = client.clone();
        let endpoint = args.endpoint.clone();
        let crate_name = args.crate_name.clone();
        let metrics = Arc::clone(&metrics);

        tasks.push(tokio::spawn(async move {
            for i in 0..args.requests_per_worker {
                let request_id = 10_000_u64 + (worker as u64 * 1_000) + i as u64;
                let params = match i % 3 {
                    0 => json!({
                        "name": "crate.search",
                        "arguments": {"query": crate_name, "limit": 10}
                    }),
                    1 => json!({
                        "name": "symbol.search",
                        "arguments": {"query": "Serializer", "crate_name": crate_name, "limit": 25}
                    }),
                    _ => json!({
                        "name": "docs.search",
                        "arguments": {"query": "serialize", "crate_name": crate_name, "limit": 10}
                    }),
                };

                let started = Instant::now();
                let result = rpc_call(&client, &endpoint, request_id, params).await;
                let latency_ms = started
                    .elapsed()
                    .as_secs_f64()
                    * 1000.0;

                let mut lock = metrics.lock().await;
                lock.total_requests += 1;
                lock.latencies_ms
                    .push(latency_ms);
                match result {
                    Ok(body) => {
                        if !is_success_result(&body) {
                            lock.failures += 1;
                        }
                    }
                    Err(_) => {
                        lock.failures += 1;
                    }
                }
            }
        }));
    }

    for task in tasks {
        task.await
            .map_err(|e| format!("worker join failure: {e}"))?;
    }

    let lock = metrics.lock().await;
    let average = if lock.latencies_ms.is_empty() {
        0.0
    } else {
        lock.latencies_ms
            .iter()
            .sum::<f64>()
            / lock.latencies_ms.len() as f64
    };
    let p95_latency = p95(&lock.latencies_ms);
    let error_rate = if lock.total_requests == 0 {
        0.0
    } else {
        (lock.failures as f64) / (lock.total_requests as f64)
    };

    println!("Load test completed");
    println!("endpoint: {}", args.endpoint);
    println!("concurrency: {}", args.concurrency);
    println!("requests_per_worker: {}", args.requests_per_worker);
    println!("total_requests: {}", lock.total_requests);
    println!("failures: {}", lock.failures);
    println!("error_rate: {:.4}", error_rate);
    println!("average_latency_ms: {:.2}", average);
    println!("p95_latency_ms: {:.2}", p95_latency);

    Ok(())
}
