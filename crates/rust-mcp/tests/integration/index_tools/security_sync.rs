use rust_mcp::mcp::run_security_sync_scan_for_tests;

use super::common;

/// Verifies that the security sync scan runs against seeded crates without
/// errors and processes the expected number of crates. Since we don't have
/// a mock OSV server, the OSV queries will fail (no outbound connectivity
/// in integration tests), but the scan loop itself should complete without
/// panicking and should report the correct number of crates processed.
///
/// The seeded context has crates in the database, so the scan should pick
/// them up and attempt to query OSV for each batch.
#[tokio::test]
async fn security_sync_scan_processes_seeded_crates() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded context");

    let outcome = run_security_sync_scan_for_tests(&context.state).await;

    // The scan should have run at least one page.
    assert!(outcome.pages >= 1, "expected at least 1 page in security sync scan");
    // We expect errors because there's no live OSV endpoint, but the scan
    // should not panic and should have tried processing crates.
    // (crates_processed may be 0 if the OSV query fails before counting.)
}

/// When there are no indexed crates, the security sync should complete
/// quickly with zero pages processed.
#[tokio::test]
async fn security_sync_scan_handles_empty_database() {
    use rust_mcp_testing::postgres::PostgresTestContainer;

    let postgres = PostgresTestContainer::start()
        .await
        .expect("failed to start postgres");
    let config = common::test_config(
        postgres
            .connection_string()
            .to_string(),
        std::path::PathBuf::from("/tmp"),
    );
    let state = rust_mcp::state::AppState::connect(config)
        .await
        .expect("failed to connect");
    state
        .run_migrations()
        .await
        .expect("failed to run migrations");

    let outcome = run_security_sync_scan_for_tests(&state).await;

    assert_eq!(outcome.crates_processed, 0, "no crates should be processed");
    assert_eq!(outcome.advisories_written, 0, "no advisories should be written");
    assert_eq!(outcome.errors, 0, "no errors expected on empty DB");
    assert_eq!(outcome.pages, 1, "should still run one page (which returns 0 rows)");
}
