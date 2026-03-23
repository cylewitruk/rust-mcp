//! Live tests for on-demand crate source downloading from static.crates.io.
//!
//! These tests perform **real HTTP downloads** to verify the URL scheme,
//! tarball format, and extraction pipeline work end-to-end.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result};
use rust_mcp::http;
use rust_mcp::mcp::run_refresh_worker_for_tests;
use rust_mcp::mcp::utils::{read_source_file_from_disk_or_cache, resolve_source_dir};
use rust_mcp::state::AppState;
use rust_mcp_testing::local_mcp::LocalMcpHttpHarness;
use rust_mcp_testing::postgres::PostgresTestContainer;
use serde_json::{Value, json};

use super::common;

fn make_temp_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("rust-mcp-live-{label}-{nanos}"));
    std::fs::create_dir_all(&dir).expect("failed to create temp dir");
    dir
}

struct CleanupDir(PathBuf);
impl Drop for CleanupDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The URL template used to download `.crate` tarballs from static.crates.io.
fn crate_download_url(name: &str, version: &str) -> String {
    format!("https://static.crates.io/crates/{name}/{name}-{version}.crate")
}

/// Downloads and extracts a `.crate` tarball into `dest_dir`.
async fn download_and_extract(name: &str, version: &str, dest_dir: &PathBuf) -> Result<()> {
    let url = crate_download_url(name, version);
    let client = reqwest::Client::new();
    let response = client
        .get(&url)
        .send()
        .await?;

    assert!(response.status().is_success(), "expected 200 from {url}, got {}", response.status());

    let bytes = response.bytes().await?;
    assert!(bytes.len() > 100, "crate tarball should be non-trivial, got {} bytes", bytes.len());

    let gz = flate2::read::GzDecoder::new(&bytes[..]);
    let mut archive = tar::Archive::new(gz);
    archive.unpack(dest_dir)?;

    Ok(())
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

/// Downloads a real `.crate` from static.crates.io, extracts it, and verifies
/// the source files are readable. Uses `itoa` 1.0.1 — a tiny, stable crate.
#[tokio::test]
async fn live_download_real_crate_and_extract_source() -> Result<()> {
    let cache_dir = make_temp_dir("real-download");
    let _cleanup = CleanupDir(cache_dir.clone());

    let crate_name = "itoa";
    let version = "1.0.1";

    download_and_extract(crate_name, version, &cache_dir).await?;

    let extracted_dir = cache_dir.join(format!("{crate_name}-{version}"));
    assert!(extracted_dir.is_dir(), "expected {crate_name}-{version}/ directory after extraction");

    // Verify key files exist.
    let cargo_toml = extracted_dir.join("Cargo.toml");
    assert!(cargo_toml.exists(), "Cargo.toml should exist");
    let toml_content = std::fs::read_to_string(&cargo_toml)?;
    assert!(toml_content.contains("itoa"), "Cargo.toml should reference itoa");

    let lib_rs = extracted_dir.join("src/lib.rs");
    assert!(lib_rs.exists(), "src/lib.rs should exist");
    let lib_content = std::fs::read_to_string(&lib_rs)?;
    assert!(!lib_content.is_empty(), "src/lib.rs should have content");

    Ok(())
}

/// Verifies that `resolve_source_dir` discovers extracted sources in the
/// cache directory (second fallback after cargo registry).
#[tokio::test]
async fn live_resolve_source_dir_finds_cached_download() -> Result<()> {
    let cache_dir = make_temp_dir("resolve-cache");
    let _cleanup = CleanupDir(cache_dir.clone());
    let registry_dir = make_temp_dir("resolve-registry");
    let _cleanup2 = CleanupDir(registry_dir.clone());

    let crate_name = "itoa";
    let version = "1.0.1";

    // Before download, resolve_source_dir should return None.
    assert!(
        resolve_source_dir(&registry_dir, Some(&cache_dir), crate_name, version).is_none(),
        "should not find source before download"
    );

    download_and_extract(crate_name, version, &cache_dir).await?;

    // Now resolve_source_dir should find it in the cache.
    let resolved = resolve_source_dir(&registry_dir, Some(&cache_dir), crate_name, version);
    assert!(resolved.is_some(), "resolve_source_dir should find cached download");
    assert!(resolved.unwrap().is_dir());

    Ok(())
}

/// Verifies that `read_source_file_from_disk_or_cache` can read a specific
/// file from a downloaded and extracted crate source.
#[tokio::test]
async fn live_read_source_file_from_downloaded_cache() -> Result<()> {
    let cache_dir = make_temp_dir("read-cache");
    let _cleanup = CleanupDir(cache_dir.clone());
    let registry_dir = make_temp_dir("read-registry");
    let _cleanup2 = CleanupDir(registry_dir.clone());

    let crate_name = "itoa";
    let version = "1.0.1";

    download_and_extract(crate_name, version, &cache_dir).await?;

    // read_source_file_from_disk_or_cache should return the file content.
    let content = read_source_file_from_disk_or_cache(
        &registry_dir,
        Some(&cache_dir),
        crate_name,
        version,
        "src/lib.rs",
    );
    assert!(content.is_some(), "should be able to read src/lib.rs from cache");
    let content = content.unwrap();
    assert!(!content.is_empty(), "src/lib.rs content should not be empty");

    // Cargo.toml should also be readable.
    let cargo = read_source_file_from_disk_or_cache(
        &registry_dir,
        Some(&cache_dir),
        crate_name,
        version,
        "Cargo.toml",
    );
    assert!(cargo.is_some(), "should be able to read Cargo.toml from cache");
    assert!(
        cargo
            .unwrap()
            .contains("itoa"),
        "Cargo.toml should reference itoa"
    );

    Ok(())
}

// ──────────────────────────────────────────────────────────────────────────────
// Full MCP tool-call tests (download-then-index)
// ──────────────────────────────────────────────────────────────────────────────

struct LiveSourceContext {
    mcp: LocalMcpHttpHarness,
    #[allow(dead_code)]
    state: AppState,
    _postgres: PostgresTestContainer,
    _registry_cleanup: CleanupDir,
    _cache_cleanup: CleanupDir,
    _worker: tokio::task::JoinHandle<()>,
}

/// Creates a live MCP context with an **empty** cargo registry and source
/// cache, forcing `source_read` to download from crates.io on demand.
async fn live_source_context() -> Result<LiveSourceContext> {
    let postgres = PostgresTestContainer::start().await?;

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let registry_dir = std::env::temp_dir().join(format!("rust-mcp-live-registry-{nanos}"));
    std::fs::create_dir_all(&registry_dir)?;
    let cache_dir = std::env::temp_dir().join(format!("rust-mcp-live-cache-{nanos}"));
    std::fs::create_dir_all(&cache_dir)?;

    let mut config = common::test_config(
        postgres
            .connection_string()
            .to_string(),
        registry_dir.clone(),
    );
    config.crate_source_cache_dir = cache_dir.clone();

    let state = AppState::connect(config.clone()).await?;
    state.run_migrations().await?;

    // Spawn the refresh worker so on-demand indexing jobs are picked up.
    let worker = tokio::spawn(run_refresh_worker_for_tests(state.clone()));

    let router = http::router(state.clone(), config, common::test_prometheus_handle());
    let mcp = LocalMcpHttpHarness::spawn(router).await?;
    mcp.wait_until_ready(Duration::from_secs(30))
        .await?;
    let _ = mcp
        .initialize("live-source-download")
        .await?;

    Ok(LiveSourceContext {
        mcp,
        state,
        _postgres: postgres,
        _registry_cleanup: CleanupDir(registry_dir),
        _cache_cleanup: CleanupDir(cache_dir),
        _worker: worker,
    })
}

/// Syncs `itoa` (a tiny crate) then calls `source_read` for `src/lib.rs`.
/// The cargo registry is empty, so `ensure_source_indexed` must download from
/// crates.io before indexing. This exercises the download-then-index ordering
/// fix.
#[tokio::test]
async fn live_source_read_downloads_and_indexes_on_demand() -> Result<()> {
    let ctx = live_source_context().await?;

    // First, sync itoa so the crate + version rows exist in the DB.
    let sync_response = ctx
        .mcp
        .call_tool(
            "index_crates",
            json!({
                "crates": [{ "name": "itoa" }]
            }),
        )
        .await
        .context("index_crates for itoa failed")?;
    let sync_payload = common::structured_content(&sync_response);
    assert!(
        sync_payload
            .get("succeeded")
            .and_then(Value::as_u64)
            .unwrap_or_default()
            >= 1,
        "itoa should be synced"
    );

    // Now call source_read. The source is NOT in the local registry, so the
    // server must download it from crates.io, index the source files, and
    // then serve the content.
    let read_response = ctx
        .mcp
        .call_tool(
            "source_read",
            json!({
                "crate_name": "itoa",
                "path": "src/lib.rs",
                "start_line": 1,
                "end_line": 10
            }),
        )
        .await
        .context("source_read for itoa src/lib.rs failed")?;
    let read_payload = common::structured_content(&read_response);

    assert_eq!(
        read_payload
            .get("crate_name")
            .and_then(Value::as_str),
        Some("itoa"),
    );
    assert_eq!(
        read_payload
            .get("path")
            .and_then(Value::as_str),
        Some("src/lib.rs"),
    );
    let content = read_payload
        .get("content")
        .and_then(Value::as_str)
        .expect("source_read should return content");
    assert!(!content.is_empty(), "content should not be empty after on-demand download");

    Ok(())
}
