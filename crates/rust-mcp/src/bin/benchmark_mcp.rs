//! Benchmark harness for MCP tool latency/correctness over HTTP transport.

use std::collections::HashMap;
use std::time::Instant;

use clap::Parser;
use reqwest::Client;
use serde_json::json;

mod common;

const CLIENT_NAME: &str = "benchmark-mcp";

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

#[tokio::main]
async fn main() -> Result<(), String> {
    let args = Args::parse();
    let client = Client::new();

    let discover =
        common::rpc_call(&client, &args.endpoint, CLIENT_NAME, 1, "server/discover", json!({}))
            .await?;
    if !common::is_success_result(&discover) {
        return Err(format!("server/discover failed: {discover}"));
    }

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
            let result = common::rpc_call(
                &client,
                &args.endpoint,
                CLIENT_NAME,
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

            let ok = result.is_ok_and(|value| common::is_success_result(&value));

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
