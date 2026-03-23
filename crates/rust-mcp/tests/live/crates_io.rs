//! Live tests that exercise the crates.io integration client against real
//! upstream endpoints. These verify that URL construction, redirect following,
//! and error handling work against the actual crates.io + static.crates.io
//! infrastructure.

use std::time::Duration;

use anyhow::{Context as _, Result, anyhow};
use rust_mcp::http;
use rust_mcp::integration::crates_io::CratesIoClient;
use rust_mcp::state::AppState;
use rust_mcp_testing::local_mcp::LocalMcpHttpHarness;
use rust_mcp_testing::postgres::PostgresTestContainer;
use serde_json::{Value, json};

use super::common;

// ──────────────────────────────────────────────────────────────────────────────
// Helpers
// ──────────────────────────────────────────────────────────────────────────────

struct LiveCratesIoContext {
    mcp: LocalMcpHttpHarness,
    state: AppState,
    _postgres: PostgresTestContainer,
}

async fn live_crates_io_context() -> Result<LiveCratesIoContext> {
    let postgres = PostgresTestContainer::start().await?;
    let config = common::test_config(
        postgres
            .connection_string()
            .to_string(),
        std::path::PathBuf::from("/tmp"),
    );
    let state = AppState::connect(config.clone()).await?;
    state.run_migrations().await?;

    let router = http::router(state.clone(), config, common::test_prometheus_handle());
    let mcp = LocalMcpHttpHarness::spawn(router).await?;
    mcp.wait_until_ready(Duration::from_secs(30))
        .await?;
    let _ = mcp
        .initialize("live-crates-io-integration")
        .await?;

    Ok(LiveCratesIoContext {
        mcp,
        state,
        _postgres: postgres,
    })
}

// ──────────────────────────────────────────────────────────────────────────────
// Direct CratesIoClient tests
// ──────────────────────────────────────────────────────────────────────────────

/// `serde` always has a README. Verify `fetch_readme` returns `Some(...)`.
#[tokio::test]
async fn live_fetch_readme_succeeds_for_serde() -> Result<()> {
    let context = live_crates_io_context().await?;
    let client = CratesIoClient::new(&context.state);

    let readme = client
        .fetch_readme("serde", "1.0.193")
        .await
        .map_err(|e| anyhow!("fetch_readme for serde@1.0.193 should not error: {e}"))?;

    assert!(readme.is_some(), "serde@1.0.193 should have a README on crates.io");
    let text = readme.unwrap();
    assert!(text.contains("Serde") || text.contains("serde"), "README should mention serde");

    Ok(())
}

/// Versions with semver build metadata (e.g. `toml_datetime@1.0.0+spec-1.1.0`)
/// are real crates on crates.io. Verify `fetch_readme` succeeds or returns
/// `None` — it must NOT return an error.
#[tokio::test]
async fn live_fetch_readme_handles_build_metadata_version() -> Result<()> {
    let context = live_crates_io_context().await?;
    let client = CratesIoClient::new(&context.state);

    // toml_datetime publishes versions with +spec build metadata.
    // We don't care if it has a README; we care that no 403/error propagates.
    let result = client
        .fetch_readme("toml_datetime", "1.0.0+spec-1.1.0")
        .await;

    assert!(
        result.is_ok(),
        "fetch_readme for toml_datetime@1.0.0+spec-1.1.0 should not error, got: {:?}",
        result.err()
    );

    Ok(())
}

/// Fetching a README for a nonexistent crate version should return `None` (404)
/// or an error indicating the crate doesn't exist — not a panic or 403 leak.
#[tokio::test]
async fn live_fetch_readme_returns_none_for_nonexistent_version() -> Result<()> {
    let context = live_crates_io_context().await?;
    let client = CratesIoClient::new(&context.state);

    let result = client
        .fetch_readme("serde", "0.0.0-nonexistent")
        .await;

    // crates.io should return 404 for a version that doesn't exist,
    // which our client maps to Ok(None).
    match result {
        Ok(None) => {} // expected
        Ok(Some(_)) => panic!("should not find a README for a nonexistent version"),
        Err(e) => {
            // Acceptable if the error is a 404-like message
            assert!(
                e.contains("404") || e.contains("not found") || e.contains("Not Found"),
                "unexpected error for nonexistent version: {e}"
            );
        }
    }

    Ok(())
}

// ──────────────────────────────────────────────────────────────────────────────
// Full crate sync flow through MCP
// ──────────────────────────────────────────────────────────────────────────────

/// Sync `serde` through the full MCP `index_crates` flow and verify that
/// the crate metadata, versions, and README are persisted correctly.
#[tokio::test]
async fn live_sync_serde_stores_metadata_and_readme() -> Result<()> {
    let context = live_crates_io_context().await?;

    let response = context
        .mcp
        .call_tool(
            "index_crates",
            json!({
                "crates": [{ "name": "serde" }],
                "include_dependencies": true
            }),
        )
        .await
        .context("index_crates for serde failed")?;

    let payload = common::structured_content(&response);
    assert!(
        payload
            .get("succeeded")
            .and_then(Value::as_u64)
            .unwrap_or_default()
            >= 1,
        "expected at least one crate synced"
    );

    // Verify crate was stored in DB
    let crate_exists =
        sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM crates WHERE name = 'serde')")
            .fetch_one(&context.state.db)
            .await?;
    assert!(crate_exists, "serde should be in the crates table");

    // Verify at least one version was stored
    let version_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*)::BIGINT
         FROM crate_versions cv
         JOIN crates c ON c.id = cv.crate_id
         WHERE c.name = 'serde'",
    )
    .fetch_one(&context.state.db)
    .await?;
    assert!(version_count > 0, "serde should have at least one version stored");

    // Verify README was stored for the selected version
    let readme_stored = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(
            SELECT 1
            FROM crate_versions cv
            JOIN crates c ON c.id = cv.crate_id
            WHERE c.name = 'serde'
              AND cv.readme IS NOT NULL
              AND LENGTH(cv.readme) > 0
        )",
    )
    .fetch_one(&context.state.db)
    .await?;
    assert!(readme_stored, "serde should have a README stored in at least one version");

    // Verify dependencies were stored
    let dep_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*)::BIGINT
         FROM dependency_edges d
         JOIN crate_versions cv ON cv.id = d.from_version_id
         JOIN crates c ON c.id = cv.crate_id
         WHERE c.name = 'serde'",
    )
    .fetch_one(&context.state.db)
    .await?;
    assert!(dep_count > 0, "serde should have at least one dependency stored");

    Ok(())
}

/// Sync a crate that uses build metadata versions (`toml_datetime`) and
/// verify the full flow completes without errors.
#[tokio::test]
async fn live_sync_crate_with_build_metadata_versions() -> Result<()> {
    let context = live_crates_io_context().await?;

    let response = context
        .mcp
        .call_tool(
            "index_crates",
            json!({
                "crates": [{ "name": "toml_datetime" }],
                "include_dependencies": false
            }),
        )
        .await
        .context("index_crates for toml_datetime failed")?;

    let payload = common::structured_content(&response);
    assert!(
        payload
            .get("succeeded")
            .and_then(Value::as_u64)
            .unwrap_or_default()
            >= 1,
        "expected at least one crate synced for toml_datetime"
    );

    // Verify it was stored
    let crate_exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM crates WHERE name = 'toml_datetime')",
    )
    .fetch_one(&context.state.db)
    .await?;
    assert!(crate_exists, "toml_datetime should be in the crates table after sync");

    Ok(())
}
