//! E2E coverage for MCP tool behavior over HTTP transport.

use rust_mcp_testing::rust_mcp::RustMcpTestContainer;
use serde_json::{Value, json};

use super::crates_io_fixtures::{
    ManifestFixtureMount, MockRegistryServer, SEEDED_ALT_CRATE_NAME, SEEDED_ALT_CRATE_VERSION,
    SEEDED_CRATE_NAME, SEEDED_CRATE_NEXT_VERSION, SEEDED_CRATE_VERSION, mock_registry_env,
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

struct SeededContext {
    _mock_server: MockRegistryServer,
    rust_mcp: RustMcpTestContainer,
}

struct SeededManifestContext {
    _mock_server: MockRegistryServer,
    _manifest_fixture: ManifestFixtureMount,
    rust_mcp: RustMcpTestContainer,
    manifest_path: String,
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

async fn seeded_indexed_manifest_context() -> SeededManifestContext {
    let mock_server = MockRegistryServer::start()
        .await
        .expect("failed to start mock crates/docs server");
    let manifest_fixture =
        ManifestFixtureMount::create().expect("failed to create manifest fixture");
    let rust_mcp = start_ready_rust_mcp_with_env_and_mounts(
        mock_registry_env(&mock_server),
        vec![manifest_fixture.bind_mount()],
    )
    .await;

    let _ = initialize_mcp_session(&rust_mcp).await;

    seed_index_data(&rust_mcp).await;

    SeededManifestContext {
        _mock_server: mock_server,
        manifest_path: manifest_fixture
            .container_manifest_path()
            .to_string(),
        _manifest_fixture: manifest_fixture,
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
        "index.sync_crates",
        json!({
            "query": "demo",
            "page": 1,
            "per_page": 10,
            "include_dependencies": include_dependencies
        }),
    )
    .await
}

async fn refresh_seeded_rustdoc_json(rust_mcp: &RustMcpTestContainer) -> Value {
    call_tool_payload(
        rust_mcp,
        "index.refresh",
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
        "index.refresh",
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
            .get("synced_crates")
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
