//! Reusable database fixture helpers for rust-mcp integration tests.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result, bail};
use serde_json::json;
use sha2::{Digest as _, Sha256};
use sqlx::PgPool;

use crate::db;

/// Identifiers returned when seeding a crate and one of its versions.
#[derive(Debug, Clone, Copy)]
pub struct SeededCrateVersion {
    /// Inserted crate row identifier.
    pub crate_id: i64,
    /// Inserted crate version row identifier.
    pub version_id: i64,
}

/// Temporary directory containing one or more materialized rustdoc JSON files.
#[derive(Debug)]
pub struct RustdocFixtureDir {
    path: PathBuf,
}

impl RustdocFixtureDir {
    /// Returns the materialized fixture directory path.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for RustdocFixtureDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Resolves a workspace-root rustdoc fixture path.
pub fn workspace_rustdoc_fixture_path(file_name: &str) -> Result<PathBuf> {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let fixture_path = workspace_root
        .join("rustdoc-json")
        .join(file_name);
    if !fixture_path.exists() {
        bail!("rustdoc fixture `{}` does not exist at {}", file_name, fixture_path.display());
    }
    Ok(fixture_path)
}

/// Decompresses a `.json.zst` rustdoc fixture into a temporary directory using
/// the `<crate>-<version>.json` naming convention expected by the indexer.
pub fn materialize_workspace_rustdoc_fixture_zst(
    file_name: &str,
    crate_name: &str,
    version: &str,
) -> Result<RustdocFixtureDir> {
    let fixture_path = workspace_rustdoc_fixture_path(file_name)?;
    materialize_rustdoc_fixture_zst(&fixture_path, crate_name, version)
}

/// Decompresses a `.json.zst` rustdoc fixture into a temporary directory using
/// the `<crate>-<version>.json` naming convention expected by the indexer.
pub fn materialize_rustdoc_fixture_zst(
    fixture_path: &Path,
    crate_name: &str,
    version: &str,
) -> Result<RustdocFixtureDir> {
    let compressed = std::fs::read(fixture_path).with_context(|| {
        format!("failed to read compressed rustdoc fixture {}", fixture_path.display())
    })?;
    let decoded = zstd::stream::decode_all(Cursor::new(compressed)).with_context(|| {
        format!("failed to decompress zstd rustdoc fixture {}", fixture_path.display())
    })?;

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("rust-mcp-rustdoc-zst-{nanos}"));
    std::fs::create_dir_all(&dir).with_context(|| {
        format!("failed to create materialized rustdoc fixture directory {}", dir.display())
    })?;

    let output_path = dir.join(format!("{crate_name}-{version}.json"));
    std::fs::write(&output_path, decoded).with_context(|| {
        format!("failed to write materialized rustdoc fixture {}", output_path.display())
    })?;

    Ok(RustdocFixtureDir { path: dir })
}

/// Inserts or updates a crate row with default metadata and returns the crate
/// id.
pub async fn seed_crate(pool: &PgPool, name: &str) -> Result<i64> {
    seed_crate_with_metadata(pool, name, None, &[], &[]).await
}

/// Inserts or updates a crate row with metadata and returns the crate id.
pub async fn seed_crate_with_metadata(
    pool: &PgPool,
    name: &str,
    description: Option<&str>,
    categories: &[&str],
    keywords: &[&str],
) -> Result<i64> {
    let categories = categories
        .iter()
        .map(|value| (*value).to_string())
        .collect::<Vec<_>>();
    let keywords = keywords
        .iter()
        .map(|value| (*value).to_string())
        .collect::<Vec<_>>();

    let crate_id =
        db::upsert_crate_with_metadata(pool, name, description, categories, keywords).await?;

    Ok(crate_id)
}

/// Inserts or updates a crate version row and returns its id.
pub async fn seed_crate_version(
    pool: &PgPool,
    crate_id: i64,
    version: &str,
    total_downloads: i64,
    published_at: Option<&str>,
) -> Result<i64> {
    let checksum_input = format!("{crate_id}:{version}");
    let checksum = format!("{:x}", Sha256::digest(checksum_input.as_bytes()));

    let version_id =
        db::upsert_crate_version(pool, crate_id, version, total_downloads, published_at, checksum)
            .await?;

    Ok(version_id)
}

/// Inserts or updates a crate and one version in one call.
pub async fn seed_crate_release(
    pool: &PgPool,
    crate_name: &str,
    version: &str,
    total_downloads: i64,
    published_at: Option<&str>,
) -> Result<SeededCrateVersion> {
    let crate_id = seed_crate(pool, crate_name).await?;
    let version_id =
        seed_crate_version(pool, crate_id, version, total_downloads, published_at).await?;

    Ok(SeededCrateVersion { crate_id, version_id })
}

/// Inserts a dependency edge and returns its id.
pub async fn seed_dependency_edge(
    pool: &PgPool,
    from_version_id: i64,
    to_crate_id: i64,
    requirement: &str,
    dependency_kind: &str,
    optional: bool,
) -> Result<i64> {
    let edge_id = db::insert_dependency_edge(
        pool,
        from_version_id,
        to_crate_id,
        requirement,
        dependency_kind,
        optional,
    )
    .await?;

    Ok(edge_id)
}

