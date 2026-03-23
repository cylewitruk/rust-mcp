//! E2E coverage for MCP tool behavior over HTTP transport.

use rust_mcp_testing::fixtures::{RustdocFixtureDir, materialize_workspace_rustdoc_fixture_zst};
use rust_mcp_testing::rust_mcp::RustMcpTestContainer;
use serde_json::{Value, json};

use super::crates_io_fixtures::{
    MockRegistryServer, SEEDED_ALT_CRATE_NAME, SEEDED_ALT_CRATE_VERSION, SEEDED_CRATE_NAME,
    SEEDED_CRATE_NEXT_VERSION, SEEDED_CRATE_VERSION, mock_registry_env,
};
use super::docs_rs_fixtures::SEEDED_RUSTDOC_PATH;
use super::helpers::{
    first_content_text, initialize_mcp_session, start_ready_rust_mcp,
    start_ready_rust_mcp_with_env, start_ready_rust_mcp_with_env_and_mounts, structured_content,
    tool_result,
};

mod core;
mod crate_tools;
mod dependency_tools;
mod docs_tools;
mod index_tools;
mod metrics;
mod source_tools;
mod symbol_tools;

const RUSTDOC_FALLBACK_MOUNT_PATH: &str = "/e2e-rustdoc-json";
const TOKIO_FALLBACK_FIXTURE_FILE: &str = "tokio_1.49.0_x86_64-unknown-linux-gnu_latest.json.zst";
const TOKIO_FALLBACK_CRATE_NAME: &str = "tokio";
const TOKIO_FALLBACK_CRATE_VERSION: &str = "1.49.0";
const TOKIO_FALLBACK_LOCAL_SOURCE_PATH: &str = "rustdoc-json/tokio-1.49.0.json";

struct SeededContext {
    _mock_server: MockRegistryServer,
    rust_mcp: RustMcpTestContainer,
}

struct SeededRustdocFallbackContext {
    _mock_server: MockRegistryServer,
    _rustdoc_fixture_dir: RustdocFixtureDir,
    rust_mcp: RustMcpTestContainer,
}

async fn initialized_container() -> RustMcpTestContainer {
    let rust_mcp = start_ready_rust_mcp().await;
    let _ = initialize_mcp_session(&rust_mcp).await;
    rust_mcp
}

async fn seeded_initialized_context() -> SeededContext {
    let mock_server = MockRegistryServer::start()
        .await
        .expect("failed to start mock crates/docs server");
    let rust_mcp = start_ready_rust_mcp_with_env(mock_registry_env(&mock_server)).await;
    let _ = initialize_mcp_session(&rust_mcp).await;

    SeededContext {
        _mock_server: mock_server,
        rust_mcp,
    }
}

async fn seeded_indexed_context() -> SeededContext {
    let context = seeded_initialized_context().await;
    seed_index_data(&context.rust_mcp).await;
    context
}

async fn seeded_rustdoc_fallback_initialized_context() -> SeededRustdocFallbackContext {
    let mock_server = MockRegistryServer::start()
        .await
        .expect("failed to start mock crates/docs server");
    let rustdoc_fixture_dir = materialize_workspace_rustdoc_fixture_zst(
        TOKIO_FALLBACK_FIXTURE_FILE,
        TOKIO_FALLBACK_CRATE_NAME,
        TOKIO_FALLBACK_CRATE_VERSION,
    )
    .expect("failed to materialize tokio rustdoc fixture");

    let mut env = mock_registry_env(&mock_server);
    env.retain(|(key, _)| key != "DOCS_RS_BASE_URL");
    // Force docs.rs rustdoc fetches to fail so the server must use local fallback.
    env.push(("DOCS_RS_BASE_URL".to_string(), "http://127.0.0.1:1".to_string()));
    env.push(("RUSTDOC_JSON_DIR".to_string(), RUSTDOC_FALLBACK_MOUNT_PATH.to_string()));

    let rust_mcp = start_ready_rust_mcp_with_env_and_mounts(
        env,
        vec![(
            rustdoc_fixture_dir
                .path()
                .to_string_lossy()
                .to_string(),
            RUSTDOC_FALLBACK_MOUNT_PATH.to_string(),
        )],
    )
    .await;
    let _ = initialize_mcp_session(&rust_mcp).await;

    SeededRustdocFallbackContext {
        _mock_server: mock_server,
        _rustdoc_fixture_dir: rustdoc_fixture_dir,
        rust_mcp,
    }
}

