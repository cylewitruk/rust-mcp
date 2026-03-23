//! Live integration tests for index operations against real upstreams.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context as _, Result};
use rust_mcp::http;
use rust_mcp::state::AppState;
use rust_mcp_testing::local_mcp::LocalMcpHttpHarness;
use rust_mcp_testing::postgres::PostgresTestContainer;
use serde_json::{Value, json};

use super::common;

const LIVE_CARGO_REGISTRY_ENV: &str = "RUST_MCP_LIVE_CARGO_REGISTRY_DIR";

struct LiveContext {
    mcp: LocalMcpHttpHarness,
    #[allow(dead_code)]
    state: AppState,
    _postgres: PostgresTestContainer,
}

async fn live_context(cargo_registry_dir: PathBuf) -> Result<LiveContext> {
    let postgres = PostgresTestContainer::start().await?;

    let mut config = common::test_config(
        postgres
            .connection_string()
            .to_string(),
        PathBuf::from("/tmp"),
    );
    config.cargo_registry_dir = cargo_registry_dir;

    let state = AppState::connect(config.clone()).await?;
    state.run_migrations().await?;

    let router = http::router(state.clone(), config, common::test_prometheus_handle());
    let mcp = LocalMcpHttpHarness::spawn(router).await?;
    mcp.wait_until_ready(Duration::from_secs(30))
        .await?;
    let _ = mcp
        .initialize("live-index-integration")
        .await?;

    Ok(LiveContext {
        mcp,
        state,
        _postgres: postgres,
    })
}

#[tokio::test]
async fn live_local_cargo_registry_refresh_indexes_real_sources() -> Result<()> {
    let cargo_registry_dir = std::env::var(LIVE_CARGO_REGISTRY_ENV)
        .map(PathBuf::from)
        .expect(
            "RUST_MCP_LIVE_CARGO_REGISTRY_DIR must be set when running live-tests (e.g. \
             ~/.cargo/registry/src/index.crates.io-*)",
        );

    let context = live_context(cargo_registry_dir).await?;

    let sync_response = context
        .mcp
        .call_tool(
            "index_crates",
            json!({
                "crates": [{ "name": "tokio" }],
                "include_dependencies": false
            }),
        )
        .await
        .context("index_crates live call failed")?;
    let sync_payload = common::structured_content(&sync_response);

    assert!(
        sync_payload
            .get("succeeded")
            .and_then(Value::as_u64)
            .unwrap_or_default()
            >= 1,
        "expected at least one crate to be synced from live crates.io"
    );

    let refresh_response = context
        .mcp
        .call_tool(
            "index_refresh",
            json!({
                "scope": "local_cache",
                "crate_name": "tokio"
            }),
        )
        .await
        .context("index_refresh local_cache call failed")?;
    let refresh_payload = common::structured_content(&refresh_response);

    assert!(matches!(
        refresh_payload
            .get("status")
            .and_then(Value::as_str),
        Some("completed" | "completed_with_errors")
    ));

    let indexed_source_files = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*)::BIGINT
         FROM source_files sf
         JOIN crate_versions cv ON cv.id = sf.crate_version_id
         JOIN crates c ON c.id = cv.crate_id
         WHERE c.name = 'tokio'",
    )
    .fetch_one(&context.state.db)
    .await?;

    assert!(
        indexed_source_files > 0,
        "expected local cargo registry refresh to index tokio source files"
    );

    Ok(())
}
