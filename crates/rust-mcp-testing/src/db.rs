//! Database helper functions for setting up test data in the Rust MCP testing
//! suite.

use serde_json::Value;
use sqlx::PgPool;

pub(crate) async fn upsert_crate_with_metadata(
    pool: &PgPool,
    name: &str,
    description: Option<&str>,
    categories: Vec<String>,
    keywords: Vec<String>,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar::<_, i64>(
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
    .await
}

pub(crate) async fn upsert_crate_version(
    pool: &PgPool,
    crate_id: i64,
    version: &str,
    total_downloads: i64,
    published_at: Option<&str>,
    checksum: String,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar::<_, i64>(
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
    .await
}

pub(crate) async fn insert_dependency_edge(
    pool: &PgPool,
    from_version_id: i64,
    to_crate_id: i64,
    requirement: &str,
    dependency_kind: &str,
    optional: bool,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar::<_, i64>(
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
    .bind(serde_json::json!([]))
    .fetch_one(pool)
    .await
}

pub(crate) async fn upsert_feature_flag(
    pool: &PgPool,
    crate_version_id: i64,
    feature_name: &str,
    enables: Value,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar::<_, i64>(
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
    .await
}

pub(crate) async fn upsert_source_file(
    pool: &PgPool,
    crate_version_id: i64,
    path: &str,
    sha256: String,
    file_size: i64,
    language: Option<&str>,
    content: &str,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar::<_, i64>(
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
    .await
}

pub(crate) async fn insert_symbol(
    pool: &PgPool,
    crate_version_id: i64,
    source_file_id: i64,
    name: &str,
    kind: &str,
    start_line: i32,
    end_line: i32,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar::<_, i64>(
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
    .await
}

pub(crate) async fn upsert_docs_page(
    pool: &PgPool,
    crate_version_id: i64,
    path: &str,
    title: Option<&str>,
    content: &str,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar::<_, i64>(
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
    .await
}
