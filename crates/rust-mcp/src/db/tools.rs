use serde_json::Value;
use sqlx::{PgPool, Postgres, QueryBuilder};

use super::models::{
    CrateAdvisoryRow, CrateCoreRow, CrateDependencyRow, CrateDependentRow, CrateVersionHistoryRow,
    CrateVersionSelectionRow, DocsSearchRow, SourceContextImplLookupRow,
    SourceContextLineLookupRow, SourceContextTypeLookupRow, SourceReadRow, SymbolSearchRow,
};

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
    offset: i64,
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
    qb.push(" OFFSET ");
    qb.push_bind(offset.max(0));

    qb.build_query_as::<DocsSearchRow>()
        .fetch_all(db)
        .await
}

/// Loads canonical crate metadata by crate name.
pub async fn fetch_crate_core_by_name(
    db: &PgPool,
    crate_name: &str,
) -> Result<Option<CrateCoreRow>, sqlx::Error> {
    sqlx::query_as::<_, CrateCoreRow>(
        "SELECT
            id,
            name,
            description,
            repository_url,
            docs_url,
            homepage_url,
            categories,
            keywords,
            updated_at::TEXT AS updated_at
         FROM crates
         WHERE name = $1",
    )
    .bind(crate_name)
    .fetch_optional(db)
    .await
}

/// Loads the latest indexed crate version for a crate id.
pub async fn fetch_latest_crate_version(
    db: &PgPool,
    crate_id: i64,
) -> Result<Option<CrateVersionSelectionRow>, sqlx::Error> {
    sqlx::query_as::<_, CrateVersionSelectionRow>(
        "SELECT
            id,
            version,
            rust_version,
            published_at::TEXT AS published_at,
            readme
         FROM crate_versions
         WHERE crate_id = $1
         ORDER BY published_at DESC NULLS LAST, id DESC
         LIMIT 1",
    )
    .bind(crate_id)
    .fetch_optional(db)
    .await
}

/// Loads one specific indexed crate version by exact version string.
pub async fn fetch_crate_version_by_name(
    db: &PgPool,
    crate_id: i64,
    version: &str,
) -> Result<Option<CrateVersionSelectionRow>, sqlx::Error> {
    sqlx::query_as::<_, CrateVersionSelectionRow>(
        "SELECT
            id,
            version,
            rust_version,
            published_at::TEXT AS published_at,
            readme
         FROM crate_versions
         WHERE crate_id = $1 AND version = $2
         LIMIT 1",
    )
    .bind(crate_id)
    .bind(version)
    .fetch_optional(db)
    .await
}

/// Aggregates total downloads across all indexed versions of a crate.
pub async fn fetch_crate_total_downloads(db: &PgPool, crate_id: i64) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(SUM(total_downloads), 0)::BIGINT
         FROM crate_versions
         WHERE crate_id = $1",
    )
    .bind(crate_id)
    .fetch_one(db)
    .await
}

/// Loads the last published timestamp across indexed versions of a crate.
pub async fn fetch_crate_last_published_at(
    db: &PgPool,
    crate_id: i64,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar::<_, Option<String>>(
        "SELECT MAX(published_at)::TEXT
         FROM crate_versions
         WHERE crate_id = $1",
    )
    .bind(crate_id)
    .fetch_one(db)
    .await
}

/// Loads a limited newest-first version timeline for `crate.intel`.
pub async fn fetch_crate_version_history(
    db: &PgPool,
    crate_id: i64,
    limit: i64,
) -> Result<Vec<CrateVersionHistoryRow>, sqlx::Error> {
    sqlx::query_as::<_, CrateVersionHistoryRow>(
        "SELECT
            cv.version,
            cv.rust_version,
            cv.published_at::TEXT AS published_at,
            cv.yanked,
            cv.total_downloads,
            EXISTS(
                SELECT 1
                FROM advisory_matches am
                WHERE am.version_id = cv.id
            ) AS has_advisory
         FROM crate_versions cv
         WHERE cv.crate_id = $1
         ORDER BY cv.published_at DESC NULLS LAST, cv.id DESC
         LIMIT $2",
    )
    .bind(crate_id)
    .bind(limit.max(1))
    .fetch_all(db)
    .await
}

