use serde_json::Value;
use sqlx::{PgPool, Postgres, QueryBuilder};

use super::models::DocsSearchRow;

/// Executes a lightweight readiness probe against PostgreSQL.
pub async fn run_readiness_probe(db: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::query_scalar::<_, i64>("SELECT 1::BIGINT")
        .fetch_one(db)
        .await?;
    Ok(())
}

/// Loads a non-expired query cache value by key/source.
pub async fn fetch_query_cache_value(
    db: &PgPool,
    key: &str,
    source: &str,
) -> Result<Option<Value>, sqlx::Error> {
    sqlx::query_scalar::<_, Value>(
        "SELECT value
         FROM query_cache
         WHERE key = $1 AND source = $2 AND expires_at > NOW()
         LIMIT 1",
    )
    .bind(key)
    .bind(source)
    .fetch_optional(db)
    .await
}

/// Appends a query-cache hit/miss event for telemetry.
pub async fn insert_query_cache_event(
    db: &PgPool,
    source: &str,
    hit: bool,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO query_cache_events (source, hit, created_at)
         VALUES ($1, $2, NOW())",
    )
    .bind(source)
    .bind(hit)
    .execute(db)
    .await?;
    Ok(())
}

/// Upserts a query cache entry with the requested TTL.
pub async fn upsert_query_cache_value(
    db: &PgPool,
    key: &str,
    source: &str,
    value: &Value,
    ttl_seconds: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO query_cache (key, value, source, created_at, expires_at)
         VALUES ($1, $2, $3, NOW(), NOW() + ($4 * INTERVAL '1 second'))
         ON CONFLICT (key) DO UPDATE
         SET value = EXCLUDED.value,
             source = EXCLUDED.source,
             created_at = NOW(),
             expires_at = EXCLUDED.expires_at",
    )
    .bind(key)
    .bind(value)
    .bind(source)
    .bind(ttl_seconds.max(1))
    .execute(db)
    .await?;
    Ok(())
}

/// Records one tool invocation for operational metrics.
pub async fn insert_tool_invocation(
    db: &PgPool,
    tool_name: &str,
    success: bool,
    latency_ms: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO tool_invocations (tool_name, success, latency_ms, created_at)
         VALUES ($1, $2, $3, NOW())",
    )
    .bind(tool_name)
    .bind(success)
    .bind(latency_ms.max(0))
    .execute(db)
    .await?;
    Ok(())
}

/// Upserts a docs page and only writes when indexed content changes.
pub async fn upsert_docs_page_if_changed(
    db: &PgPool,
    crate_version_id: i64,
    path: &str,
    title: Option<&str>,
    content: &str,
    source_url: Option<&str>,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        "INSERT INTO docs_pages (
            crate_version_id,
            path,
            title,
            content,
            source_url,
            indexed_at
         ) VALUES (
            $1, $2, $3, $4, $5, NOW()
         )
         ON CONFLICT (crate_version_id, path) DO UPDATE
         SET title = EXCLUDED.title,
             content = EXCLUDED.content,
             source_url = EXCLUDED.source_url,
             indexed_at = NOW()
         WHERE docs_pages.title IS DISTINCT FROM EXCLUDED.title
            OR docs_pages.content IS DISTINCT FROM EXCLUDED.content
            OR docs_pages.source_url IS DISTINCT FROM EXCLUDED.source_url",
    )
    .bind(crate_version_id)
    .bind(path)
    .bind(title)
    .bind(content)
    .bind(source_url)
    .execute(db)
    .await?;
    Ok(result.rows_affected())
}

/// Queries docs pages for `docs.search` with optional crate/version/path
/// filters.
pub async fn search_docs_pages(
    db: &PgPool,
    crate_name: Option<&str>,
    version: Option<&str>,
    path_prefix: Option<&str>,
    query: &str,
    limit: i64,
) -> Result<Vec<DocsSearchRow>, sqlx::Error> {
    let mut qb = QueryBuilder::<Postgres>::new(
        "SELECT
            c.name AS crate_name,
            cv.version,
            dp.path,
            dp.title,
            dp.source_url,
            dp.indexed_at::TEXT AS indexed_at,
            dp.content
         FROM docs_pages dp
         JOIN crate_versions cv ON cv.id = dp.crate_version_id
         JOIN crates c ON c.id = cv.crate_id ",
    );

    let mut has_where = false;
    if let Some(crate_filter) = crate_name {
        qb.push(if has_where { "AND " } else { "WHERE " });
        has_where = true;
        qb.push("c.name = ");
        qb.push_bind(crate_filter);
        qb.push(' ');
    }
    if let Some(version_filter) = version {
        qb.push(if has_where { "AND " } else { "WHERE " });
        has_where = true;
        qb.push("cv.version = ");
        qb.push_bind(version_filter);
        qb.push(' ');
    }
    if let Some(path_filter) = path_prefix {
        qb.push(if has_where { "AND " } else { "WHERE " });
        has_where = true;
        qb.push("dp.path ILIKE ");
        qb.push_bind(format!("{path_filter}%"));
        qb.push(' ');
    }

    qb.push(if has_where { "AND " } else { "WHERE " });
    qb.push("(dp.content ILIKE ");
    qb.push_bind(format!("%{query}%"));
    qb.push(" OR COALESCE(dp.title, '') ILIKE ");
    qb.push_bind(format!("%{query}%"));
    qb.push(" OR dp.path ILIKE ");
    qb.push_bind(format!("%{query}%"));
    qb.push(") ");

    qb.push("ORDER BY dp.indexed_at DESC, c.name ASC, dp.path ASC LIMIT ");
    qb.push_bind(limit.max(1));

    qb.build_query_as::<DocsSearchRow>()
        .fetch_all(db)
        .await
}
