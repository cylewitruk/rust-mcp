//! Docker image E2E smoke tests for rust-mcp HTTP endpoints.

use std::time::Duration;

use rust_mcp_testing::rust_mcp::RustMcpTestContainer;
use serde_json::json;

#[tokio::test]
async fn rust_mcp_container_serves_health_and_ready_endpoints() {
    let rust_mcp = RustMcpTestContainer::start()
        .await
        .expect("failed to start rust-mcp container");
    rust_mcp
        .wait_until_ready(Duration::from_secs(120))
        .await
        .expect("container did not become ready");

    let client = reqwest::Client::new();

    let healthz = client
        .get(format!("{}/healthz", rust_mcp.base_url()))
        .send()
        .await
        .expect("healthz request failed");
    assert!(healthz.status().is_success());

    let readyz = client
        .get(format!("{}/readyz", rust_mcp.base_url()))
        .send()
        .await
        .expect("readyz request failed");
    assert!(readyz.status().is_success());

    let mcp_response = client
        .post(rust_mcp.mcp_url())
        .body("{}")
        .send()
        .await
        .expect("mcp endpoint request failed");
    assert!(
        mcp_response
            .status()
            .is_client_error()
            || mcp_response
                .status()
                .is_success()
    );
}

#[tokio::test]
async fn rust_mcp_container_supports_initialize_and_ping_tool() {
    let rust_mcp = RustMcpTestContainer::start()
        .await
        .expect("failed to start rust-mcp container");
    rust_mcp
        .wait_until_ready(Duration::from_secs(120))
        .await
        .expect("container did not become ready");

    let initialize = rust_mcp
        .initialize_mcp()
        .await
        .expect("MCP initialize failed");
    assert!(
        initialize
            .get("result")
            .is_some()
    );

    let ping = rust_mcp
        .call_tool("ping", json!({ "message": "e2e-smoke" }))
        .await
        .expect("MCP tools/call ping failed");
    assert!(ping.get("result").is_some());
}
