use rust_mcp_types::types::krate::CrateSearchSort;
use rust_mcp_types::types::source::SourceSearchMode;
use serde_json::Value;
use sqlx::{PgPool, Postgres, QueryBuilder};

use super::models::{
    AlternativesCandidateRow, ApiDiffSymbolRow, ApiSurfaceRow, CrateAdvisoryRow,
    CrateCompareCountsRow, CrateCompareVersionRow, CrateCoreRow, CrateDependencyRow,
    CrateDependentRow, CrateTypeInfoRow, CrateUsageSourceRow, CrateVersionHistoryRow,
    CrateVersionLicenseRow, CrateVersionSelectionRow, CrateVersionTimelineRow, DependencyCrateRow,
    DependencyResolveEdgeRow, DependencyVersionRow, DeprecatedItemRow, DocsSearchRow,
    ErrorTypeImplRow, ErrorTypeReturnRow, ErrorTypeTypeRow, FeatureImpactDependencyRow,
    FeatureImpactFeatureRow, GraphDependencyTraversalRow, GraphDependentTraversalRow,
    GraphLatestVersionRow, HotspotSourceFileRow, ImportPathRow, ReExportSourceRow,
    ReExportSymbolKindRow, RustdocReExportRow, SourceContextImplLookupRow,
    SourceContextLineLookupRow, SourceContextTypeLookupRow, SourceReadRow, SourceSearchRow,
    SymbolSearchRow,
};

/// Executes a lightweight readiness probe against PostgreSQL.
pub async fn run_readiness_probe(db: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::query_scalar::<_, i64>("SELECT 1::BIGINT")
        .fetch_one(db)
        .await?;
    Ok(())
}

/// Returns true if at least one source file is indexed for the given crate
/// version.
pub async fn has_source_files_for_version(
    db: &PgPool,
    crate_version_id: i64,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM source_files WHERE crate_version_id = $1)",
    )
    .bind(crate_version_id)
    .fetch_one(db)
    .await
}

/// Returns true if at least one rustdoc-backed symbol is indexed for the given
/// crate version.
pub async fn has_rustdoc_symbols_for_version(
    db: &PgPool,
    crate_version_id: i64,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM symbols WHERE crate_version_id = $1 AND index_source = \
         'rustdoc_json')",
    )
    .bind(crate_version_id)
    .fetch_one(db)
    .await
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

/// Deletes one crate row by id (used for test fixture cleanup).
pub async fn delete_crate_by_id(db: &PgPool, crate_id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM crates WHERE id = $1")
        .bind(crate_id)
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
#[derive(Debug, Clone, Copy)]
pub struct DocsSearchParams<'a> {
    /// Optional exact crate-name filter.
    pub crate_name: Option<&'a str>,
    /// Optional exact crate-version filter.
    pub version: Option<&'a str>,
    /// Optional docs path prefix filter.
    pub path_prefix: Option<&'a str>,
    /// Search term matched against content/title/path.
    pub query: &'a str,
    /// Maximum rows to return.
    pub limit: i64,
    /// Zero-based offset for pagination.
    pub offset: i64,
}

/// Queries docs pages for `docs.search` with optional crate/version/path
/// filters.
pub async fn search_docs_pages(
    db: &PgPool,
    params: &DocsSearchParams<'_>,
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
    if let Some(crate_filter) = params.crate_name {
        qb.push(if has_where { "AND " } else { "WHERE " });
        has_where = true;
        qb.push("c.name = ");
        qb.push_bind(crate_filter);
        qb.push(' ');
    }
    if let Some(version_filter) = params.version {
        qb.push(if has_where { "AND " } else { "WHERE " });
        has_where = true;
        qb.push("cv.version = ");
        qb.push_bind(version_filter);
        qb.push(' ');
    }
    if let Some(path_filter) = params.path_prefix {
        qb.push(if has_where { "AND " } else { "WHERE " });
        has_where = true;
        qb.push("dp.path ILIKE ");
        qb.push_bind(format!("{path_filter}%"));
        qb.push(' ');
    }

    qb.push(if has_where { "AND " } else { "WHERE " });
    qb.push("(dp.content ILIKE ");
    qb.push_bind(format!("%{}%", params.query));
    qb.push(" OR COALESCE(dp.title, '') ILIKE ");
    qb.push_bind(format!("%{}%", params.query));
    qb.push(" OR dp.path ILIKE ");
    qb.push_bind(format!("%{}%", params.query));
    qb.push(") ");

    qb.push("ORDER BY dp.indexed_at DESC, c.name ASC, dp.path ASC LIMIT ");
    qb.push_bind(params.limit.max(1));
    qb.push(" OFFSET ");
    qb.push_bind(params.offset.max(0));

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

/// Loads indexed source content for the latest crate version and one path.
pub async fn fetch_source_read_latest_for_crate_path(
    db: &PgPool,
    crate_name: &str,
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
         WHERE c.name = $1 AND sf.path = $2
         ORDER BY cv.published_at DESC NULLS LAST, cv.id DESC
         LIMIT 1",
    )
    .bind(crate_name)
    .bind(path)
    .fetch_optional(db)
    .await
}

/// Counts public rustdoc_json symbols for one crate version.
pub async fn count_rustdoc_public_symbols_for_version(
    db: &PgPool,
    crate_version_id: i64,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*)::BIGINT
         FROM symbols
         WHERE crate_version_id = $1
           AND visibility = 'public'
           AND index_source = 'rustdoc_json'",
    )
    .bind(crate_version_id)
    .fetch_one(db)
    .await
}

/// Loads one page of crate API symbols with optional filters.
#[derive(Debug, Clone, Copy)]
pub struct ApiSurfaceListParams<'a> {
    /// Crate version id to query against.
    pub crate_version_id: i64,
    /// Optional symbol-kind filter list.
    pub kinds: Option<&'a [String]>,
    /// Optional preferred index source (for example `rustdoc_json`).
    pub preferred_source: Option<&'a str>,
    /// Optional source-file path filter using `ILIKE` semantics.
    pub path_like: Option<&'a str>,
    /// Maximum rows to return.
    pub limit: i64,
    /// Zero-based offset for pagination.
    pub offset: i64,
}