async fn call_tool_response(
    rust_mcp: &RustMcpTestContainer,
    tool_name: &str,
    arguments: Value,
) -> Value {
    rust_mcp
        .call_tool(tool_name, arguments)
        .await
        .unwrap_or_else(|error| panic!("MCP tools/call {tool_name} failed: {error}"))
}

async fn call_tool_payload(
    rust_mcp: &RustMcpTestContainer,
    tool_name: &str,
    arguments: Value,
) -> Value {
    let response = call_tool_response(rust_mcp, tool_name, arguments).await;
    structured_content(tool_result(&response, tool_name), tool_name).clone()
}

async fn sync_seeded_demo_crates(
    rust_mcp: &RustMcpTestContainer,
    include_dependencies: bool,
) -> Value {
    call_tool_payload(
        rust_mcp,
        "index_crates",
        json!({
            "crates": [
                { "name": SEEDED_CRATE_NAME },
                { "name": SEEDED_ALT_CRATE_NAME }
            ],
            "include_dependencies": include_dependencies
        }),
    )
    .await
}

async fn refresh_seeded_rustdoc_json(rust_mcp: &RustMcpTestContainer) -> Value {
    call_tool_payload(
        rust_mcp,
        "index_refresh",
        json!({
            "scope": "rustdoc_json",
            "page": 1,
            "per_page": 20
        }),
    )
    .await
}

async fn refresh_seeded_docs_pages(rust_mcp: &RustMcpTestContainer) -> Value {
    call_tool_payload(
        rust_mcp,
        "index_refresh",
        json!({
            "scope": "docs",
            "page": 1,
            "per_page": 20
        }),
    )
    .await
}

async fn seed_index_data(rust_mcp: &RustMcpTestContainer) {
    let sync_payload = sync_seeded_demo_crates(rust_mcp, true).await;
    assert!(
        sync_payload
            .get("succeeded")
            .and_then(Value::as_u64)
            .is_some_and(|count| count >= 2),
        "expected at least two synced crates in fixture sync: {sync_payload}"
    );

    let refresh_payload = refresh_seeded_rustdoc_json(rust_mcp).await;
    assert!(
        matches!(
            refresh_payload
                .get("status")
                .and_then(Value::as_str),
            Some("completed") | Some("completed_with_errors")
        ),
        "unexpected index.refresh status payload: {refresh_payload}"
    );
}

async fn seed_docs_index_data(rust_mcp: &RustMcpTestContainer) {
    seed_index_data(rust_mcp).await;

    let refresh_payload = refresh_seeded_docs_pages(rust_mcp).await;
    assert!(
        matches!(
            refresh_payload
                .get("status")
                .and_then(Value::as_str),
            Some("completed") | Some("completed_with_errors")
        ),
        "unexpected docs index.refresh status payload: {refresh_payload}"
    );

    let result = refresh_payload
        .get("result")
        .and_then(Value::as_object)
        .expect("docs index.refresh should return result object");
    assert!(
        result
            .get("synced_versions")
            .and_then(Value::as_u64)
            .is_some_and(|count| count >= 1),
        "expected docs sync to write at least one docs page: {refresh_payload}"
    );
}

async fn seeded_docs_indexed_context() -> SeededContext {
    let context = seeded_initialized_context().await;
    seed_docs_index_data(&context.rust_mcp).await;
    context
}