/// Loads dependency edges for one crate version.
pub async fn fetch_crate_dependencies_for_version(
    db: &PgPool,
    crate_version_id: i64,
) -> Result<Vec<CrateDependencyRow>, sqlx::Error> {
    sqlx::query_as::<_, CrateDependencyRow>(
        "SELECT
            c.name AS dependency_name,
            d.requirement,
            d.dependency_kind,
            d.optional,
            d.features
         FROM dependency_edges d
         JOIN crates c ON c.id = d.to_crate_id
         WHERE d.from_version_id = $1
         ORDER BY c.name ASC",
    )
    .bind(crate_version_id)
    .fetch_all(db)
    .await
}

/// Counts distinct crates depending on the specified crate.
pub async fn fetch_dependent_crate_count(db: &PgPool, crate_id: i64) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(COUNT(DISTINCT cv_from.crate_id), 0)::BIGINT
         FROM dependency_edges d
         JOIN crate_versions cv_from ON cv_from.id = d.from_version_id
         WHERE d.to_crate_id = $1",
    )
    .bind(crate_id)
    .fetch_one(db)
    .await
}

/// Loads a limited set of top dependents for one crate.
pub async fn fetch_dependent_crates(
    db: &PgPool,
    crate_id: i64,
    limit: i64,
) -> Result<Vec<CrateDependentRow>, sqlx::Error> {
    sqlx::query_as::<_, CrateDependentRow>(
        "WITH dependent_crates AS (
             SELECT DISTINCT cv_from.crate_id
             FROM dependency_edges d
             JOIN crate_versions cv_from ON cv_from.id = d.from_version_id
             WHERE d.to_crate_id = $1
         )
         SELECT
             c.name AS crate_name,
             lv.version AS latest_version,
             COALESCE(lv.total_downloads, 0)::BIGINT AS total_downloads
         FROM dependent_crates dc
         JOIN crates c ON c.id = dc.crate_id
         LEFT JOIN LATERAL (
             SELECT
                 cv.version,
                 cv.total_downloads
             FROM crate_versions cv
             WHERE cv.crate_id = c.id
             ORDER BY cv.published_at DESC NULLS LAST, cv.id DESC
             LIMIT 1
         ) lv ON true
         ORDER BY total_downloads DESC, c.name ASC
         LIMIT $2",
    )
    .bind(crate_id)
    .bind(limit.max(1))
    .fetch_all(db)
    .await
}

/// Loads advisories that match either a crate version or crate-level scope.
pub async fn fetch_crate_advisories_for_version(
    db: &PgPool,
    crate_id: i64,
    crate_version_id: i64,
) -> Result<Vec<CrateAdvisoryRow>, sqlx::Error> {
    sqlx::query_as::<_, CrateAdvisoryRow>(
        "SELECT
            advisory_id,
            title,
            severity,
            url,
            affected_range,
            fixed_versions,
            source
         FROM advisory_matches
         WHERE crate_id = $1 AND (version_id = $2 OR version_id IS NULL)
         ORDER BY advisory_id ASC",
    )
    .bind(crate_id)
    .bind(crate_version_id)
    .fetch_all(db)
    .await
}

/// Loads indexed source content for one crate version and path.
pub async fn fetch_source_read_for_crate_version_path(
    db: &PgPool,
    crate_name: &str,
    crate_version_id: i64,
    path: &str,
) -> Result<Option<SourceReadRow>, sqlx::Error> {
    sqlx::query_as::<_, SourceReadRow>(
        "SELECT
            c.name AS crate_name,
            cv.version,
            sf.path,
            sf.content
         FROM source_files sf
         JOIN crate_versions cv ON cv.id = sf.crate_version_id
         JOIN crates c ON c.id = cv.crate_id
         WHERE c.name = $1 AND cv.id = $2 AND sf.path = $3
         LIMIT 1",
    )
    .bind(crate_name)
    .bind(crate_version_id)
    .bind(path)
    .fetch_optional(db)
    .await
}