/// Loads one page of crate API symbols with optional filters.
pub async fn list_public_api_symbols_for_version(
    db: &PgPool,
    params: &ApiSurfaceListParams<'_>,
) -> Result<Vec<ApiSurfaceRow>, sqlx::Error> {
    if let Some(path_like) = params.path_like {
        sqlx::query_as::<_, ApiSurfaceRow>(
            "SELECT
                s.name,
                s.kind,
                s.signature,
                s.visibility,
                sf.path AS source_path,
                s.start_line,
                s.end_line,
                s.index_source
             FROM symbols s
             JOIN source_files sf ON sf.id = s.source_file_id
             WHERE s.crate_version_id = $1
               AND s.visibility = 'public'
               AND ($2::TEXT[] IS NULL OR s.kind = ANY($2))
               AND ($3::TEXT IS NULL OR s.index_source = $3)
               AND sf.path ILIKE $4 ESCAPE '\\'
             ORDER BY
                s.kind ASC,
                s.name ASC,
                sf.path ASC,
                s.start_line ASC,
                s.end_line ASC
             LIMIT $5
             OFFSET $6",
        )
        .bind(params.crate_version_id)
        .bind(params.kinds)
        .bind(params.preferred_source)
        .bind(path_like)
        .bind(params.limit.max(1))
        .bind(params.offset.max(0))
        .fetch_all(db)
        .await
    } else {
        sqlx::query_as::<_, ApiSurfaceRow>(
            "SELECT
                s.name,
                s.kind,
                s.signature,
                s.visibility,
                sf.path AS source_path,
                s.start_line,
                s.end_line,
                s.index_source
             FROM symbols s
             JOIN source_files sf ON sf.id = s.source_file_id
             WHERE s.crate_version_id = $1
               AND s.visibility = 'public'
               AND ($2::TEXT[] IS NULL OR s.kind = ANY($2))
               AND ($3::TEXT IS NULL OR s.index_source = $3)
             ORDER BY
                s.kind ASC,
                s.name ASC,
                sf.path ASC,
                s.start_line ASC,
                s.end_line ASC
             LIMIT $4
             OFFSET $5",
        )
        .bind(params.crate_version_id)
        .bind(params.kinds)
        .bind(params.preferred_source)
        .bind(params.limit.max(1))
        .bind(params.offset.max(0))
        .fetch_all(db)
        .await
    }
}

