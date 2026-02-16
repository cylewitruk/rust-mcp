use serde_json::json;

use super::{call_tool_response, initialized_container};

#[tokio::test]
async fn rust_mcp_container_exports_tool_metrics_after_calls() {
    let rust_mcp = initialized_container().await;

    let _ = call_tool_response(&rust_mcp, "ping", json!({ "message": "metrics" })).await;
    let _ = call_tool_response(&rust_mcp, "index.status", json!({})).await;

    let metrics = reqwest::Client::new()
        .get(format!("{}/metrics", rust_mcp.metrics_url()))
        .send()
        .await
        .expect("metrics endpoint request failed");
    assert!(metrics.status().is_success());

    let body = metrics
        .text()
        .await
        .expect("failed to read metrics body");
    assert!(body.contains("rust_mcp_tool_invocations_total"));
    assert!(body.contains("tool=\"index.status\""));
    assert!(body.contains("rust_mcp_tool_latency_ms"));
}