/// Loads the first matching symbol start line for `source.context`.
pub async fn fetch_symbol_start_line_for_context(
    db: &PgPool,
    crate_version_id: i64,
    path: &str,
    symbol_name: &str,
) -> Result<Option<SourceContextLineLookupRow>, sqlx::Error> {
    sqlx::query_as::<_, SourceContextLineLookupRow>(
        "SELECT s.start_line
         FROM symbols s
         JOIN source_files sf ON sf.id = s.source_file_id
         WHERE s.crate_version_id = $1
           AND sf.path = $2
           AND s.name = $3
         ORDER BY s.start_line ASC
         LIMIT 1",
    )
    .bind(crate_version_id)
    .bind(path)
    .bind(symbol_name)
    .fetch_optional(db)
    .await
}

/// Loads the nearest containing impl block for `source.context`.
pub async fn fetch_containing_impl_for_context(
    db: &PgPool,
    crate_version_id: i64,
    path: &str,
    line: i32,
) -> Result<Option<SourceContextImplLookupRow>, sqlx::Error> {
    sqlx::query_as::<_, SourceContextImplLookupRow>(
        "SELECT
            ci.type_name,
            ci.type_name_display,
            ci.trait_name,
            ci.trait_name_display,
            ci.impl_kind,
            ci.start_line
         FROM crate_impls ci
         JOIN source_files sf ON sf.id = ci.source_file_id
         WHERE ci.crate_version_id = $1
           AND sf.path = $2
           AND ci.start_line <= $3
         ORDER BY ci.start_line DESC
         LIMIT 1",
    )
    .bind(crate_version_id)
    .bind(path)
    .bind(line)
    .fetch_optional(db)
    .await
}

/// Loads nearby type definitions for `source.context`.
pub async fn fetch_surrounding_types_for_context(
    db: &PgPool,
    crate_version_id: i64,
    path: &str,
    line: i32,
    limit: i64,
) -> Result<Vec<SourceContextTypeLookupRow>, sqlx::Error> {
    sqlx::query_as::<_, SourceContextTypeLookupRow>(
        "SELECT
            ct.type_name,
            ct.kind,
            ct.start_line
         FROM crate_types ct
         JOIN source_files sf ON sf.id = ct.source_file_id
         WHERE ct.crate_version_id = $1
           AND sf.path = $2
         ORDER BY ABS(ct.start_line - $3), ct.start_line ASC
         LIMIT $4",
    )
    .bind(crate_version_id)
    .bind(path)
    .bind(line)
    .bind(limit.max(1))
    .fetch_all(db)
    .await
}

/// Symbol search filters used by `symbol.search` DB helpers.
#[derive(Debug, Clone, Copy)]
pub struct SymbolSearchFilters<'a> {
    /// Search term matched against symbol names.
    pub query: &'a str,
    /// Optional crate-name exact filter.
    pub crate_name: Option<&'a str>,
    /// Optional version exact filter.
    pub version: Option<&'a str>,
    /// Optional symbol-kind exact filter.
    pub kind: Option<&'a str>,
    /// Whether to include every crate version when no version filter is set.
    pub include_all_versions: bool,
    /// Whether to deduplicate rows by canonical symbol identity.
    pub collapse_by_canonical: bool,
}

fn push_symbol_search_where_clause<'a>(
    qb: &mut QueryBuilder<'a, Postgres>,
    filters: &SymbolSearchFilters<'a>,
) {
    let mut has_where = false;
    if let Some(crate_filter) = filters.crate_name {
        qb.push(if has_where { "AND " } else { "WHERE " });
        has_where = true;
        qb.push("c.name = ");
        qb.push_bind(crate_filter);
        qb.push(' ');
    }

    if let Some(version_filter) = filters.version {
        qb.push(if has_where { "AND " } else { "WHERE " });
        has_where = true;
        qb.push("cv.version = ");
        qb.push_bind(version_filter);
        qb.push(' ');
    }

    if let Some(kind_filter) = filters.kind {
        qb.push(if has_where { "AND " } else { "WHERE " });
        has_where = true;
        qb.push("s.kind = ");
        qb.push_bind(kind_filter);
        qb.push(' ');
    }

    qb.push(if has_where { "AND " } else { "WHERE " });
    has_where = true;
    qb.push("s.name ILIKE ");
    qb.push_bind(format!("%{}%", filters.query));
    qb.push(' ');

    if filters.version.is_none() && !filters.include_all_versions {
        qb.push(if has_where { "AND " } else { "WHERE " });
        qb.push(
            "cv.id = (
                SELECT cv2.id
                FROM crate_versions cv2
                WHERE cv2.crate_id = c.id
                ORDER BY cv2.published_at DESC NULLS LAST, cv2.id DESC
                LIMIT 1
             ) ",
        );
    }
}

