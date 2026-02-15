//! Reusable database fixture helpers for rust-mcp integration tests.

use anyhow::Result;
use serde_json::json;
use sha2::{Digest as _, Sha256};
use sqlx::PgPool;

/// Identifiers returned when seeding a crate and one of its versions.
#[derive(Debug, Clone, Copy)]
pub struct SeededCrateVersion {
    /// Inserted crate row identifier.
    pub crate_id: i64,
    /// Inserted crate version row identifier.
    pub version_id: i64,
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

    let crate_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO crates (name, description, categories, keywords)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (name) DO UPDATE SET
           description = COALESCE(EXCLUDED.description, crates.description),
           categories = EXCLUDED.categories,
           keywords = EXCLUDED.keywords,
           updated_at = NOW()
         RETURNING id",
    )
    .bind(name)
    .bind(description)
    .bind(categories)
    .bind(keywords)
    .fetch_one(pool)
    .await?;

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

    let version_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO crate_versions (
             crate_id,
             version,
             published_at,
             yanked,
             total_downloads,
             checksum
         )
         VALUES ($1, $2, $3::TIMESTAMPTZ, FALSE, $4, $5)
         ON CONFLICT (crate_id, version) DO UPDATE SET
           published_at = EXCLUDED.published_at,
           total_downloads = EXCLUDED.total_downloads,
           checksum = EXCLUDED.checksum,
           updated_at = NOW()
         RETURNING id",
    )
    .bind(crate_id)
    .bind(version)
    .bind(published_at)
    .bind(total_downloads)
    .bind(checksum)
    .fetch_one(pool)
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
    let edge_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO dependency_edges (
             from_version_id,
             to_crate_id,
             requirement,
             dependency_kind,
             optional,
             features
         )
         VALUES ($1, $2, $3, $4, $5, $6::JSONB)
         RETURNING id",
    )
    .bind(from_version_id)
    .bind(to_crate_id)
    .bind(requirement)
    .bind(dependency_kind)
    .bind(optional)
    .bind(json!([]))
    .fetch_one(pool)
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

    let feature_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO crate_version_features (crate_version_id, feature_name, enables)
         VALUES ($1, $2, $3::JSONB)
         ON CONFLICT (crate_version_id, feature_name) DO UPDATE SET
           enables = EXCLUDED.enables,
           updated_at = NOW()
         RETURNING id",
    )
    .bind(crate_version_id)
    .bind(feature_name)
    .bind(enables)
    .fetch_one(pool)
    .await?;

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

    let source_file_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO source_files (
             crate_version_id,
             path,
             sha256,
             file_size,
             language,
             content
         )
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (crate_version_id, path) DO UPDATE SET
           sha256 = EXCLUDED.sha256,
           file_size = EXCLUDED.file_size,
           language = EXCLUDED.language,
           content = EXCLUDED.content,
           indexed_at = NOW()
         RETURNING id",
    )
    .bind(crate_version_id)
    .bind(path)
    .bind(sha256)
    .bind(file_size)
    .bind(language)
    .bind(content)
    .fetch_one(pool)
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
    let symbol_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO symbols (
             crate_version_id,
             source_file_id,
             name,
             kind,
             visibility,
             start_line,
             end_line,
             index_source
         )
         VALUES ($1, $2, $3, $4, 'public', $5, $6, 'fixture')
         RETURNING id",
    )
    .bind(crate_version_id)
    .bind(source_file_id)
    .bind(name)
    .bind(kind)
    .bind(start_line)
    .bind(end_line)
    .fetch_one(pool)
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
    let docs_page_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO docs_pages (crate_version_id, path, title, content)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (crate_version_id, path) DO UPDATE SET
           title = EXCLUDED.title,
           content = EXCLUDED.content,
           indexed_at = NOW()
         RETURNING id",
    )
    .bind(crate_version_id)
    .bind(path)
    .bind(title)
    .bind(content)
    .fetch_one(pool)
    .await?;

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
