//! Live tests for registry discovery against the real `~/.cargo/registry`.
//!
//! These verify that `parse_registry_dir_name` and the full scan pipeline
//! correctly handle real-world directory names, including those with semver
//! build metadata containing hyphens (e.g. `toml_parser-1.0.6+spec-1.1.0`).

use std::path::PathBuf;

use anyhow::{Context as _, Result};
use rust_mcp::mcp::run_registry_scan_with_outcome_for_tests;
use rust_mcp::state::AppState;
use rust_mcp_testing::postgres::PostgresTestContainer;

use super::common;

const LIVE_CARGO_REGISTRY_ENV: &str = "RUST_MCP_LIVE_CARGO_REGISTRY_DIR";

// ──────────────────────────────────────────────────────────────────────────────
// Helpers
// ──────────────────────────────────────────────────────────────────────────────

struct LiveDiscoveryContext {
    state: AppState,
    _postgres: PostgresTestContainer,
}

async fn live_discovery_context(cargo_registry_dir: PathBuf) -> Result<LiveDiscoveryContext> {
    let postgres = PostgresTestContainer::start().await?;
    let mut config = common::test_config(
        postgres
            .connection_string()
            .to_string(),
        std::path::PathBuf::from("/tmp"),
    );
    config.cargo_registry_dir = cargo_registry_dir;

    let state = AppState::connect(config).await?;
    state.run_migrations().await?;

    Ok(LiveDiscoveryContext { state, _postgres: postgres })
}

fn cargo_registry_dir() -> PathBuf {
    std::env::var(LIVE_CARGO_REGISTRY_ENV)
        .map(PathBuf::from)
        .expect(
            "RUST_MCP_LIVE_CARGO_REGISTRY_DIR must be set when running live-tests (e.g. \
             ~/.cargo/registry)",
        )
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

/// Walk the real cargo registry and verify that the discovery scan completes
/// successfully with a reasonable number of discovered crates. This catches
/// panics or logic errors when parsing real-world directory names.
#[tokio::test]
async fn live_registry_scan_completes_without_errors() -> Result<()> {
    let context = live_discovery_context(cargo_registry_dir()).await?;

    let outcome = run_registry_scan_with_outcome_for_tests(&context.state).await;

    assert!(
        outcome.discovered > 0,
        "should discover at least one crate directory in the local registry"
    );
    assert!(outcome.enqueued > 0, "fresh database should enqueue at least one crate");
    // A small number of unparseable entries is acceptable, but it should
    // be a tiny fraction of the total.
    let unparseable_ratio = if outcome.discovered > 0 {
        outcome.unparseable as f64 / outcome.discovered as f64
    } else {
        0.0
    };
    assert!(
        unparseable_ratio < 0.05,
        "unparseable directories should be <5% of discovered (got {}/{} = {:.1}%)",
        outcome.unparseable,
        outcome.discovered,
        unparseable_ratio * 100.0
    );

    Ok(())
}

/// Verify that no enqueued crate names contain characters that are invalid in
/// crate names (`.` or `+`), which would indicate a parsing bug where the
/// split landed inside semver build metadata.
#[tokio::test]
async fn live_registry_scan_produces_valid_crate_names() -> Result<()> {
    let context = live_discovery_context(cargo_registry_dir()).await?;

    run_registry_scan_with_outcome_for_tests(&context.state).await;

    let bad_names: Vec<String> = sqlx::query_scalar(
        "SELECT crate_name FROM refresh_jobs
         WHERE scope = 'crate'
           AND (crate_name LIKE '%.%' OR crate_name LIKE '%+%')",
    )
    .fetch_all(&context.state.db)
    .await
    .context("failed to query refresh_jobs for invalid crate names")?;

    assert!(
        bad_names.is_empty(),
        "found crate names containing '.' or '+', indicating a build metadata parsing bug: {:?}",
        &bad_names[..bad_names.len().min(10)]
    );

    Ok(())
}