/// Counts matching symbol rows for `symbol.search`.
pub async fn count_symbol_search_hits(
    db: &PgPool,
    filters: &SymbolSearchFilters<'_>,
) -> Result<i64, sqlx::Error> {
    let mut qb = QueryBuilder::<Postgres>::new(if filters.collapse_by_canonical {
        "SELECT COUNT(DISTINCT (c.name, sf.path, s.name, s.kind))::BIGINT
         FROM symbols s
         JOIN crate_versions cv ON cv.id = s.crate_version_id
         JOIN crates c ON c.id = cv.crate_id
         JOIN source_files sf ON sf.id = s.source_file_id "
    } else {
        "SELECT COUNT(*)::BIGINT
         FROM symbols s
         JOIN crate_versions cv ON cv.id = s.crate_version_id
         JOIN crates c ON c.id = cv.crate_id
         JOIN source_files sf ON sf.id = s.source_file_id "
    });

    push_symbol_search_where_clause(&mut qb, filters);

    qb.build_query_scalar::<i64>()
        .fetch_one(db)
        .await
}

/// Loads one page of symbol-search hits.
pub async fn search_symbol_hits(
    db: &PgPool,
    filters: &SymbolSearchFilters<'_>,
    limit: i64,
    offset: i64,
) -> Result<Vec<SymbolSearchRow>, sqlx::Error> {
    let mut qb = QueryBuilder::<Postgres>::new(if filters.collapse_by_canonical {
        "SELECT DISTINCT ON (c.name, sf.path, s.name, s.kind)
            s.id AS _symbol_id,
            c.name AS crate_name,
            cv.version,
            sf.path AS source_path,
            s.name,
            s.kind,
            s.signature,
            s.visibility,
            s.start_line,
            s.end_line,
            s.index_source,
            s.indexed_at::TEXT AS indexed_at
         FROM symbols s
         JOIN crate_versions cv ON cv.id = s.crate_version_id
         JOIN crates c ON c.id = cv.crate_id
         JOIN source_files sf ON sf.id = s.source_file_id "
    } else {
        "SELECT
            s.id AS _symbol_id,
            c.name AS crate_name,
            cv.version,
            sf.path AS source_path,
            s.name,
            s.kind,
            s.signature,
            s.visibility,
            s.start_line,
            s.end_line,
            s.index_source,
            s.indexed_at::TEXT AS indexed_at
         FROM symbols s
         JOIN crate_versions cv ON cv.id = s.crate_version_id
         JOIN crates c ON c.id = cv.crate_id
         JOIN source_files sf ON sf.id = s.source_file_id "
    });

    push_symbol_search_where_clause(&mut qb, filters);

    if filters.collapse_by_canonical {
        qb.push(
            "ORDER BY
            c.name ASC,
            sf.path ASC,
            s.name ASC,
            s.kind ASC,
            cv.published_at DESC NULLS LAST,
            cv.id DESC,
            s.start_line ASC,
            s.end_line ASC,
            s.id ASC
         LIMIT ",
        );
    } else {
        qb.push(
            "ORDER BY
            c.name ASC,
            cv.published_at DESC NULLS LAST,
            cv.id DESC,
            s.name ASC,
            s.kind ASC,
            sf.path ASC,
            s.start_line ASC,
            s.end_line ASC,
            s.id ASC
         LIMIT ",
        );
    }
    qb.push_bind(limit.max(1));
    qb.push(" OFFSET ");
    qb.push_bind(offset.max(0));

    qb.build_query_as::<SymbolSearchRow>()
        .fetch_all(db)
        .await
}
