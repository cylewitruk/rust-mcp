use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use rust_mcp::config::{Config, LogFormat};
use rust_mcp::http;
use rust_mcp::state::AppState;
use rust_mcp_testing::fixtures::{
    MinimalCrateGraphFixture, seed_minimal_crate_graph, write_fixture_source_to_registry,
};
use rust_mcp_testing::local_mcp::LocalMcpHttpHarness;
use rust_mcp_testing::postgres::PostgresTestContainer;
use serde_json::Value;

/// Creates a test-only `PrometheusHandle` without installing a global recorder.
pub fn test_prometheus_handle() -> PrometheusHandle {
    PrometheusBuilder::new()
        .build_recorder()
        .handle()
}

pub fn test_config(database_url: String, cargo_registry_dir: PathBuf) -> Config {
    Config {
        http_bind: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)),
        mcp_sse_keep_alive_secs: 15,
        mcp_sse_retry_ms: 3000,
        database_url,
        crates_io_base_url: "https://crates.io".to_string(),
        crates_io_user_agent: "rust-mcp-tests/0.1.0".to_string(),
        crates_io_timeout_secs: 20,
        crates_io_min_interval_ms: 1,
        crates_io_window_max_requests: 0,
        crates_io_window_duration_secs: 1,
        docs_rs_base_url: "https://docs.rs".to_string(),
        docs_rs_min_interval_ms: 1,
        docs_rs_window_max_requests: 0,
        docs_rs_window_duration_secs: 1,
        osv_min_interval_ms: 1,
        osv_window_max_requests: 0,
        osv_window_duration_secs: 1,
        database_min_connections: 1,
        database_max_connections: 4,
        max_concurrent_requests: 32,
        auto_migrate: false,
        cargo_registry_dir,
        crate_source_cache_dir: PathBuf::from("/tmp/rust-mcp-test-crate-sources"),
        data_dir: PathBuf::from("/tmp"),
        rustsec_db_dir: None,
        rustdoc_json_dir: None,
        schema_export_dir: None,
        rust_log: "warn".to_string(),
        log_format: LogFormat::Pretty,
        mcp_strict_accept: true,
        registry_scan_interval_secs: 0,
        registry_scan_batch_limit: 0,
        pre_warm_crates: String::new(),
        enrichment_maintenance_interval_secs: 0,
        rustdoc_retry_cooldown_secs: 86400,
        registry_cache_watch_interval_ms: 0,
        security_sync_interval_secs: 0,
        security_sync_batch_size: 50,
    }
}

pub struct SeededMcpContext {
    pub mcp: LocalMcpHttpHarness,
    #[allow(dead_code)]
    pub state: AppState,
    #[allow(dead_code)]
    pub fixture: MinimalCrateGraphFixture,
    pub registry_dir: PathBuf,
    _postgres: PostgresTestContainer,
}

impl Drop for SeededMcpContext {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.registry_dir);
    }
}

pub async fn seeded_mcp_context() -> Result<SeededMcpContext> {
    let postgres = PostgresTestContainer::start().await?;

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let registry_dir = std::env::temp_dir().join(format!("rust-mcp-test-registry-{nanos}"));
    std::fs::create_dir_all(&registry_dir)?;

    let config = test_config(
        postgres
            .connection_string()
            .to_string(),
        registry_dir.clone(),
    );
    let state = AppState::connect(config.clone()).await?;
    state.run_migrations().await?;
    let fixture = seed_minimal_crate_graph(&state.db).await?;

    // Write the minimal crate graph's source file to the fixture registry so
    // tool handlers that read from disk (source.read, source.search, etc.)
    // can find it.
    write_fixture_source_to_registry(
        &registry_dir,
        "serde_json",
        "1.0.145",
        "src/lib.rs",
        "pub fn from_str<T>() -> Result<T, Error> { todo!() }",
    );

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

    let router = http::router(state.clone(), config, test_prometheus_handle());
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
        registry_dir,
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
