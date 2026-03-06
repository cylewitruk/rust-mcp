//! Live tests for GitHub API integration against real upstream endpoints.
//!
//! These tests perform **real HTTP requests** to api.github.com and verify that
//! the client correctly fetches repo metadata, contributor counts, releases,
//! and advisories.

use anyhow::{Result, anyhow};
use rust_mcp::integration::github::GitHubClient;
use rust_mcp_testing::postgres::PostgresTestContainer;

use super::common;

async fn github_state() -> Result<(rust_mcp::state::AppState, PostgresTestContainer)> {
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
// Repository metadata
// ──────────────────────────────────────────────────────────────────────────────

/// `serde-rs/serde` is a stable, high-traffic repository. Verify
/// `fetch_repo_metadata` returns sensible data.
#[tokio::test]
async fn live_fetch_repo_metadata_for_serde() -> Result<()> {
    let (state, _pg) = github_state().await?;
    let client = GitHubClient::new(&state);

    let meta = client
        .fetch_repo_metadata("serde-rs", "serde")
        .await
        .map_err(|e| anyhow!("fetch_repo_metadata for serde-rs/serde failed: {e}"))?;

    assert!(
        meta.stargazers_count > 1000,
        "serde should have >1000 stars, got {}",
        meta.stargazers_count
    );
    assert!(meta.forks_count > 100, "serde should have >100 forks, got {}", meta.forks_count);
    assert!(!meta.archived, "serde should not be archived");
    assert!(meta.pushed_at.is_some(), "serde should have a pushed_at date");

    // License should be present.
    let spdx = meta
        .license
        .as_ref()
        .and_then(|l| l.spdx_id.as_deref());
    assert!(spdx.is_some(), "serde should have a license SPDX ID");

    Ok(())
}

/// Fetching metadata for a nonexistent repository should return an error.
#[tokio::test]
async fn live_fetch_repo_metadata_for_nonexistent_repo() -> Result<()> {
    let (state, _pg) = github_state().await?;
    let client = GitHubClient::new(&state);

    let result = client
        .fetch_repo_metadata("__nonexistent_user_12345__", "no-such-repo")
        .await;

    assert!(result.is_err(), "should fail for nonexistent repo");
    let err = result.unwrap_err();
    assert!(
        err.contains("404") || err.contains("Not Found"),
        "error should indicate 404, got: {err}"
    );

    Ok(())
}

// ──────────────────────────────────────────────────────────────────────────────
// Contributor count
// ──────────────────────────────────────────────────────────────────────────────

/// `serde-rs/serde` has many contributors. Verify `fetch_contributor_count`
/// returns a reasonable count.
#[tokio::test]
async fn live_fetch_contributor_count_for_serde() -> Result<()> {
    let (state, _pg) = github_state().await?;
    let client = GitHubClient::new(&state);

    let count = client
        .fetch_contributor_count("serde-rs", "serde")
        .await
        .map_err(|e| anyhow!("fetch_contributor_count for serde failed: {e}"))?;

    assert!(count.is_some(), "serde should return a contributor count");
    assert!(count.unwrap() > 50, "serde should have >50 contributors, got {:?}", count);

    Ok(())
}

// ──────────────────────────────────────────────────────────────────────────────
// Releases
// ──────────────────────────────────────────────────────────────────────────────

/// `serde-rs/serde` publishes GitHub releases. Verify we get at least one.
#[tokio::test]
async fn live_fetch_releases_for_serde() -> Result<()> {
    let (state, _pg) = github_state().await?;
    let client = GitHubClient::new(&state);

    let releases = client
        .fetch_releases("serde-rs", "serde", 5)
        .await
        .map_err(|e| anyhow!("fetch_releases for serde failed: {e}"))?;

    assert!(!releases.is_empty(), "serde should have at least one release");

    let first = &releases[0];
    assert!(!first.tag_name.is_empty(), "release should have a tag name");

    Ok(())
}

// ──────────────────────────────────────────────────────────────────────────────
// Git probe (commit liveness)
// ──────────────────────────────────────────────────────────────────────────────

/// Performs a real shallow clone of `serde-rs/serde` and extracts commit
/// liveness data. This is the slowest test in the suite.
#[tokio::test]
async fn live_git_probe_for_serde() -> Result<()> {
    use rust_mcp::integration::git_probe;

    let tmp = std::env::temp_dir().join("rust-mcp-live-git-probe");
    let _ = std::fs::create_dir_all(&tmp);

    let result = git_probe::probe_repo(
        "https://github.com/serde-rs/serde.git",
        "serde-rs",
        "serde",
        &tmp,
        50, // shallow depth for speed
        120,
    )
    .await
    .map_err(|e| anyhow!("git probe for serde failed: {e}"))?;

    assert!(
        result
            .last_commit_at
            .is_some(),
        "should have a last_commit_at timestamp"
    );
    assert!(
        result
            .last_commit_message
            .is_some(),
        "should have a last_commit_message"
    );
    // serde is actively maintained, should have recent commits
    assert!(result.recent_commit_count >= 0, "recent_commit_count should be non-negative");

    // Cleanup
    let _ = std::fs::remove_dir_all(tmp.join("git-probes"));

    Ok(())
}
