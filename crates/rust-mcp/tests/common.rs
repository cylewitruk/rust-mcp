use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use rust_mcp::config::{Config, LogFormat, TransportMode};
use rust_mcp::http;
use rust_mcp::state::AppState;
use rust_mcp_testing::fixtures::{MinimalCrateGraphFixture, seed_minimal_crate_graph};
use rust_mcp_testing::local_mcp::LocalMcpHttpHarness;
use rust_mcp_testing::postgres::PostgresTestContainer;
use serde_json::Value;

pub(crate) fn test_config(database_url: String) -> Config {
    Config {
        mcp_transport: TransportMode::Http,
        http_bind: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)),
        mcp_sse_keep_alive_secs: 15,
        mcp_sse_retry_ms: 3000,
        database_url,
        crates_io_base_url: "https://crates.io".to_string(),
        crates_io_user_agent: "rust-mcp-tests/0.1.0".to_string(),
        crates_io_timeout_secs: 20,
        crates_io_min_interval_ms: 1,
        docs_rs_base_url: "https://docs.rs".to_string(),
        docs_rs_min_interval_ms: 1,
        osv_min_interval_ms: 1,
        database_min_connections: 1,
        database_max_connections: 4,
        max_concurrent_requests: 32,
        prometheus_bind: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)),
        auto_migrate: false,
        cargo_registry_dir: PathBuf::from("/tmp"),
        data_dir: PathBuf::from("/tmp"),
        rustsec_db_dir: None,
        rustdoc_json_dir: None,
        rust_log: "warn".to_string(),
        log_format: LogFormat::Pretty,
    }
}

pub struct SeededMcpContext {
    pub mcp: LocalMcpHttpHarness,
    #[allow(dead_code)]
    pub state: AppState,
    #[allow(dead_code)]
    pub fixture: MinimalCrateGraphFixture,
    _postgres: PostgresTestContainer,
}

pub async fn seeded_mcp_context() -> Result<SeededMcpContext> {
    let postgres = PostgresTestContainer::start().await?;
    let config = test_config(
        postgres
            .connection_string()
            .to_string(),
    );
    let state = AppState::connect(config.clone()).await?;
    state.run_migrations().await?;
    let fixture = seed_minimal_crate_graph(&state.db).await?;

    // Disable proactive freshness probes to keep tests deterministic and offline.
    sqlx::query(
        "UPDATE crates
         SET last_checked_at = NOW(),
             next_check_at = NOW() + INTERVAL '30 days',
             ttl_hint_seconds = 86400,
             ttl_reason = 'integration_test_seed'",
    )
    .execute(&state.db)
    .await?;

    let router = http::router(state.clone(), config);
    let mcp = LocalMcpHttpHarness::spawn(router).await?;
    mcp.wait_until_ready(Duration::from_secs(20))
        .await?;
    let _ = mcp
        .initialize("mcp-tools-integration")
        .await?;

    Ok(SeededMcpContext {
        mcp,
        state,
        fixture,
        _postgres: postgres,
    })
}

#[allow(dead_code)]
pub fn structured_content(response: &Value) -> &Value {
    response
        .get("result")
        .and_then(|result| {
            result
                .get("structuredContent")
                .or_else(|| result.get("structured_content"))
        })
        .expect("tool response should include structured content")
}

#[allow(dead_code)]
pub fn first_content_text(response: &Value) -> &str {
    response
        .get("result")
        .and_then(|result| result.get("content"))
        .and_then(Value::as_array)
        .and_then(|content| content.first())
        .and_then(|first| first.get("text"))
        .and_then(Value::as_str)
        .expect("tool response should include text content")
}