/// Loads import-path candidates for a symbol search term.
pub async fn list_import_path_matches(
    db: &PgPool,
    crate_version_id: i64,
    symbol_name: &str,
    kind: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<Vec<ImportPathRow>, sqlx::Error> {
    sqlx::query_as::<_, ImportPathRow>(
        "SELECT
            s.name AS symbol_name,
            s.kind,
            s.canonical_path,
            s.definition_path,
            sf.path AS source_path,
            s.start_line,
            s.end_line,
            s.index_source
         FROM symbols s
         JOIN source_files sf ON sf.id = s.source_file_id
         WHERE s.crate_version_id = $1
           AND (s.visibility = 'public' OR s.visibility IS NULL)
           AND (
                LOWER(s.name) = LOWER($2)
                OR LOWER(COALESCE(s.canonical_path, '')) = LOWER($2)
                OR LOWER(COALESCE(s.canonical_path, '')) LIKE ('%::' || LOWER($2))
           )
           AND ($3::TEXT IS NULL OR LOWER(s.kind) = LOWER($3))
         ORDER BY
            CASE
                WHEN LOWER(s.name) = LOWER($2) THEN 0
                WHEN LOWER(COALESCE(s.canonical_path, '')) = LOWER($2) THEN 1
                WHEN LOWER(COALESCE(s.canonical_path, '')) LIKE ('%::' || LOWER($2)) THEN 2
                ELSE 3
            END,
            CASE WHEN s.canonical_path IS NULL THEN 1 ELSE 0 END,
            CASE WHEN s.index_source = 'rustdoc_json' THEN 0 ELSE 1 END,
            s.start_line ASC,
            s.id ASC
         LIMIT $4
         OFFSET $5",
    )
    .bind(crate_version_id)
    .bind(symbol_name)
    .bind(kind)
    .bind(limit.max(1))
    .bind(offset.max(0))
    .fetch_all(db)
    .await
}

/// Lists candidate error-like type declarations for a crate version.
pub async fn list_error_type_rows(
    db: &PgPool,
    crate_version_id: i64,
) -> Result<Vec<ErrorTypeTypeRow>, sqlx::Error> {
    sqlx::query_as::<_, ErrorTypeTypeRow>(
        "SELECT
            ct.type_name,
            ct.kind,
            ct.fields,
            ct.variants,
            sf.path AS source_path,
            ct.start_line
         FROM crate_types ct
         JOIN source_files sf ON sf.id = ct.source_file_id
         WHERE ct.crate_version_id = $1
         ORDER BY ct.type_name ASC",
    )
    .bind(crate_version_id)
    .fetch_all(db)
    .await
}

/// Lists impl blocks for a crate version used in error-type heuristics.
pub async fn list_error_impl_rows(
    db: &PgPool,
    crate_version_id: i64,
) -> Result<Vec<ErrorTypeImplRow>, sqlx::Error> {
    sqlx::query_as::<_, ErrorTypeImplRow>(
        "SELECT
            ci.type_name,
            ci.trait_name,
            ci.trait_name_display,
            sf.path AS source_path
         FROM crate_impls ci
         JOIN source_files sf ON sf.id = ci.source_file_id
         WHERE ci.crate_version_id = $1",
    )
    .bind(crate_version_id)
    .fetch_all(db)
    .await
}

/// Lists function signatures returning `Result` variants matching a type hint.
pub async fn list_error_return_rows(
    db: &PgPool,
    crate_version_id: i64,
    type_name_like: &str,
) -> Result<Vec<ErrorTypeReturnRow>, sqlx::Error> {
    sqlx::query_as::<_, ErrorTypeReturnRow>(
        "SELECT
            s.name,
            s.signature
         FROM symbols s
         WHERE s.crate_version_id = $1
           AND s.signature IS NOT NULL
           AND s.signature ILIKE '%Result<%'
           AND s.signature ILIKE $2
         ORDER BY s.name ASC
         LIMIT 50",
    )
    .bind(crate_version_id)
    .bind(type_name_like)
    .fetch_all(db)
    .await
}

/// Loads raw source content for one crate version and file path.
pub async fn fetch_source_content_for_version_path(
    db: &PgPool,
    crate_version_id: i64,
    path: &str,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar::<_, String>(
        "SELECT content
         FROM source_files
         WHERE crate_version_id = $1 AND path = $2
         LIMIT 1",
    )
    .bind(crate_version_id)
    .bind(path)
    .fetch_optional(db)
    .await
}

/// Lists rustdoc-derived public re-export symbol rows for a crate version.
pub async fn list_rustdoc_re_exports(
    db: &PgPool,
    crate_version_id: i64,
    path_prefix: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<Vec<RustdocReExportRow>, sqlx::Error> {
    sqlx::query_as::<_, RustdocReExportRow>(
        "SELECT
            s.canonical_path,
            s.definition_path AS original_definition_path,
            s.kind,
            s.visibility,
            sf.path AS source_path,
            s.start_line AS source_line
         FROM symbols s
         JOIN source_files sf ON sf.id = s.source_file_id
         WHERE s.crate_version_id = $1
           AND s.index_source = 'rustdoc_json'
           AND s.visibility = 'public'
           AND s.canonical_path IS NOT NULL
           AND s.definition_path IS NOT NULL
           AND s.canonical_path <> s.definition_path
           AND ($2::TEXT IS NULL OR s.canonical_path LIKE ($2 || '%'))
         ORDER BY
            s.canonical_path ASC,
            s.definition_path ASC,
            s.kind ASC,
            s.start_line ASC
         LIMIT $3
         OFFSET $4",
    )
    .bind(crate_version_id)
    .bind(path_prefix)
    .bind(limit.max(1))
    .bind(offset.max(0))
    .fetch_all(db)
    .await
}

/// Lists likely root module source files used to infer manual re-exports.
pub async fn list_re_export_sources_for_version(
    db: &PgPool,
    crate_version_id: i64,
) -> Result<Vec<ReExportSourceRow>, sqlx::Error> {
    sqlx::query_as::<_, ReExportSourceRow>(
        "SELECT path, content
         FROM source_files
         WHERE crate_version_id = $1
           AND (path = 'src/lib.rs' OR path LIKE 'src/%/mod.rs')
         ORDER BY CASE WHEN path = 'src/lib.rs' THEN 0 ELSE 1 END, path ASC",
    )
    .bind(crate_version_id)
    .fetch_all(db)
    .await
}

/// Loads a symbol kind/visibility hint for re-export classification.
pub async fn fetch_re_export_symbol_kind(
    db: &PgPool,
    crate_version_id: i64,
    symbol_name: &str,
) -> Result<Option<ReExportSymbolKindRow>, sqlx::Error> {
    sqlx::query_as::<_, ReExportSymbolKindRow>(
        "SELECT
            s.kind,
            s.visibility
         FROM symbols s
         WHERE s.crate_version_id = $1
           AND LOWER(s.name) = LOWER($2)
         ORDER BY CASE WHEN s.visibility = 'public' THEN 0 ELSE 1 END, s.start_line ASC
         LIMIT 1",
    )
    .bind(crate_version_id)
    .bind(symbol_name)
    .fetch_optional(db)
    .await
}

/// Lists dependent source files that mention a requested crate symbol pattern.
pub async fn list_crate_usage_sources(
    db: &PgPool,
    crate_id: i64,
    symbol_filter: &str,
    limit: i64,
    offset: i64,
) -> Result<Vec<CrateUsageSourceRow>, sqlx::Error> {
    sqlx::query_as::<_, CrateUsageSourceRow>(
        "WITH dependents AS (
            SELECT DISTINCT ON (dc.id)
                dc.id AS dependent_crate_id,
                dc.name AS dependent_crate_name,
                dcv.id AS dependent_version_id,
                dcv.version AS dependent_version,
                dcv.total_downloads AS dependent_downloads
            FROM dependency_edges de
            JOIN crate_versions dcv ON dcv.id = de.from_version_id
            JOIN crates dc ON dc.id = dcv.crate_id
            WHERE de.to_crate_id = $1
            ORDER BY dc.id, dcv.published_at DESC NULLS LAST, dcv.id DESC
        )
        SELECT
            d.dependent_crate_name AS dependent_crate,
            d.dependent_version,
            d.dependent_downloads,
            sf.path,
            sf.content
        FROM dependents d
        JOIN source_files sf ON sf.crate_version_id = d.dependent_version_id
        WHERE sf.content ILIKE $2
          AND (sf.language IS NULL OR sf.language != 'rustdoc_json')
        ORDER BY d.dependent_downloads DESC, d.dependent_crate_name ASC, sf.path ASC
        LIMIT $3
        OFFSET $4",
    )
    .bind(crate_id)
    .bind(symbol_filter)
    .bind(limit.max(1))
    .bind(offset.max(0))
    .fetch_all(db)
    .await
}

/// Loads license metadata from the latest indexed version of a crate.
pub async fn fetch_latest_crate_license_for_crate(
    db: &PgPool,
    crate_id: i64,
) -> Result<Option<CrateVersionLicenseRow>, sqlx::Error> {
    sqlx::query_as::<_, CrateVersionLicenseRow>(
        "SELECT
            version,
            license_expression
         FROM crate_versions
         WHERE crate_id = $1
         ORDER BY published_at DESC NULLS LAST, id DESC
         LIMIT 1",
    )
    .bind(crate_id)
    .fetch_optional(db)
    .await
}

/// Loads license metadata for a specific crate version.
pub async fn fetch_crate_license_for_version(
    db: &PgPool,
    crate_id: i64,
    version: &str,
) -> Result<Option<CrateVersionLicenseRow>, sqlx::Error> {
    sqlx::query_as::<_, CrateVersionLicenseRow>(
        "SELECT
            version,
            license_expression
         FROM crate_versions
         WHERE crate_id = $1 AND version = $2
         LIMIT 1",
    )
    .bind(crate_id)
    .bind(version)
    .fetch_optional(db)
    .await
}

/// Lists source files used for hotspot analysis, optionally filtered by path.
pub async fn list_source_files_for_hotspots(
    db: &PgPool,
    crate_version_id: i64,
    path_like: Option<&str>,
) -> Result<Vec<HotspotSourceFileRow>, sqlx::Error> {
    if let Some(path_like) = path_like {
        sqlx::query_as::<_, HotspotSourceFileRow>(
            "SELECT
                sf.path,
                sf.content
             FROM source_files sf
             WHERE sf.crate_version_id = $1
               AND sf.path ILIKE $2 ESCAPE '\\\\'
               AND (sf.language IS NULL OR sf.language != 'rustdoc_json')
             ORDER BY sf.path ASC
             LIMIT 1000",
        )
        .bind(crate_version_id)
        .bind(path_like)
        .fetch_all(db)
        .await
    } else {
        sqlx::query_as::<_, HotspotSourceFileRow>(
            "SELECT
                sf.path,
                sf.content
             FROM source_files sf
             WHERE sf.crate_version_id = $1
               AND (sf.language IS NULL OR sf.language != 'rustdoc_json')
             ORDER BY sf.path ASC
             LIMIT 1000",
        )
        .bind(crate_version_id)
        .fetch_all(db)
        .await
    }
}

/// Lists Rust-language source files for a crate version.
pub async fn list_rust_source_files_for_version(
    db: &PgPool,
    crate_version_id: i64,
) -> Result<Vec<HotspotSourceFileRow>, sqlx::Error> {
    sqlx::query_as::<_, HotspotSourceFileRow>(
        "SELECT path, content
         FROM source_files
         WHERE crate_version_id = $1
           AND language = 'rust'
         ORDER BY path ASC",
    )
    .bind(crate_version_id)
    .fetch_all(db)
    .await
}

/// Lists declared Cargo features for a crate version.
pub async fn list_crate_features_for_version(
    db: &PgPool,
    crate_version_id: i64,
) -> Result<Vec<FeatureImpactFeatureRow>, sqlx::Error> {
    sqlx::query_as::<_, FeatureImpactFeatureRow>(
        "SELECT feature_name, enables
         FROM crate_version_features
         WHERE crate_version_id = $1
         ORDER BY feature_name ASC",
    )
    .bind(crate_version_id)
    .fetch_all(db)
    .await
}

/// Lists dependency edges contributing to feature-impact analysis.
pub async fn list_feature_impact_dependencies_for_version(
    db: &PgPool,
    crate_version_id: i64,
) -> Result<Vec<FeatureImpactDependencyRow>, sqlx::Error> {
    sqlx::query_as::<_, FeatureImpactDependencyRow>(
        "SELECT
            c.name AS dependency_name,
            de.optional
         FROM dependency_edges de
         JOIN crates c ON c.id = de.to_crate_id
         WHERE de.from_version_id = $1",
    )
    .bind(crate_version_id)
    .fetch_all(db)
    .await
}

/// Lists version timeline rows with release, downloads, and advisory counts.
pub async fn list_crate_version_timeline(
    db: &PgPool,
    crate_id: i64,
    limit: i64,
    offset: i64,
) -> Result<Vec<CrateVersionTimelineRow>, sqlx::Error> {
    sqlx::query_as::<_, CrateVersionTimelineRow>(
        "SELECT
            cv.version,
            cv.rust_version,
            cv.published_at::TEXT AS published_at,
            cv.yanked,
            COALESCE(cv.total_downloads, 0)::BIGINT AS total_downloads,
            COUNT(am.id)::BIGINT AS advisory_count,
            CASE
                WHEN cv.published_at IS NULL THEN NULL
                ELSE EXTRACT(EPOCH FROM (NOW() - cv.published_at))::BIGINT / 86400
            END AS release_age_days
         FROM crate_versions cv
         LEFT JOIN advisory_matches am ON am.version_id = cv.id
         WHERE cv.crate_id = $1
         GROUP BY cv.id, cv.version, cv.published_at, cv.yanked, cv.total_downloads
         ORDER BY cv.published_at DESC NULLS LAST, cv.id DESC
         LIMIT $2
         OFFSET $3",
    )
    .bind(crate_id)
    .bind(limit.max(1))
    .bind(offset.max(0))
    .fetch_all(db)
    .await
}

/// Lists candidate alternative crates ranked by overlap and similarity.
pub async fn list_alternatives_candidates(
    db: &PgPool,
    crate_id: i64,
    crate_name: &str,
    categories: &[String],
    keywords: &[String],
    limit: i64,
) -> Result<Vec<AlternativesCandidateRow>, sqlx::Error> {
    sqlx::query_as::<_, AlternativesCandidateRow>(
        "SELECT
            c.name AS crate_name,
            c.description,
            c.categories,
            c.keywords,
            lv.version AS latest_version,
            COALESCE(lv.total_downloads, 0)::BIGINT AS total_downloads,
            COALESCE(lv.yanked, false) AS yanked,
            COALESCE(lv.advisory_count, 0)::BIGINT AS advisory_count,
            lv.license_expression,
            COALESCE(dep.dependent_count, 0)::BIGINT AS dependent_count,
            similarity(c.name, $2)::DOUBLE PRECISION AS name_similarity
         FROM crates c
         LEFT JOIN LATERAL (
             SELECT
                 cv.version,
                 cv.total_downloads,
                 cv.yanked,
                 cv.license_expression,
                 COUNT(am.id)::BIGINT AS advisory_count
             FROM crate_versions cv
             LEFT JOIN advisory_matches am ON am.version_id = cv.id
             WHERE cv.crate_id = c.id
             GROUP BY cv.id
             ORDER BY cv.published_at DESC NULLS LAST, cv.id DESC
             LIMIT 1
         ) lv ON true
         LEFT JOIN LATERAL (
             SELECT COALESCE(COUNT(DISTINCT cv_from.crate_id), 0)::BIGINT AS dependent_count
             FROM dependency_edges d
             JOIN crate_versions cv_from ON cv_from.id = d.from_version_id
             WHERE d.to_crate_id = c.id
         ) dep ON true
         WHERE c.id <> $1
           AND (
               similarity(c.name, $2) >= 0.10
               OR c.categories && $3::TEXT[]
               OR c.keywords && $4::TEXT[]
           )
         ORDER BY
            similarity(c.name, $2) DESC,
            COALESCE(lv.total_downloads, 0) DESC,
            c.name ASC
         LIMIT $5",
    )
    .bind(crate_id)
    .bind(crate_name)
    .bind(categories)
    .bind(keywords)
    .bind(limit.max(1))
    .fetch_all(db)
    .await
}

/// Lists canonicalized API symbols for one version in API-diff comparisons.
pub async fn list_api_diff_symbols_for_version(
    db: &PgPool,
    crate_version_id: i64,
) -> Result<Vec<ApiDiffSymbolRow>, sqlx::Error> {
    sqlx::query_as::<_, ApiDiffSymbolRow>(
        "SELECT DISTINCT ON (s.name, s.kind)
            s.name,
            s.kind,
            s.signature,
            s.visibility,
            s.index_source
         FROM symbols s
         WHERE s.crate_version_id = $1
           AND (s.visibility = 'public' OR s.visibility IS NULL)
         ORDER BY s.name ASC,
                  s.kind ASC,
                  CASE WHEN s.index_source = 'rustdoc_json' THEN 0 ELSE 1 END,
                  CASE WHEN s.signature IS NULL THEN 1 ELSE 0 END,
                  s.start_line ASC,
                  s.end_line ASC,
                  s.id ASC",
    )
    .bind(crate_version_id)
    .fetch_all(db)
    .await
}

/// Loads the latest version row used by crate comparison workflows.
pub async fn fetch_compare_latest_version(
    db: &PgPool,
    crate_id: i64,
) -> Result<Option<CrateCompareVersionRow>, sqlx::Error> {
    sqlx::query_as::<_, CrateCompareVersionRow>(
        "SELECT
            id,
            version,
            rust_version,
            published_at::TEXT AS published_at,
            yanked,
            total_downloads,
            license_expression
         FROM crate_versions
         WHERE crate_id = $1
         ORDER BY published_at DESC NULLS LAST, id DESC
         LIMIT 1",
    )
    .bind(crate_id)
    .fetch_optional(db)
    .await
}

/// Loads a specific version row used by crate comparison workflows.
pub async fn fetch_compare_version_by_name(
    db: &PgPool,
    crate_id: i64,
    version: &str,
) -> Result<Option<CrateCompareVersionRow>, sqlx::Error> {
    sqlx::query_as::<_, CrateCompareVersionRow>(
        "SELECT
            id,
            version,
            rust_version,
            published_at::TEXT AS published_at,
            yanked,
            total_downloads,
            license_expression
         FROM crate_versions
         WHERE crate_id = $1 AND version = $2
         LIMIT 1",
    )
    .bind(crate_id)
    .bind(version)
    .fetch_optional(db)
    .await
}

/// Loads aggregate comparison counters for dependencies, features, and risk.
pub async fn fetch_compare_counts(
    db: &PgPool,
    selected_version_id: i64,
    crate_id: i64,
) -> Result<CrateCompareCountsRow, sqlx::Error> {
    sqlx::query_as::<_, CrateCompareCountsRow>(
        "SELECT
            (SELECT COUNT(*)::BIGINT
             FROM dependency_edges de
             WHERE de.from_version_id = $1) AS dependency_count,
            (SELECT COUNT(*)::BIGINT
             FROM crate_version_features cvf
             WHERE cvf.crate_version_id = $1) AS feature_count,
            (SELECT COUNT(*)::BIGINT
             FROM advisory_matches am
             WHERE am.crate_id = $2
               AND (am.version_id = $1 OR am.version_id IS NULL)) AS advisory_count,
            (SELECT COUNT(DISTINCT cv_from.crate_id)::BIGINT
             FROM dependency_edges de2
             JOIN crate_versions cv_from ON cv_from.id = de2.from_version_id
             WHERE de2.to_crate_id = $2) AS dependent_count",
    )
    .bind(selected_version_id)
    .bind(crate_id)
    .fetch_one(db)
    .await
}

/// Lists latest crate-version ids for graph traversal roots.
pub async fn list_graph_latest_versions_for_crates(
    db: &PgPool,
    crate_ids: &[i64],
) -> Result<Vec<GraphLatestVersionRow>, sqlx::Error> {
    if crate_ids.is_empty() {
        return Ok(Vec::new());
    }

    sqlx::query_as::<_, GraphLatestVersionRow>(
        "SELECT DISTINCT ON (crate_id)
            crate_id,
            id,
            version
         FROM crate_versions
         WHERE crate_id = ANY($1)
         ORDER BY crate_id, published_at DESC NULLS LAST, id DESC",
    )
    .bind(crate_ids)
    .fetch_all(db)
    .await
}

/// Lists outgoing dependency traversal edges for graph expansion.
pub async fn list_graph_dependency_traversal_rows(
    db: &PgPool,
    from_version_ids: &[i64],
) -> Result<Vec<GraphDependencyTraversalRow>, sqlx::Error> {
    if from_version_ids.is_empty() {
        return Ok(Vec::new());
    }

    sqlx::query_as::<_, GraphDependencyTraversalRow>(
        "SELECT
            c_from.name AS from_crate_name,
            cv_from.version AS from_version,
            d.to_crate_id AS to_crate_id,
            c_to.name AS to_crate_name,
            d.requirement,
            d.dependency_kind,
            d.optional
         FROM dependency_edges d
         JOIN crate_versions cv_from ON cv_from.id = d.from_version_id
         JOIN crates c_from ON c_from.id = cv_from.crate_id
         JOIN crates c_to ON c_to.id = d.to_crate_id
         WHERE d.from_version_id = ANY($1)",
    )
    .bind(from_version_ids)
    .fetch_all(db)
    .await
}

/// Lists incoming dependent traversal edges for reverse graph expansion.
pub async fn list_graph_dependent_traversal_rows(
    db: &PgPool,
    to_crate_ids: &[i64],
) -> Result<Vec<GraphDependentTraversalRow>, sqlx::Error> {
    if to_crate_ids.is_empty() {
        return Ok(Vec::new());
    }

    sqlx::query_as::<_, GraphDependentTraversalRow>(
        "SELECT
            cv_from.crate_id AS from_crate_id,
            c_from.name AS from_crate_name,
            cv_from.version AS from_version,
            c_to.name AS to_crate_name,
            d.requirement,
            d.dependency_kind,
            d.optional
         FROM dependency_edges d
         JOIN crate_versions cv_from ON cv_from.id = d.from_version_id
         JOIN crates c_from ON c_from.id = cv_from.crate_id
         JOIN crates c_to ON c_to.id = d.to_crate_id
         WHERE d.to_crate_id = ANY($1)",
    )
    .bind(to_crate_ids)
    .fetch_all(db)
    .await
}

/// Loads a crate id/name pair for dependency resolution by crate name.
pub async fn fetch_dependency_crate_by_name(
    db: &PgPool,
    crate_name: &str,
) -> Result<Option<DependencyCrateRow>, sqlx::Error> {
    sqlx::query_as::<_, DependencyCrateRow>("SELECT id, name FROM crates WHERE name = $1 LIMIT 1")
        .bind(crate_name)
        .fetch_optional(db)
        .await
}

/// Lists indexed versions for a crate id in newest-first order.
pub async fn list_dependency_versions_by_crate(
    db: &PgPool,
    crate_id: i64,
) -> Result<Vec<DependencyVersionRow>, sqlx::Error> {
    sqlx::query_as::<_, DependencyVersionRow>(
        "SELECT id, version, rust_version, yanked
         FROM crate_versions
         WHERE crate_id = $1
         ORDER BY published_at DESC NULLS LAST, id DESC",
    )
    .bind(crate_id)
    .fetch_all(db)
    .await
}

/// Lists indexed versions for a crate matched by lowercase crate name.
pub async fn list_dependency_versions_by_lower_name(
    db: &PgPool,
    crate_name_lower: &str,
) -> Result<Vec<DependencyVersionRow>, sqlx::Error> {
    sqlx::query_as::<_, DependencyVersionRow>(
        "SELECT cv.id, cv.version, cv.rust_version, cv.yanked
         FROM crate_versions cv
         JOIN crates c ON c.id = cv.crate_id
         WHERE LOWER(c.name) = $1",
    )
    .bind(crate_name_lower)
    .fetch_all(db)
    .await
}

/// Counts advisories applying to a crate version (including crate-level
/// advisories).
pub async fn count_advisories_for_crate_version(
    db: &PgPool,
    crate_id: i64,
    version_id: i64,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*)::BIGINT
         FROM advisory_matches
         WHERE crate_id = $1
           AND (version_id = $2 OR version_id IS NULL)",
    )
    .bind(crate_id)
    .bind(version_id)
    .fetch_one(db)
    .await
}

