//! Live tests for OSV (Open Source Vulnerabilities) API integration.
//!
//! These tests query the real `api.osv.dev` endpoint to verify request
//! construction, response parsing, and error handling.

use anyhow::{Result, anyhow};
use rust_mcp::integration::osv::OsvDevClient;
use rust_mcp_testing::postgres::PostgresTestContainer;

use super::common;

async fn osv_state() -> Result<(rust_mcp::state::AppState, PostgresTestContainer)> {
    let postgres = PostgresTestContainer::start().await?;
    let config = common::test_config(
        postgres
            .connection_string()
            .to_string(),
        std::path::PathBuf::from("/tmp"),
    );
    let state = rust_mcp::state::AppState::connect(config).await?;
    state.run_migrations().await?;
    Ok((state, postgres))
}

// ──────────────────────────────────────────────────────────────────────────────
// Known vulnerable crate
// ──────────────────────────────────────────────────────────────────────────────

/// `hyper` has known historical CVEs. Verify `query_crate` returns at least
/// one vulnerability entry.
#[tokio::test]
async fn live_osv_query_returns_vulns_for_hyper() -> Result<()> {
    let (state, _pg) = osv_state().await?;
    let client = OsvDevClient::new(&state);

    let response = client
        .query_crate("hyper")
        .await
        .map_err(|e| anyhow!("OSV query for hyper failed: {e}"))?;

    assert!(
        !response.vulns.is_empty(),
        "hyper should have at least one known vulnerability in OSV"
    );

    let first = &response.vulns[0];
    assert!(!first.id.is_empty(), "vulnerability should have an ID");
    // OSV IDs for Rust crates typically start with RUSTSEC- or GHSA-
    assert!(
        first
            .id
            .starts_with("RUSTSEC-")
            || first.id.starts_with("GHSA-"),
        "expected RUSTSEC- or GHSA- prefix, got: {}",
        first.id
    );

    Ok(())
}

// ──────────────────────────────────────────────────────────────────────────────
// Crate with no known vulns
// ──────────────────────────────────────────────────────────────────────────────

/// `itoa` is a tiny utility crate with no known CVEs. Verify `query_crate`
/// returns an empty list (not an error).
#[tokio::test]
async fn live_osv_query_returns_empty_for_itoa() -> Result<()> {
    let (state, _pg) = osv_state().await?;
    let client = OsvDevClient::new(&state);

    let response = client
        .query_crate("itoa")
        .await
        .map_err(|e| anyhow!("OSV query for itoa failed: {e}"))?;

    assert!(
        response.vulns.is_empty(),
        "itoa should have no known vulnerabilities, got {} entries",
        response.vulns.len()
    );

    Ok(())
}

// ──────────────────────────────────────────────────────────────────────────────
// Nonexistent crate
// ──────────────────────────────────────────────────────────────────────────────

/// Querying for a crate that doesn't exist should return an empty list, not
/// an error.
#[tokio::test]
async fn live_osv_query_returns_empty_for_nonexistent_crate() -> Result<()> {
    let (state, _pg) = osv_state().await?;
    let client = OsvDevClient::new(&state);

    let response = client
        .query_crate("__totally_fake_crate_name_12345__")
        .await
        .map_err(|e| anyhow!("OSV query for nonexistent crate failed: {e}"))?;

    assert!(response.vulns.is_empty(), "nonexistent crate should have no vulnerabilities");

    Ok(())
}
