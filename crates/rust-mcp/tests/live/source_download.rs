//! Live tests for on-demand crate source downloading from static.crates.io.
//!
//! These tests perform **real HTTP downloads** to verify the URL scheme,
//! tarball format, and extraction pipeline work end-to-end.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use rust_mcp::mcp::utils::{read_source_file_from_disk_or_cache, resolve_source_dir};

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