/// Lists dependency edges for selected versions used by resolver output.
pub async fn list_dependency_resolve_edges_for_versions(
    db: &PgPool,
    version_ids: &[i64],
) -> Result<Vec<DependencyResolveEdgeRow>, sqlx::Error> {
    if version_ids.is_empty() {
        return Ok(Vec::new());
    }

    sqlx::query_as::<_, DependencyResolveEdgeRow>(
        "SELECT
            to_c.name AS to_crate_name,
            de.requirement,
            de.optional,
            de.features
         FROM dependency_edges de
         JOIN crates to_c ON to_c.id = de.to_crate_id
         WHERE de.from_version_id = ANY($1::BIGINT[])",
    )
    .bind(version_ids)
    .fetch_all(db)
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

/// Lists crate impl rows filtered by optional trait/type names and pagination.
pub async fn list_crate_impl_rows_for_filters(
    db: &PgPool,
    crate_version_id: i64,
    trait_name: Option<&str>,
    type_name: Option<&str>,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<Vec<super::models::CrateImplLookupRow>, sqlx::Error> {
    let mut base = String::from(
        "SELECT
            ci.type_name,
            ci.type_name_display,
            ci.trait_name,
            ci.trait_name_display,
            ci.impl_kind,
            ci.methods,
            ci.is_blanket,
            ci.is_synthetic,
            ci.is_negative,
            ci.blanket_type,
            ci.generics,
            ci.where_clauses,
            sf.path AS source_path,
            ci.start_line,
            ci.end_line,
            ci.index_source
         FROM crate_impls ci
         JOIN source_files sf ON sf.id = ci.source_file_id
         WHERE ci.crate_version_id = $1
           AND ($2::TEXT IS NULL OR LOWER(ci.trait_name) = LOWER($2))
           AND ($3::TEXT IS NULL OR LOWER(ci.type_name) = LOWER($3))
         ORDER BY
            CASE ci.impl_kind
                WHEN 'inherent' THEN 0
                WHEN 'derive' THEN 1
                WHEN 'trait' THEN 2
                ELSE 3
            END,
            CASE
                WHEN ci.index_source = 'rustdoc_json'
                     AND (
                        jsonb_array_length(ci.methods::jsonb) > 0
                        OR ci.trait_name_display IS NOT NULL
                        OR ci.is_blanket
                        OR ci.is_synthetic
                        OR ci.is_negative
                     ) THEN 0
                WHEN ci.index_source = 'rustdoc_json' THEN 1
                ELSE 2
            END,
            ci.type_name ASC,
            ci.start_line ASC",
    );

    if limit.is_some() {
        base.push_str(" LIMIT $4 OFFSET $5");
        sqlx::query_as::<_, super::models::CrateImplLookupRow>(&base)
            .bind(crate_version_id)
            .bind(trait_name)
            .bind(type_name)
            .bind(limit.unwrap_or(1).max(1))
            .bind(offset.unwrap_or(0).max(0))
            .fetch_all(db)
            .await
    } else {
        sqlx::query_as::<_, super::models::CrateImplLookupRow>(&base)
            .bind(crate_version_id)
            .bind(trait_name)
            .bind(type_name)
            .fetch_all(db)
            .await
    }
}

/// Loads the best matching type-info row for a crate type name.
pub async fn fetch_crate_type_info_row(
    db: &PgPool,
    crate_version_id: i64,
    type_name: &str,
) -> Result<Option<CrateTypeInfoRow>, sqlx::Error> {
    sqlx::query_as::<_, CrateTypeInfoRow>(
        "SELECT
            ct.type_name,
            ct.kind,
            ct.visibility,
            ct.canonical_path,
            ct.definition_path,
            ct.generic_params,
            ct.where_clauses,
            ct.fields,
            ct.variants,
            ct.deprecated_since,
            ct.deprecated_note,
            ct.is_non_exhaustive,
            ct.auto_traits,
            sf.path AS source_path,
            ct.start_line,
            ct.end_line,
            ct.index_source
         FROM crate_types ct
         JOIN source_files sf ON sf.id = ct.source_file_id
         WHERE ct.crate_version_id = $1
           AND LOWER(ct.type_name) = LOWER($2)
         ORDER BY
            CASE WHEN ct.visibility = 'public' THEN 0 ELSE 1 END,
            CASE
                WHEN ct.index_source = 'rustdoc_json'
                     AND ct.canonical_path IS NOT NULL
                     AND ct.definition_path IS NOT NULL THEN 0
                WHEN ct.index_source = 'rustdoc_json' THEN 1
                WHEN ct.canonical_path IS NOT NULL THEN 2
                ELSE 3
            END,
            ct.start_line ASC
         LIMIT 1",
    )
    .bind(crate_version_id)
    .bind(type_name)
    .fetch_optional(db)
    .await
}

/// Lists trait rows for a crate version by lowercase trait-name set.
pub async fn list_crate_trait_rows_for_names(
    db: &PgPool,
    crate_version_id: i64,
    names_lower: &[String],
) -> Result<Vec<super::models::CrateTraitLookupRow>, sqlx::Error> {
    if names_lower.is_empty() {
        return Ok(Vec::new());
    }

    sqlx::query_as::<_, super::models::CrateTraitLookupRow>(
        "SELECT
            trait_name,
            is_auto,
            is_unsafe,
            is_dyn_compatible,
            supertraits,
            required_methods,
            provided_methods,
            associated_types,
            generics,
            index_source
         FROM crate_traits
         WHERE crate_version_id = $1
           AND LOWER(trait_name) = ANY($2::TEXT[])
         ORDER BY
            CASE WHEN index_source = 'rustdoc_json' THEN 0 ELSE 1 END,
            trait_name ASC",
    )
    .bind(crate_version_id)
    .bind(names_lower)
    .fetch_all(db)
    .await
}

/// Searches indexed source files with crate/version/path filters and mode
/// selection.
#[derive(Debug, Clone, Copy)]
pub struct SourceFileSearchParams<'a> {
    /// Search term for text or regex matching.
    pub query: &'a str,
    /// Optional exact crate-name filter.
    pub crate_name: Option<&'a str>,
    /// Optional exact version filter.
    pub version: Option<&'a str>,
    /// Optional path pattern for `ILIKE` search.
    pub path_like: Option<&'a str>,
    /// Search mode (`text` or `regex`).
    pub mode: SourceSearchMode,
    /// Maximum rows to return.
    pub limit: i64,
    /// Zero-based offset for pagination.
    pub offset: i64,
}

/// Searches indexed source files with crate/version/path filters and mode
/// selection.
pub async fn search_source_files(
    db: &PgPool,
    params: &SourceFileSearchParams<'_>,
) -> Result<Vec<SourceSearchRow>, sqlx::Error> {
    let mut qb = QueryBuilder::<Postgres>::new(
        "SELECT
            c.name AS crate_name,
            cv.version,
            sf.path,
            sf.content,
            sf.indexed_at::TEXT AS indexed_at
         FROM source_files sf
         JOIN crate_versions cv ON cv.id = sf.crate_version_id
         JOIN crates c ON c.id = cv.crate_id ",
    );

    let mut has_where = false;
    if let Some(crate_name_filter) = params.crate_name {
        qb.push(if has_where { "AND " } else { "WHERE " });
        has_where = true;
        qb.push("c.name = ");
        qb.push_bind(crate_name_filter);
        qb.push(' ');
    }

    if let Some(version_filter) = params.version {
        qb.push(if has_where { "AND " } else { "WHERE " });
        has_where = true;
        qb.push("cv.version = ");
        qb.push_bind(version_filter);
        qb.push(' ');
    }

    if let Some(path_filter) = params.path_like {
        qb.push(if has_where { "AND " } else { "WHERE " });
        has_where = true;
        qb.push("sf.path ILIKE ");
        qb.push_bind(path_filter);
        qb.push(" ESCAPE '\\\\' ");
    }

    qb.push(if has_where { "AND " } else { "WHERE " });
    match params.mode {
        SourceSearchMode::Text => {
            qb.push("sf.content ILIKE ");
            qb.push_bind(format!("%{}%", params.query));
            qb.push(' ');
        }
        SourceSearchMode::Regex => {
            qb.push("sf.content ~* ");
            qb.push_bind(params.query);
            qb.push(' ');
        }
    }

    qb.push("AND (sf.language IS NULL OR sf.language != 'rustdoc_json') ");

    qb.push("ORDER BY sf.indexed_at DESC, c.name ASC, sf.path ASC LIMIT ");
    qb.push_bind(params.limit.max(1));
    qb.push(" OFFSET ");
    qb.push_bind(params.offset.max(0));

    qb.build_query_as::<SourceSearchRow>()
        .fetch_all(db)
        .await
}

/// Searches crates with optional query/category/keyword filters and sort
/// strategy.
#[derive(Debug, Clone, Copy)]
pub struct CrateSearchParams<'a> {
    /// Optional free-text search query.
    pub query: Option<&'a str>,
    /// Optional exact category filter.
    pub category: Option<&'a str>,
    /// Optional exact keyword filter.
    pub keyword: Option<&'a str>,
    /// Sort strategy for result ordering.
    pub sort: CrateSearchSort,
    /// Maximum rows to return.
    pub limit: i64,
    /// Zero-based offset for pagination.
    pub offset: i64,
}