/// Inserts or updates a crate feature-flag row and returns its id.
pub async fn seed_feature_flag(
    pool: &PgPool,
    crate_version_id: i64,
    feature_name: &str,
    enables: &[&str],
) -> Result<i64> {
    let enables = json!(enables);

    let feature_id = db::upsert_feature_flag(pool, crate_version_id, feature_name, enables).await?;

    Ok(feature_id)
}

/// Inserts or updates a source file row and returns its id.
pub async fn seed_source_file(
    pool: &PgPool,
    crate_version_id: i64,
    path: &str,
    language: Option<&str>,
    content: &str,
) -> Result<i64> {
    let sha256 = format!("{:x}", Sha256::digest(content.as_bytes()));
    let file_size = i64::try_from(content.len())?;

    let source_file_id =
        db::upsert_source_file(pool, crate_version_id, path, sha256, file_size, language, content)
            .await?;

    Ok(source_file_id)
}

/// Inserts a symbol row and returns its id.
pub async fn seed_symbol(
    pool: &PgPool,
    crate_version_id: i64,
    source_file_id: i64,
    name: &str,
    kind: &str,
    start_line: i32,
    end_line: i32,
) -> Result<i64> {
    let symbol_id =
        db::insert_symbol(pool, crate_version_id, source_file_id, name, kind, start_line, end_line)
            .await?;

    Ok(symbol_id)
}

/// Inserts or updates a docs page row and returns its id.
pub async fn seed_docs_page(
    pool: &PgPool,
    crate_version_id: i64,
    path: &str,
    title: Option<&str>,
    content: &str,
) -> Result<i64> {
    let docs_page_id = db::upsert_docs_page(pool, crate_version_id, path, title, content).await?;

    Ok(docs_page_id)
}

/// IDs returned by `seed_minimal_crate_graph`.
#[derive(Debug, Clone, Copy)]
pub struct MinimalCrateGraphFixture {
    /// Dependent crate + version IDs.
    pub dependent: SeededCrateVersion,
    /// Dependency crate + version IDs.
    pub dependency: SeededCrateVersion,
    /// Optional dependency crate + version IDs.
    pub optional_dependency: SeededCrateVersion,
    /// Inserted dependency edge ID.
    pub dependency_edge_id: i64,
    /// Inserted optional dependency edge ID.
    pub optional_dependency_edge_id: i64,
    /// Inserted source file ID.
    pub source_file_id: i64,
    /// Inserted symbol ID.
    pub symbol_id: i64,
    /// Inserted docs page ID.
    pub docs_page_id: i64,
}

/// Seeds a compact two-crate graph with one dependency edge, source file,
/// symbol, docs page, and one feature flag.
pub async fn seed_minimal_crate_graph(pool: &PgPool) -> Result<MinimalCrateGraphFixture> {
    let dependency =
        seed_crate_release(pool, "serde", "1.0.228", 1_200_000_000, Some("2026-01-01T00:00:00Z"))
            .await?;
    let dependent = seed_crate_release(
        pool,
        "serde_json",
        "1.0.145",
        600_000_000,
        Some("2026-01-02T00:00:00Z"),
    )
    .await?;
    let optional_dependency =
        seed_crate_release(pool, "indexmap", "2.8.0", 250_000_000, Some("2026-01-03T00:00:00Z"))
            .await?;

    let dependency_edge_id = seed_dependency_edge(
        pool,
        dependent.version_id,
        dependency.crate_id,
        "^1.0",
        "normal",
        false,
    )
    .await?;

    let optional_dependency_edge_id = seed_dependency_edge(
        pool,
        dependent.version_id,
        optional_dependency.crate_id,
        "^2",
        "normal",
        true,
    )
    .await?;

    seed_feature_flag(pool, dependent.version_id, "default", &["preserve_order"]).await?;
    seed_feature_flag(pool, dependent.version_id, "preserve_order", &["dep:indexmap"]).await?;

    let source_file_id = seed_source_file(
        pool,
        dependent.version_id,
        "src/lib.rs",
        Some("rust"),
        "pub fn from_str<T>() -> Result<T, Error> { todo!() }",
    )
    .await?;

    let symbol_id =
        seed_symbol(pool, dependent.version_id, source_file_id, "from_str", "function", 1, 1)
            .await?;

    let docs_page_id = seed_docs_page(
        pool,
        dependent.version_id,
        "/serde_json/fn.from_str.html",
        Some("serde_json::from_str"),
        "Deserialize an instance of type `T` from a string of JSON text.",
    )
    .await?;

    Ok(MinimalCrateGraphFixture {
        dependent,
        dependency,
        optional_dependency,
        dependency_edge_id,
        optional_dependency_edge_id,
        source_file_id,
        symbol_id,
        docs_page_id,
    })
}
