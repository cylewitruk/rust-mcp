//! Docker-backed integration test for AppState + migrations.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;

use rust_mcp::config::{Config, LogFormat, TransportMode};
use rust_mcp::state::AppState;
use rust_mcp_testing::fixtures::seed_minimal_crate_graph;
use rust_mcp_testing::postgres::PostgresTestContainer;

fn test_config(database_url: String) -> Config {
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

#[tokio::test]
async fn app_state_connects_and_migrates_against_testcontainer_postgres() {
    let postgres = PostgresTestContainer::start()
        .await
        .expect("failed to start postgres test container");
    let state = AppState::connect(test_config(
        postgres
            .connection_string()
            .to_string(),
    ))
    .await
    .expect("failed to connect AppState to test postgres");

    state
        .run_migrations()
        .await
        .expect("failed to run migrations");
    state
        .readiness_check()
        .await
        .expect("readiness check failed");

    let crates_table_exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (
            SELECT 1
            FROM information_schema.tables
            WHERE table_schema = 'public' AND table_name = 'crates'
        )",
    )
    .fetch_one(&state.db)
    .await
    .expect("failed to check crates table existence");
    assert!(crates_table_exists);

    let pg_trgm_installed = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (
            SELECT 1
            FROM pg_extension
            WHERE extname = 'pg_trgm'
        )",
    )
    .fetch_one(&state.db)
    .await
    .expect("failed to check pg_trgm extension");
    assert!(pg_trgm_installed);
}

#[tokio::test]
async fn fixture_helpers_seed_rows_for_tool_level_tests() {
    let postgres = PostgresTestContainer::start()
        .await
        .expect("failed to start postgres test container");
    let state = AppState::connect(test_config(
        postgres
            .connection_string()
            .to_string(),
    ))
    .await
    .expect("failed to connect AppState to test postgres");
    state
        .run_migrations()
        .await
        .expect("failed to run migrations");

    let fixture = seed_minimal_crate_graph(&state.db)
        .await
        .expect("failed to seed minimal crate graph fixture");

    assert!(fixture.dependent.crate_id > 0);
    assert!(fixture.dependent.version_id > 0);
    assert!(fixture.dependency.crate_id > 0);
    assert!(fixture.dependency.version_id > 0);
    assert!(fixture.dependency_edge_id > 0);
    assert!(fixture.source_file_id > 0);
    assert!(fixture.symbol_id > 0);
    assert!(fixture.docs_page_id > 0);

    let crate_count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM crates")
        .fetch_one(&state.db)
        .await
        .expect("failed to count crates");
    assert_eq!(crate_count, 2);

    let symbol_count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM symbols")
        .fetch_one(&state.db)
        .await
        .expect("failed to count symbols");
    assert_eq!(symbol_count, 1);
}