/// Searches crates with optional query/category/keyword filters and sort
/// strategy.
pub async fn search_crates(
    db: &PgPool,
    params: &CrateSearchParams<'_>,
) -> Result<Vec<super::models::CrateSearchRow>, sqlx::Error> {
    let mut qb = QueryBuilder::<Postgres>::new(
        "SELECT c.id AS crate_id, c.name, c.description, c.repository_url, c.docs_url, \
         c.homepage_url, c.categories, c.keywords, COALESCE(MAX(cv.total_downloads), 0)::BIGINT \
         AS total_downloads, (ARRAY_REMOVE(ARRAY_AGG(cv.version ORDER BY cv.published_at DESC \
         NULLS LAST, cv.id DESC), NULL))[1] AS latest_version, MAX(cv.published_at)::TEXT AS \
         latest_published_at, COALESCE(COUNT(DISTINCT dep.from_version_id), 0)::BIGINT AS \
         dependent_count, ",
    );

    if let Some(q) = params.query {
        let name_like = format!("%{q}%");
        qb.push("(similarity(c.name, ");
        qb.push_bind(q);
        qb.push(") * 2.0 + similarity(COALESCE(c.description, ''), ");
        qb.push_bind(q);
        qb.push(") + CASE WHEN c.name ILIKE ");
        qb.push_bind(name_like);
        qb.push(
            " THEN 1.5 ELSE 0 END + CASE WHEN to_tsvector('english', COALESCE(c.description, '')) \
             @@ websearch_to_tsquery('english', ",
        );
        qb.push_bind(q);
        qb.push(") THEN 1.0 ELSE 0 END)::DOUBLE PRECISION AS relevance_score ");
    } else {
        qb.push("0.0::DOUBLE PRECISION AS relevance_score ");
    }

    qb.push(
        "FROM crates c LEFT JOIN crate_versions cv ON cv.crate_id = c.id LEFT JOIN \
         dependency_edges dep ON dep.to_crate_id = c.id ",
    );

    let mut has_where = false;

    if let Some(q) = params.query {
        if !has_where {
            qb.push("WHERE ");
            has_where = true;
        }
        qb.push("(");
        qb.push("c.name ILIKE ");
        qb.push_bind(format!("%{q}%"));
        qb.push(" OR COALESCE(c.description, '') ILIKE ");
        qb.push_bind(format!("%{q}%"));
        qb.push(" OR COALESCE(c.keywords, '{}'::TEXT[]) @> ARRAY[");
        qb.push_bind(q);
        qb.push("]::TEXT[]");
        qb.push(" OR COALESCE(c.categories, '{}'::TEXT[]) @> ARRAY[");
        qb.push_bind(q);
        qb.push("]::TEXT[]");
        qb.push(
            " OR to_tsvector('english', COALESCE(c.description, '')) @@ \
             websearch_to_tsquery('english', ",
        );
        qb.push_bind(q);
        qb.push(")");
        qb.push(")");
    }

    if let Some(c) = params.category {
        if !has_where {
            qb.push("WHERE ");
            has_where = true;
        } else {
            qb.push("AND ");
        }
        qb.push("COALESCE(c.categories, '{}'::TEXT[]) @> ARRAY[");
        qb.push_bind(c);
        qb.push("]::TEXT[] ");
    }

    if let Some(k) = params.keyword {
        if !has_where {
            qb.push("WHERE ");
        } else {
            qb.push("AND ");
        }
        qb.push("COALESCE(c.keywords, '{}'::TEXT[]) @> ARRAY[");
        qb.push_bind(k);
        qb.push("]::TEXT[] ");
    }

    qb.push(
        "GROUP BY c.id, c.name, c.description, c.repository_url, c.docs_url, c.homepage_url, \
         c.categories, c.keywords ",
    );

    match params.sort {
        CrateSearchSort::Relevance => {
            if params.query.is_some() {
                qb.push("ORDER BY relevance_score DESC, total_downloads DESC, c.name ASC ");
            } else {
                qb.push("ORDER BY total_downloads DESC, c.name ASC ");
            }
        }
        CrateSearchSort::Downloads => {
            qb.push("ORDER BY total_downloads DESC, c.name ASC ");
        }
        CrateSearchSort::Recent => {
            qb.push(
                "ORDER BY MAX(cv.published_at) DESC NULLS LAST, total_downloads DESC, c.name ASC ",
            );
        }
    }

    qb.push("LIMIT ");
    qb.push_bind(params.limit.max(1));
    qb.push(" OFFSET ");
    qb.push_bind(params.offset.max(0));

    qb.build_query_as::<super::models::CrateSearchRow>()
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

/// Lists all deprecated symbols and types for a crate version.
///
/// Unions rows from `symbols` and `crate_types` where either `deprecated_since`
/// or `deprecated_note` is non-null, ordered by name then kind.
pub async fn list_deprecated_items(
    db: &PgPool,
    crate_version_id: i64,
    limit: i64,
    offset: i64,
) -> Result<Vec<DeprecatedItemRow>, sqlx::Error> {
    sqlx::query_as::<_, DeprecatedItemRow>(
        "SELECT
             s.name,
             s.kind,
             s.deprecated_since,
             s.deprecated_note,
             s.canonical_path,
             s.index_source
         FROM symbols s
         WHERE s.crate_version_id = $1
           AND (s.deprecated_since IS NOT NULL OR s.deprecated_note IS NOT NULL)
         UNION ALL
         SELECT
             ct.type_name AS name,
             ct.kind,
             ct.deprecated_since,
             ct.deprecated_note,
             ct.canonical_path,
             ct.index_source
         FROM crate_types ct
         WHERE ct.crate_version_id = $1
           AND (ct.deprecated_since IS NOT NULL OR ct.deprecated_note IS NOT NULL)
         ORDER BY name ASC, kind ASC
         LIMIT $2 OFFSET $3",
    )
    .bind(crate_version_id)
    .bind(limit.max(1))
    .bind(offset.max(0))
    .fetch_all(db)
    .await
}
