use std::collections::{HashMap, HashSet};

use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};

use super::models::{
    CrateLevelAdvisoryMatchInsert, GitHubCandidateRow, GitHubMetadataInsert, GitHubMetadataRow,
    GitHubReleaseRow, IndexCoverageCountsRow, IndexFailureByScopeRow, IndexFreshnessRow,
    IndexOperationalMetricsRow, IndexQueueCountsRow, IndexedExtractionBatch,
    LocalCacheVersionKeyRow, RefreshJobGaugeCountsRow, RefreshJobQueueRow,
    RefreshJobRetryDistributionRow, RustdocSyncCandidateRow, SecurityCrateRow, SecurityVersionRow,
    VersionAdvisoryMatchInsert,
};
use crate::integration::crates_io::{CratesIoCrateDetailResponse, CratesIoDependency};

/// Row counts written by a batch index insert.
#[derive(Debug, Clone, Copy, Default)]
pub struct IndexedWriteCounts {
    /// Number of `symbols` rows inserted.
    pub symbols: usize,
    /// Number of `crate_types` rows inserted.
    pub types: usize,
    /// Number of `crate_impls` rows inserted.
    pub impls: usize,
    /// Number of `crate_traits` rows inserted.
    pub traits: usize,
}

/// Row counts written while persisting one crate sync operation.
#[derive(Debug, Clone, Copy, Default)]
pub struct PersistCrateSyncOutcome {
    /// Number of crate versions upserted.
    pub versions_synced: usize,
    /// Number of dependency edges written.
    pub dependencies_synced: usize,
}

/// Marks which crate versions are locally present in the cargo registry.
///
/// Versions in `local_versions` are set to `locally_present = TRUE`; all
/// other versions of the same crate are reset to `FALSE`. Uses
/// `IS DISTINCT FROM` to skip no-op updates.
pub async fn mark_versions_locally_present(
    db: &PgPool,
    crate_name: &str,
    local_versions: &[String],
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE crate_versions cv
         SET locally_present = (cv.version = ANY($2::TEXT[])),
             source_origin = CASE
                 WHEN cv.version = ANY($2::TEXT[]) THEN 1
                 ELSE cv.source_origin
             END
         FROM crates c
         WHERE c.id = cv.crate_id
           AND c.name_lower = LOWER($1)
           AND (cv.locally_present IS DISTINCT FROM (cv.version = ANY($2::TEXT[]))
                OR (cv.version = ANY($2::TEXT[]) AND cv.source_origin <> 1))",
    )
    .bind(crate_name)
    .bind(local_versions)
    .execute(db)
    .await?;
    Ok(result.rows_affected())
}

/// Marks a crate version as successfully enriched with rustdoc JSON.
///
/// Sets both `rustdoc_enriched_at` and `rustdoc_last_attempt_at` to the
/// current time.
pub async fn mark_version_rustdoc_enriched(
    db: &PgPool,
    crate_version_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE crate_versions
         SET rustdoc_enriched_at = NOW(),
             rustdoc_last_attempt_at = NOW()
         WHERE id = $1",
    )
    .bind(crate_version_id)
    .execute(db)
    .await?;
    Ok(())
}

/// Records that a rustdoc enrichment attempt was made but did not succeed.
///
/// Only updates `rustdoc_last_attempt_at` so that the retry cooldown is
/// respected without marking the version as fully enriched.
pub async fn mark_version_rustdoc_attempted(
    db: &PgPool,
    crate_version_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE crate_versions
         SET rustdoc_last_attempt_at = NOW()
         WHERE id = $1",
    )
    .bind(crate_version_id)
    .execute(db)
    .await?;
    Ok(())
}

/// Sets the `source_origin` flag on a crate version.
pub async fn set_source_origin(
    db: &PgPool,
    crate_version_id: i64,
    origin: i16,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE crate_versions
         SET source_origin = $2
         WHERE id = $1",
    )
    .bind(crate_version_id)
    .bind(origin)
    .execute(db)
    .await?;
    Ok(())
}

/// Returns distinct crate names that have locally-present versions needing
/// rustdoc enrichment.
///
/// A version is considered "needing enrichment" when `rustdoc_enriched_at`
/// is NULL and either no attempt has been made or the last attempt is older
/// than `retry_cooldown_seconds`.
pub async fn fetch_unenriched_crate_names(
    db: &PgPool,
    retry_cooldown_seconds: i64,
) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar::<_, String>(
        "SELECT DISTINCT c.name
         FROM crate_versions cv
         JOIN crates c ON c.id = cv.crate_id
         WHERE cv.locally_present = TRUE
           AND cv.rustdoc_enriched_at IS NULL
           AND (cv.rustdoc_last_attempt_at IS NULL
                OR cv.rustdoc_last_attempt_at < NOW() - make_interval(secs => $1::FLOAT8))
         ORDER BY c.name",
    )
    .bind(retry_cooldown_seconds as f64)
    .fetch_all(db)
    .await
}

/// Loads crate versions eligible for local source cache indexing.
///
/// When `locally_present_only` is `true`, only versions flagged as present
/// in the user's cargo registry are returned.
pub async fn fetch_local_cache_version_keys(
    db: &PgPool,
    crate_name: Option<&str>,
    locally_present_only: bool,
) -> Result<Vec<LocalCacheVersionKeyRow>, sqlx::Error> {
    sqlx::query_as::<_, LocalCacheVersionKeyRow>(
        "SELECT
            (c.name || '-' || cv.version) AS cache_dir_name,
            c.name AS crate_name,
            cv.version,
            cv.id AS crate_version_id
         FROM crate_versions cv
         JOIN crates c ON c.id = cv.crate_id
         WHERE ($1::TEXT IS NULL OR c.name_lower = LOWER($1))
           AND ($2::BOOL IS FALSE OR cv.locally_present = TRUE)",
    )
    .bind(crate_name)
    .bind(locally_present_only)
    .fetch_all(db)
    .await
}

/// Loads crate versions eligible for rustdoc JSON refresh.
///
/// When `locally_present_only` is `true`, only versions flagged as present
/// in the user's cargo registry are returned.
///
/// When `skip_enriched` is `true`, versions with `rustdoc_enriched_at` set
/// are excluded. Versions whose last attempt is within
/// `retry_cooldown_seconds` are also excluded; pass `None` to skip all
/// previously-attempted versions regardless of age.
pub async fn fetch_rustdoc_sync_candidates(
    db: &PgPool,
    crate_name: Option<&str>,
    limit: i64,
    offset: i64,
    locally_present_only: bool,
    skip_enriched: bool,
    retry_cooldown_seconds: Option<i64>,
) -> Result<Vec<RustdocSyncCandidateRow>, sqlx::Error> {
    sqlx::query_as::<_, RustdocSyncCandidateRow>(
        "SELECT
            c.name AS crate_name,
            cv.version,
            cv.id AS crate_version_id
         FROM crate_versions cv
         JOIN crates c ON c.id = cv.crate_id
         WHERE ($1::TEXT IS NULL OR c.name_lower = LOWER($1))
           AND ($4::BOOL IS FALSE OR cv.locally_present = TRUE)
           AND ($5::BOOL IS FALSE OR (
               cv.rustdoc_enriched_at IS NULL
               AND (cv.rustdoc_last_attempt_at IS NULL
                    OR ($6::BIGINT IS NOT NULL
                        AND cv.rustdoc_last_attempt_at < NOW() - make_interval(secs => \
         $6::BIGINT)))
           ))
         ORDER BY cv.published_at DESC NULLS LAST, cv.id DESC
         LIMIT $2 OFFSET $3",
    )
    .bind(crate_name)
    .bind(limit)
    .bind(offset)
    .bind(locally_present_only)
    .bind(skip_enriched)
    .bind(retry_cooldown_seconds)
    .fetch_all(db)
    .await
}

/// Upserts a source file metadata row and only writes when the SHA changes.
///
/// Content is not stored — it is read from the cargo registry on disk.
/// Returns `Some(id)` when the row was inserted or updated, `None` when the
/// SHA matched and no write occurred.
pub async fn upsert_source_file_if_sha_changed(
    db: &PgPool,
    crate_version_id: i64,
    path: &str,
    sha256: &str,
    file_size: i64,
    language: Option<&str>,
) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO source_files (
            crate_version_id, path, sha256, file_size, language, indexed_at
         ) VALUES (
            $1, $2, $3, $4, $5, NOW()
         )
         ON CONFLICT (crate_version_id, path) DO UPDATE
         SET sha256 = EXCLUDED.sha256,
             file_size = EXCLUDED.file_size,
             language = EXCLUDED.language,
             indexed_at = NOW()
         WHERE source_files.sha256 IS DISTINCT FROM EXCLUDED.sha256
         RETURNING id",
    )
    .bind(crate_version_id)
    .bind(path)
    .bind(sha256)
    .bind(file_size)
    .bind(language)
    .fetch_optional(db)
    .await
}

/// Upserts a source file metadata row unconditionally.
///
/// Content is not stored — it is read from the cargo registry on disk.
/// Always returns the row id (insert or update).
pub async fn upsert_source_file_unconditional(
    db: &PgPool,
    crate_version_id: i64,
    path: &str,
    sha256: &str,
    file_size: i64,
    language: Option<&str>,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO source_files (
            crate_version_id,
            path,
            sha256,
            file_size,
            language,
            indexed_at
         ) VALUES (
            $1, $2, $3, $4, $5, NOW()
         )
         ON CONFLICT (crate_version_id, path) DO UPDATE
         SET sha256 = EXCLUDED.sha256,
             file_size = EXCLUDED.file_size,
             language = EXCLUDED.language,
             indexed_at = NOW()
         RETURNING id",
    )
    .bind(crate_version_id)
    .bind(path)
    .bind(sha256)
    .bind(file_size)
    .bind(language)
    .fetch_one(db)
    .await
}

/// Loads a source file id for the crate version and relative path.
pub async fn fetch_source_file_id_optional(
    db: &PgPool,
    crate_version_id: i64,
    path: &str,
) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar::<_, i64>(
        "SELECT id
         FROM source_files
         WHERE crate_version_id = $1 AND path = $2
         LIMIT 1",
    )
    .bind(crate_version_id)
    .bind(path)
    .fetch_optional(db)
    .await
}

/// Counts currently indexed symbol/type/impl rows for one source file.
pub async fn count_source_file_index_rows(
    db: &PgPool,
    source_file_id: i64,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar::<_, i64>(
        "SELECT (
            (SELECT COUNT(*)::BIGINT FROM symbols WHERE source_file_id = $1)
            + (SELECT COUNT(*)::BIGINT FROM crate_types WHERE source_file_id = $1)
            + (SELECT COUNT(*)::BIGINT FROM crate_impls WHERE source_file_id = $1)
         )::BIGINT",
    )
    .bind(source_file_id)
    .fetch_one(db)
    .await
}

/// Loads a page of crates for advisory sync processing.
///
/// When `max_age_secs` is `Some(n)`, crates whose `security_checked_at` is
/// less than `n` seconds ago are skipped — they were already checked recently.
/// Pass `None` to check all crates unconditionally.
pub async fn fetch_security_crates_page(
    db: &PgPool,
    limit: i64,
    offset: i64,
    max_age_secs: Option<i64>,
) -> Result<Vec<SecurityCrateRow>, sqlx::Error> {
    sqlx::query_as::<_, SecurityCrateRow>(
        "SELECT id, name
         FROM crates
         WHERE ($3::BIGINT IS NULL
                OR security_checked_at IS NULL
                OR security_checked_at < NOW() - make_interval(secs => $3::BIGINT::DOUBLE \
         PRECISION))
         ORDER BY security_checked_at ASC NULLS FIRST, id DESC
         LIMIT $1 OFFSET $2",
    )
    .bind(limit)
    .bind(offset)
    .bind(max_age_secs)
    .fetch_all(db)
    .await
}

/// Stamps `security_checked_at = NOW()` on a crate after a successful
/// advisory sync.
pub async fn stamp_security_checked_at(db: &PgPool, crate_id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE crates SET security_checked_at = NOW() WHERE id = $1")
        .bind(crate_id)
        .execute(db)
        .await?;
    Ok(())
}

/// Loads all versions for a crate, used by advisory matching.
pub async fn fetch_security_versions_for_crate(
    db: &PgPool,
    crate_id: i64,
) -> Result<Vec<SecurityVersionRow>, sqlx::Error> {
    sqlx::query_as::<_, SecurityVersionRow>(
        "SELECT id, version
         FROM crate_versions
         WHERE crate_id = $1",
    )
    .bind(crate_id)
    .fetch_all(db)
    .await
}

/// Deletes OSV-derived advisory matches for a crate.
pub async fn clear_osv_advisory_matches(db: &PgPool, crate_id: i64) -> Result<(), sqlx::Error> {
    sqlx::query(
        "DELETE FROM advisory_matches
         WHERE crate_id = $1 AND source IN ('osv', 'rustsec_osv')",
    )
    .bind(crate_id)
    .execute(db)
    .await?;
    Ok(())
}

/// Deletes RustSec DB advisory matches for a crate.
pub async fn clear_rustsec_db_advisory_matches(
    db: &PgPool,
    crate_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "DELETE FROM advisory_matches
         WHERE crate_id = $1 AND source = 'rustsec_db'",
    )
    .bind(crate_id)
    .execute(db)
    .await?;
    Ok(())
}

/// Upserts a version-scoped advisory match row.
pub async fn upsert_version_advisory_match(
    db: &PgPool,
    advisory: &VersionAdvisoryMatchInsert<'_>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO advisory_matches (
            crate_id, version_id, advisory_id, severity, title, url,
            affected_range, fixed_versions, source, details,
            affected_functions, cwe_ids, created_at
         ) VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, NOW()
         )
         ON CONFLICT (crate_id, version_id, advisory_id)
         DO UPDATE SET
            severity = EXCLUDED.severity,
            title = EXCLUDED.title,
            url = EXCLUDED.url,
            affected_range = EXCLUDED.affected_range,
            fixed_versions = EXCLUDED.fixed_versions,
            source = EXCLUDED.source,
            details = EXCLUDED.details,
            affected_functions = EXCLUDED.affected_functions,
            cwe_ids = EXCLUDED.cwe_ids,
            created_at = NOW()",
    )
    .bind(advisory.crate_id)
    .bind(advisory.version_id)
    .bind(advisory.advisory_id)
    .bind(advisory.severity)
    .bind(advisory.title)
    .bind(advisory.url)
    .bind(advisory.affected_range)
    .bind(advisory.fixed_versions)
    .bind(advisory.source)
    .bind(advisory.details)
    .bind(advisory.affected_functions)
    .bind(advisory.cwe_ids)
    .execute(db)
    .await?;
    Ok(())
}

/// Inserts a crate-level advisory match row when no version-specific match
/// exists.
pub async fn insert_crate_level_advisory_match(
    db: &PgPool,
    advisory: &CrateLevelAdvisoryMatchInsert<'_>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO advisory_matches (
            crate_id, version_id, advisory_id, severity, title, url,
            affected_range, fixed_versions, source, details,
            affected_functions, cwe_ids, created_at
         ) VALUES (
             $1, NULL, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, NOW()
         )",
    )
    .bind(advisory.crate_id)
    .bind(advisory.advisory_id)
    .bind(advisory.severity)
    .bind(advisory.title)
    .bind(advisory.url)
    .bind(advisory.affected_range)
    .bind(advisory.fixed_versions)
    .bind(advisory.source)
    .bind(advisory.details)
    .bind(advisory.affected_functions)
    .bind(advisory.cwe_ids)
    .execute(db)
    .await?;
    Ok(())
}

/// Replaces all source-file-scoped index rows in one transaction.
pub async fn replace_source_file_index_rows(
    db: &PgPool,
    crate_version_id: i64,
    source_file_id: i64,
    index_source: &str,
    extraction: &IndexedExtractionBatch,
) -> Result<IndexedWriteCounts, sqlx::Error> {
    let mut tx = db.begin().await?;
    clear_source_file_index_rows(&mut tx, source_file_id).await?;
    let counts = insert_indexed_extraction_batch(
        &mut tx,
        crate_version_id,
        source_file_id,
        index_source,
        extraction,
    )
    .await?;
    tx.commit().await?;
    Ok(counts)
}

/// Replaces all crate-version rows for an index source in one transaction.
pub async fn replace_crate_version_index_rows(
    db: &PgPool,
    crate_version_id: i64,
    source_file_id: i64,
    index_source: &str,
    extraction: &IndexedExtractionBatch,
) -> Result<IndexedWriteCounts, sqlx::Error> {
    let mut tx = db.begin().await?;
    clear_crate_version_index_rows(&mut tx, crate_version_id, index_source).await?;
    let counts = insert_indexed_extraction_batch(
        &mut tx,
        crate_version_id,
        source_file_id,
        index_source,
        extraction,
    )
    .await?;
    tx.commit().await?;
    Ok(counts)
}

/// Prunes stale source-backed index rows and source files in one transaction.
pub async fn prune_source_file_index_and_files(
    db: &PgPool,
    crate_version_id: i64,
    seen_paths: &[String],
) -> Result<u64, sqlx::Error> {
    let mut tx = db.begin().await?;
    prune_stale_source_file_index_rows(&mut tx, crate_version_id, seen_paths).await?;
    let deleted =
        prune_source_files_for_crate_version(&mut tx, crate_version_id, seen_paths).await?;
    tx.commit().await?;
    Ok(deleted)
}

async fn ensure_crate_stub_tx(
    tx: &mut Transaction<'_, Postgres>,
    crate_name: &str,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO crates (name, categories, keywords, created_at, updated_at)
         VALUES ($1, '{}'::TEXT[], '{}'::TEXT[], NOW(), NOW())
         ON CONFLICT (name) DO UPDATE SET updated_at = NOW()
         RETURNING id",
    )
    .bind(crate_name)
    .fetch_one(&mut **tx)
    .await
}

/// Persists crate metadata, versions, features, readme, and dependencies in one
/// transaction.
pub async fn persist_crate_sync(
    db: &PgPool,
    detail: &CratesIoCrateDetailResponse,
    selected_version: Option<&str>,
    readme: Option<&str>,
    dependencies: Option<&[CratesIoDependency]>,
    categories: &[String],
    keywords: &[String],
) -> Result<PersistCrateSyncOutcome, sqlx::Error> {
    let mut tx = db.begin().await?;

    let crate_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO crates (
                 name, description, repository_url, docs_url, homepage_url, categories, keywords,
                 last_checked_at, next_check_at, ttl_hint_seconds, ttl_reason, last_refresh_error,
                 created_at, updated_at
         ) VALUES (
                 $1, $2, $3, $4, $5, $6, $7,
                 NOW(), NOW() + INTERVAL '1 day', 86400, 'sync_write', NULL,
                 NOW(), NOW()
         )
         ON CONFLICT (name) DO UPDATE SET
            description = EXCLUDED.description,
            repository_url = EXCLUDED.repository_url,
            docs_url = EXCLUDED.docs_url,
            homepage_url = EXCLUDED.homepage_url,
            categories = EXCLUDED.categories,
            keywords = EXCLUDED.keywords,
            last_checked_at = NOW(),
            next_check_at = NOW() + INTERVAL '1 day',
            ttl_hint_seconds = 86400,
            ttl_reason = 'sync_write',
            last_refresh_error = NULL,
            updated_at = NOW()
         RETURNING id",
    )
    .bind(&detail.krate.name)
    .bind(
        detail
            .krate
            .description
            .as_deref(),
    )
    .bind(
        detail
            .krate
            .repository
            .as_deref(),
    )
    .bind(
        detail
            .krate
            .documentation
            .as_deref(),
    )
    .bind(
        detail
            .krate
            .homepage
            .as_deref(),
    )
    .bind(categories)
    .bind(keywords)
    .fetch_one(&mut *tx)
    .await?;

    let mut version_ids = HashMap::new();
    for version in &detail.versions {
        let published_at = version
            .created_at
            .clone()
            .or(version.updated_at.clone());
        let version_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO crate_versions (
                crate_id, version, published_at, yanked, total_downloads, rust_version,
                checksum, license_expression, created_at, updated_at
             ) VALUES (
                $1, $2, $3::timestamptz, $4, $5, $6, $7, $8, NOW(), NOW()
             )
             ON CONFLICT (crate_id, version) DO UPDATE SET
                published_at = COALESCE(EXCLUDED.published_at, crate_versions.published_at),
                yanked = EXCLUDED.yanked,
                total_downloads = EXCLUDED.total_downloads,
                rust_version = COALESCE(EXCLUDED.rust_version, crate_versions.rust_version),
                checksum = COALESCE(EXCLUDED.checksum, crate_versions.checksum),
                license_expression = COALESCE(
                    EXCLUDED.license_expression,
                    crate_versions.license_expression
                ),
                updated_at = NOW()
             RETURNING id",
        )
        .bind(crate_id)
        .bind(&version.num)
        .bind(published_at)
        .bind(version.yanked)
        .bind(
            version
                .downloads
                .unwrap_or_default(),
        )
        .bind(
            version
                .rust_version
                .as_deref(),
        )
        .bind(version.checksum.as_deref())
        .bind(version.license.as_deref())
        .fetch_one(&mut *tx)
        .await?;

        sqlx::query("DELETE FROM crate_version_features WHERE crate_version_id = $1")
            .bind(version_id)
            .execute(&mut *tx)
            .await?;

        for (feature_name, enables) in &version.features {
            if feature_name.trim().is_empty() {
                continue;
            }

            let enables_json = Value::Array(
                enables
                    .iter()
                    .map(|value| Value::String(value.trim().to_string()))
                    .filter(|value| {
                        value
                            .as_str()
                            .map(|text| !text.is_empty())
                            .unwrap_or(false)
                    })
                    .collect::<Vec<_>>(),
            );

            sqlx::query(
                "INSERT INTO crate_version_features (
                    crate_version_id,
                    feature_name,
                    enables,
                    created_at,
                    updated_at
                 ) VALUES (
                    $1, $2, $3, NOW(), NOW()
                 )",
            )
            .bind(version_id)
            .bind(feature_name.trim())
            .bind(enables_json)
            .execute(&mut *tx)
            .await?;
        }

        version_ids.insert(version.num.clone(), version_id);
    }

    let mut dependencies_synced = 0_usize;
    if let Some(selected_version_name) = selected_version
        && let Some(selected_version_id) = version_ids
            .get(selected_version_name)
            .copied()
    {
        if let Some(readme_text) = readme {
            sqlx::query("UPDATE crate_versions SET readme = $1, updated_at = NOW() WHERE id = $2")
                .bind(readme_text)
                .bind(selected_version_id)
                .execute(&mut *tx)
                .await?;
        }

        if let Some(dependencies) = dependencies {
            sqlx::query("DELETE FROM dependency_edges WHERE from_version_id = $1")
                .bind(selected_version_id)
                .execute(&mut *tx)
                .await?;

            for dependency in dependencies {
                let dependency_name = dependency.crate_id.trim();
                if dependency_name.is_empty() {
                    continue;
                }

                let to_crate_id = ensure_crate_stub_tx(&mut tx, dependency_name).await?;
                let features = Value::Array(
                    dependency
                        .features
                        .iter()
                        .cloned()
                        .map(Value::String)
                        .collect::<Vec<_>>(),
                );
                let dependency_kind = dependency
                    .kind
                    .as_deref()
                    .unwrap_or("normal");

                sqlx::query(
                    "INSERT INTO dependency_edges (
                        from_version_id, to_crate_id, requirement, dependency_kind, optional,
                        features, created_at
                     ) VALUES (
                        $1, $2, $3, $4, $5, $6, NOW()
                     )",
                )
                .bind(selected_version_id)
                .bind(to_crate_id)
                .bind(&dependency.req)
                .bind(dependency_kind)
                .bind(dependency.optional)
                .bind(features)
                .execute(&mut *tx)
                .await?;

                dependencies_synced += 1;
            }
        }
    }

    tx.commit().await?;

    Ok(PersistCrateSyncOutcome {
        versions_synced: detail.versions.len(),
        dependencies_synced,
    })
}

/// Returns an existing pending/running refresh job for this crate+scope, or
/// inserts a new pending job and returns its id.
/// Result of an [`enqueue_or_get_refresh_job_id`] call, indicating whether a
/// new job was created or an existing pending/running job was returned.
#[derive(Debug, Clone, Copy)]
pub struct EnqueueOutcome {
    /// The job ID (new or existing).
    pub job_id: i64,
    /// `true` when a fresh row was inserted; `false` when a pre-existing
    /// pending/running job was returned.
    pub is_new: bool,
}

/// Idempotent job enqueue: returns the existing pending/running job if one
/// exists for the same `(crate_name, scope)`, otherwise inserts a new row.
pub async fn enqueue_or_get_refresh_job_id(
    db: &PgPool,
    crate_name: &str,
    scope: &str,
    priority: i32,
    include_dependencies: bool,
    payload: Value,
) -> Result<EnqueueOutcome, sqlx::Error> {
    if let Some(existing_id) = sqlx::query_scalar::<_, i64>(
        "SELECT id
         FROM refresh_jobs
         WHERE crate_name_lower = LOWER($1) AND scope = $2 AND status IN ('pending', 'running')
         ORDER BY id ASC
         LIMIT 1",
    )
    .bind(crate_name)
    .bind(scope)
    .fetch_optional(db)
    .await?
    {
        return Ok(EnqueueOutcome {
            job_id: existing_id,
            is_new: false,
        });
    }

    let job_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO refresh_jobs (
            crate_name, scope, priority, status, include_dependencies, payload, requested_at
         ) VALUES (
            $1, $2, $3, 'pending', $4, $5, NOW()
         )
         RETURNING id",
    )
    .bind(crate_name)
    .bind(scope)
    .bind(priority)
    .bind(include_dependencies)
    .bind(payload)
    .fetch_one(db)
    .await?;

    Ok(EnqueueOutcome { job_id, is_new: true })
}

/// Loads all coverage counters for `index_status`.
pub async fn fetch_index_coverage_counts(
    db: &PgPool,
) -> Result<IndexCoverageCountsRow, sqlx::Error> {
    sqlx::query_as::<_, IndexCoverageCountsRow>(
        "SELECT
            (SELECT COUNT(*)::BIGINT FROM crates) AS crates,
            (SELECT COUNT(*)::BIGINT FROM crate_versions) AS crate_versions,
            (SELECT COUNT(*)::BIGINT FROM dependency_edges) AS dependency_edges,
            (SELECT COUNT(*)::BIGINT FROM advisory_matches) AS advisory_matches,
            (SELECT COUNT(*)::BIGINT FROM source_files) AS source_files,
            (SELECT COUNT(*)::BIGINT FROM symbols) AS symbols,
            (SELECT COUNT(*)::BIGINT FROM docs_pages) AS docs_pages",
    )
    .fetch_one(db)
    .await
}

/// Loads queue counters for `index_status`.
pub async fn fetch_index_queue_counts(db: &PgPool) -> Result<IndexQueueCountsRow, sqlx::Error> {
    sqlx::query_as::<_, IndexQueueCountsRow>(
        "SELECT
            (SELECT COUNT(*)::BIGINT
             FROM refresh_jobs
             WHERE status = 'pending' AND requested_at <= NOW()) AS pending_jobs,
            (SELECT COUNT(*)::BIGINT
             FROM refresh_jobs
             WHERE status = 'pending' AND requested_at > NOW()) AS delayed_jobs,
            (SELECT COUNT(*)::BIGINT
             FROM refresh_jobs
             WHERE status = 'pending' AND attempts > 0) AS retrying_jobs,
            (SELECT COUNT(*)::BIGINT
             FROM refresh_jobs
             WHERE status = 'running') AS running_jobs,
            (SELECT COUNT(*)::BIGINT
             FROM refresh_jobs
             WHERE status = 'failed') AS failed_jobs",
    )
    .fetch_one(db)
    .await
}

/// Loads refresh retry-attempt distribution for `index_status`.
pub async fn fetch_refresh_job_retry_distribution(
    db: &PgPool,
) -> Result<RefreshJobRetryDistributionRow, sqlx::Error> {
    sqlx::query_as::<_, RefreshJobRetryDistributionRow>(
        "SELECT
            COUNT(*) FILTER (WHERE status IN ('pending', 'running') AND attempts = 1)::BIGINT
                AS inflight_attempt_1,
            COUNT(*) FILTER (WHERE status IN ('pending', 'running') AND attempts = 2)::BIGINT
                AS inflight_attempt_2,
            COUNT(*) FILTER (WHERE status IN ('pending', 'running') AND attempts >= 3)::BIGINT
                AS inflight_attempt_3_plus,
            COUNT(*) FILTER (WHERE status = 'failed' AND attempts = 1)::BIGINT
                AS failed_attempt_1,
            COUNT(*) FILTER (WHERE status = 'failed' AND attempts = 2)::BIGINT
                AS failed_attempt_2,
            COUNT(*) FILTER (WHERE status = 'failed' AND attempts >= 3)::BIGINT
                AS failed_attempt_3_plus
         FROM refresh_jobs",
    )
    .fetch_one(db)
    .await
}

/// Loads failure counts grouped by scope.
pub async fn fetch_index_failures_by_scope(
    db: &PgPool,
) -> Result<Vec<IndexFailureByScopeRow>, sqlx::Error> {
    sqlx::query_as::<_, IndexFailureByScopeRow>(
        "SELECT
            scope,
            COUNT(*)::BIGINT AS failed_jobs
         FROM refresh_jobs
         WHERE status = 'failed'
         GROUP BY scope
         ORDER BY failed_jobs DESC, scope ASC",
    )
    .fetch_all(db)
    .await
}

/// Loads latest non-empty refresh job errors.
pub async fn fetch_recent_refresh_job_errors(
    db: &PgPool,
    limit: i64,
) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar::<_, String>(
        "SELECT last_error
         FROM refresh_jobs
         WHERE last_error IS NOT NULL AND last_error <> ''
         ORDER BY finished_at DESC NULLS LAST, id DESC
         LIMIT $1",
    )
    .bind(limit)
    .fetch_all(db)
    .await
}

/// Loads freshness timestamps for `index_status`.
pub async fn fetch_index_freshness(db: &PgPool) -> Result<IndexFreshnessRow, sqlx::Error> {
    sqlx::query_as::<_, IndexFreshnessRow>(
        "SELECT
            (SELECT MAX(updated_at)::TEXT FROM crates) AS crates_updated_at,
            (SELECT MAX(indexed_at)::TEXT FROM source_files) AS source_indexed_at,
            (SELECT MAX(indexed_at)::TEXT FROM symbols) AS symbols_indexed_at,
            (SELECT MAX(indexed_at)::TEXT FROM docs_pages) AS docs_indexed_at,
            (SELECT MAX(created_at)::TEXT FROM advisory_matches) AS advisories_updated_at",
    )
    .fetch_one(db)
    .await
}

/// Loads operational telemetry metrics over the last 24h window.
pub async fn fetch_index_operational_metrics_24h(
    db: &PgPool,
) -> Result<IndexOperationalMetricsRow, sqlx::Error> {
    sqlx::query_as::<_, IndexOperationalMetricsRow>(
        "SELECT
            (SELECT COUNT(*)::BIGINT
             FROM tool_invocations
             WHERE created_at >= NOW() - INTERVAL '24 hours') AS query_count,
            (SELECT AVG(latency_ms)::DOUBLE PRECISION
             FROM tool_invocations
             WHERE created_at >= NOW() - INTERVAL '24 hours') AS average_latency_ms,
            (SELECT
                CASE WHEN COUNT(*) = 0 THEN NULL
                     ELSE (COUNT(*) FILTER (WHERE success = FALSE))::DOUBLE PRECISION / COUNT(*)
                END
             FROM tool_invocations
             WHERE created_at >= NOW() - INTERVAL '24 hours') AS error_rate,
            (SELECT
                CASE WHEN COUNT(*) = 0 THEN NULL
                     ELSE (COUNT(*) FILTER (WHERE hit = TRUE))::DOUBLE PRECISION / COUNT(*)
                END
             FROM query_cache_events
             WHERE created_at >= NOW() - INTERVAL '24 hours') AS cache_hit_rate,
            (WITH latest AS (
                 SELECT GREATEST(
                     COALESCE((SELECT MAX(updated_at) FROM crates), TO_TIMESTAMP(0)),
                     COALESCE((SELECT MAX(indexed_at) FROM source_files), TO_TIMESTAMP(0)),
                     COALESCE((SELECT MAX(indexed_at) FROM symbols), TO_TIMESTAMP(0)),
                     COALESCE((SELECT MAX(indexed_at) FROM docs_pages), TO_TIMESTAMP(0)),
                     COALESCE((SELECT MAX(created_at) FROM advisory_matches), TO_TIMESTAMP(0))
                 ) AS latest_ts
             )
             SELECT EXTRACT(EPOCH FROM (NOW() - latest_ts))::BIGINT
             FROM latest) AS index_lag_seconds",
    )
    .fetch_one(db)
    .await
}

/// Loads completed refresh duration stats over the last 30 days for one scope.
pub async fn fetch_refresh_eta_stats(
    db: &PgPool,
    scope_key: &str,
) -> Result<(i64, Option<f64>), sqlx::Error> {
    sqlx::query_as::<_, (i64, Option<f64>)>(
        "SELECT
            COUNT(*)::BIGINT,
            AVG(EXTRACT(EPOCH FROM (finished_at - started_at)))::DOUBLE PRECISION
         FROM refresh_jobs
         WHERE scope = $1
           AND status = 'finished'
           AND started_at IS NOT NULL
           AND finished_at IS NOT NULL
           AND finished_at >= NOW() - INTERVAL '30 days'",
    )
    .bind(scope_key)
    .fetch_one(db)
    .await
}

/// Returns whether a crate is due for a freshness check.
pub async fn is_crate_refresh_due(db: &PgPool, crate_id: i64) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar::<_, bool>(
        "SELECT COALESCE(next_check_at IS NULL OR next_check_at <= NOW(), TRUE)
         FROM crates
         WHERE id = $1",
    )
    .bind(crate_id)
    .fetch_one(db)
    .await
}

/// Counts releases for a crate over the last 365 days.
pub async fn count_crate_releases_last_year(
    db: &PgPool,
    crate_id: i64,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*)::BIGINT
         FROM crate_versions
         WHERE crate_id = $1
           AND published_at >= NOW() - INTERVAL '365 days'",
    )
    .bind(crate_id)
    .fetch_one(db)
    .await
}

/// Returns days since latest release for a crate, if any releases exist.
pub async fn days_since_latest_crate_release(
    db: &PgPool,
    crate_id: i64,
) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar::<_, Option<i64>>(
        "SELECT EXTRACT(EPOCH FROM (NOW() - MAX(published_at)))::BIGINT / 86400
         FROM crate_versions
         WHERE crate_id = $1",
    )
    .bind(crate_id)
    .fetch_one(db)
    .await
}

/// Persists a failed freshness probe and schedules a short retry TTL.
pub async fn mark_crate_probe_failed(
    db: &PgPool,
    crate_id: i64,
    error_message: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE crates
         SET last_checked_at = NOW(),
             next_check_at = NOW() + INTERVAL '1 hour',
             ttl_hint_seconds = 3600,
             ttl_reason = 'probe_failed',
             last_refresh_error = $1,
             updated_at = NOW()
         WHERE id = $2",
    )
    .bind(error_message)
    .bind(crate_id)
    .execute(db)
    .await?;
    Ok(())
}

/// Persists a successful freshness check with the computed TTL policy.
pub async fn mark_crate_freshness_checked(
    db: &PgPool,
    crate_id: i64,
    ttl_seconds: i32,
    ttl_reason: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE crates
         SET last_checked_at = NOW(),
             next_check_at = NOW() + ($1 * INTERVAL '1 second'),
             ttl_hint_seconds = $1,
             ttl_reason = $2,
             last_refresh_error = NULL,
             updated_at = NOW()
         WHERE id = $3",
    )
    .bind(ttl_seconds)
    .bind(ttl_reason)
    .bind(crate_id)
    .execute(db)
    .await?;
    Ok(())
}

/// Loads refresh queue gauge counters used by the worker loop.
///
/// The `background_pending_jobs` field counts pending jobs with
/// `priority >= 50` (i.e. `PRIORITY_DISCOVERY` and above — background
/// discovery/enrichment work as opposed to on-demand requests).
pub async fn fetch_refresh_job_gauge_counts(
    db: &PgPool,
) -> Result<RefreshJobGaugeCountsRow, sqlx::Error> {
    sqlx::query_as::<_, RefreshJobGaugeCountsRow>(
        "SELECT
            COUNT(*) FILTER (WHERE status = 'pending')::BIGINT            AS pending_jobs,
            COUNT(*) FILTER (WHERE status = 'running')::BIGINT            AS running_jobs,
            COUNT(*) FILTER (WHERE status = 'failed')::BIGINT             AS failed_jobs,
            COUNT(*) FILTER (WHERE status = 'pending'
                                 AND priority >= 50)::BIGINT              AS \
         background_pending_jobs
         FROM refresh_jobs",
    )
    .fetch_one(db)
    .await
}

/// Returns the count of pending refresh jobs grouped by `scope`.
/// Only scopes with at least one pending job are included.
pub async fn fetch_refresh_job_scope_pending_counts(
    db: &PgPool,
) -> Result<Vec<(String, i64)>, sqlx::Error> {
    sqlx::query_as::<_, (String, i64)>(
        "SELECT scope, COUNT(*)::BIGINT
         FROM refresh_jobs
         WHERE status = 'pending'
         GROUP BY scope",
    )
    .fetch_all(db)
    .await
}

/// Atomically claims the next runnable refresh job.
pub async fn claim_next_refresh_job(
    db: &PgPool,
) -> Result<Option<RefreshJobQueueRow>, sqlx::Error> {
    sqlx::query_as::<_, RefreshJobQueueRow>(
        "WITH next_job AS (
             SELECT id
             FROM refresh_jobs
             WHERE status = 'pending'
               AND requested_at <= NOW()
             ORDER BY priority ASC, attempts ASC, requested_at ASC, id ASC
             FOR UPDATE SKIP LOCKED
             LIMIT 1
         )
         UPDATE refresh_jobs r
         SET status = 'running',
             started_at = NOW(),
             finished_at = NULL,
             attempts = r.attempts + 1,
             last_error = NULL
         FROM next_job
         WHERE r.id = next_job.id
         RETURNING r.id, r.crate_name, r.scope, r.include_dependencies, r.payload, r.attempts",
    )
    .fetch_optional(db)
    .await
}

/// Marks a claimed refresh job as finished.
pub async fn mark_refresh_job_finished(db: &PgPool, job_id: i64) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE refresh_jobs
         SET status = 'finished',
             finished_at = NOW(),
             last_error = NULL
         WHERE id = $1",
    )
    .bind(job_id)
    .execute(db)
    .await?;
    Ok(())
}

/// Marks a refresh job failed or re-queues it with backoff.
pub async fn mark_refresh_job_failed_or_requeued(
    db: &PgPool,
    job_id: i64,
    terminal: bool,
    retry_delay_seconds: i64,
    error_message: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE refresh_jobs
         SET status = CASE WHEN $1 THEN 'failed' ELSE 'pending' END,
             requested_at = CASE WHEN $1 THEN requested_at ELSE NOW() + ($2 * INTERVAL '1 second') \
         END,
             finished_at = CASE WHEN $1 THEN NOW() ELSE NULL END,
             last_error = $3
         WHERE id = $4",
    )
    .bind(terminal)
    .bind(retry_delay_seconds)
    .bind(error_message)
    .bind(job_id)
    .execute(db)
    .await?;
    Ok(())
}

/// Writes the latest refresh error onto the crate metadata row.
pub async fn mark_crate_refresh_error(
    db: &PgPool,
    crate_name: &str,
    error_message: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE crates
         SET last_refresh_error = $1,
             updated_at = NOW()
         WHERE name_lower = LOWER($2)",
    )
    .bind(error_message)
    .bind(crate_name)
    .execute(db)
    .await?;
    Ok(())
}

/// Deletes source-file scoped symbol/type/impl rows.
pub async fn clear_source_file_index_rows(
    tx: &mut Transaction<'_, Postgres>,
    source_file_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM symbols WHERE source_file_id = $1")
        .bind(source_file_id)
        .execute(&mut **tx)
        .await?;

    sqlx::query("DELETE FROM crate_types WHERE source_file_id = $1")
        .bind(source_file_id)
        .execute(&mut **tx)
        .await?;

    sqlx::query("DELETE FROM crate_impls WHERE source_file_id = $1")
        .bind(source_file_id)
        .execute(&mut **tx)
        .await?;

    Ok(())
}

/// Deletes crate-version scoped index rows for one index source.
pub async fn clear_crate_version_index_rows(
    tx: &mut Transaction<'_, Postgres>,
    crate_version_id: i64,
    index_source: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "DELETE FROM symbols
         WHERE crate_version_id = $1 AND index_source = $2",
    )
    .bind(crate_version_id)
    .bind(index_source)
    .execute(&mut **tx)
    .await?;

    sqlx::query(
        "DELETE FROM crate_types
         WHERE crate_version_id = $1 AND index_source = $2",
    )
    .bind(crate_version_id)
    .bind(index_source)
    .execute(&mut **tx)
    .await?;

    sqlx::query(
        "DELETE FROM crate_impls
         WHERE crate_version_id = $1 AND index_source = $2",
    )
    .bind(crate_version_id)
    .bind(index_source)
    .execute(&mut **tx)
    .await?;

    sqlx::query(
        "DELETE FROM crate_traits
         WHERE crate_version_id = $1 AND index_source = $2",
    )
    .bind(crate_version_id)
    .bind(index_source)
    .execute(&mut **tx)
    .await?;

    Ok(())
}

/// Inserts a full extraction batch into index tables.
pub async fn insert_indexed_extraction_batch(
    tx: &mut Transaction<'_, Postgres>,
    crate_version_id: i64,
    source_file_id: i64,
    index_source: &str,
    extraction: &IndexedExtractionBatch,
) -> Result<IndexedWriteCounts, sqlx::Error> {
    let mut counts = IndexedWriteCounts::default();

    for symbol in &extraction.symbols {
        sqlx::query(
            "INSERT INTO symbols (
                crate_version_id,
                source_file_id,
                name,
                kind,
                signature,
                visibility,
                start_line,
                end_line,
                index_source,
                indexed_at,
                rustdoc_item_id,
                canonical_path,
                definition_path,
                deprecated_since,
                deprecated_note,
                docs,
                attrs
             ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, NOW(),
                $10, $11, $12, $13, $14, $15, $16
             )",
        )
        .bind(crate_version_id)
        .bind(source_file_id)
        .bind(&symbol.name)
        .bind(&symbol.kind)
        .bind(&symbol.signature)
        .bind(&symbol.visibility)
        .bind(symbol.start_line)
        .bind(symbol.end_line)
        .bind(index_source)
        .bind(symbol.rustdoc_item_id)
        .bind(&symbol.canonical_path)
        .bind(&symbol.definition_path)
        .bind(&symbol.deprecated_since)
        .bind(&symbol.deprecated_note)
        .bind(&symbol.docs)
        .bind(&symbol.attrs)
        .execute(&mut **tx)
        .await?;
        counts.symbols += 1;
    }

    // syn parsing does not evaluate #[cfg] attributes, so conditional
    // compilation can produce multiple items with the same (type_name, kind)
    // in a single file.  DO NOTHING keeps the first definition and skips
    // duplicates — good enough for the gap-filler role syn data plays until
    // authoritative rustdoc JSON is available for the crate version.
    for extracted_type in &extraction.types {
        sqlx::query(
            "INSERT INTO crate_types (
                crate_version_id,
                source_file_id,
                type_name,
                kind,
                visibility,
                generic_params,
                fields,
                variants,
                start_line,
                end_line,
                index_source,
                indexed_at,
                rustdoc_item_id,
                canonical_path,
                definition_path,
                deprecated_since,
                deprecated_note,
                is_non_exhaustive,
                auto_traits,
                where_clauses,
                docs,
                attrs
             ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                $11, NOW(), $12, $13, $14, $15, $16, $17, $18, $19,
                $20, $21
             )
             ON CONFLICT (crate_version_id, source_file_id, type_name, kind)
                WHERE index_source != 'rustdoc_json'
             DO NOTHING",
        )
        .bind(crate_version_id)
        .bind(source_file_id)
        .bind(&extracted_type.type_name)
        .bind(&extracted_type.kind)
        .bind(&extracted_type.visibility)
        .bind(&extracted_type.generic_params)
        .bind(&extracted_type.fields)
        .bind(&extracted_type.variants)
        .bind(extracted_type.start_line)
        .bind(extracted_type.end_line)
        .bind(index_source)
        .bind(extracted_type.rustdoc_item_id)
        .bind(&extracted_type.canonical_path)
        .bind(&extracted_type.definition_path)
        .bind(&extracted_type.deprecated_since)
        .bind(&extracted_type.deprecated_note)
        .bind(extracted_type.is_non_exhaustive)
        .bind(&extracted_type.auto_traits)
        .bind(&extracted_type.where_clauses)
        .bind(&extracted_type.docs)
        .bind(&extracted_type.attrs)
        .execute(&mut **tx)
        .await?;
        counts.types += 1;
    }

    for extracted_impl in &extraction.impls {
        sqlx::query(
            "INSERT INTO crate_impls (
                crate_version_id,
                source_file_id,
                type_name,
                type_name_display,
                trait_name,
                trait_name_display,
                impl_kind,
                methods,
                start_line,
                end_line,
                index_source,
                indexed_at,
                rustdoc_item_id,
                is_blanket,
                is_synthetic,
                is_negative,
                blanket_type,
                generics,
                where_clauses,
                docs
             ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                $11, NOW(), $12, $13, $14, $15, $16, $17, $18, $19
             )",
        )
        .bind(crate_version_id)
        .bind(source_file_id)
        .bind(&extracted_impl.type_name)
        .bind(&extracted_impl.type_name_display)
        .bind(&extracted_impl.trait_name)
        .bind(&extracted_impl.trait_name_display)
        .bind(&extracted_impl.impl_kind)
        .bind(&extracted_impl.methods)
        .bind(extracted_impl.start_line)
        .bind(extracted_impl.end_line)
        .bind(index_source)
        .bind(extracted_impl.rustdoc_item_id)
        .bind(extracted_impl.is_blanket)
        .bind(extracted_impl.is_synthetic)
        .bind(extracted_impl.is_negative)
        .bind(&extracted_impl.blanket_type)
        .bind(&extracted_impl.generics)
        .bind(&extracted_impl.where_clauses)
        .bind(&extracted_impl.docs)
        .execute(&mut **tx)
        .await?;
        counts.impls += 1;
    }

    for extracted_trait in &extraction.traits {
        sqlx::query(
            "INSERT INTO crate_traits (
                crate_version_id,
                trait_name,
                is_auto,
                is_unsafe,
                is_dyn_compatible,
                supertraits,
                required_methods,
                provided_methods,
                associated_types,
                generics,
                index_source,
                indexed_at,
                rustdoc_item_id,
                docs
             ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                $11, NOW(), $12, $13
             )",
        )
        .bind(crate_version_id)
        .bind(&extracted_trait.trait_name)
        .bind(extracted_trait.is_auto)
        .bind(extracted_trait.is_unsafe)
        .bind(extracted_trait.is_dyn_compatible)
        .bind(&extracted_trait.supertraits)
        .bind(&extracted_trait.required_methods)
        .bind(&extracted_trait.provided_methods)
        .bind(&extracted_trait.associated_types)
        .bind(&extracted_trait.generics)
        .bind(index_source)
        .bind(extracted_trait.rustdoc_item_id)
        .bind(&extracted_trait.docs)
        .execute(&mut **tx)
        .await?;
        counts.traits += 1;
    }

    Ok(counts)
}

/// Deletes stale source-file-backed index rows for a crate version.
pub async fn prune_stale_source_file_index_rows(
    tx: &mut Transaction<'_, Postgres>,
    crate_version_id: i64,
    seen_paths: &[String],
) -> Result<(), sqlx::Error> {
    if seen_paths.is_empty() {
        sqlx::query(
            "DELETE FROM symbols
             WHERE crate_version_id = $1
               AND source_file_id IN (
                 SELECT id FROM source_files WHERE crate_version_id = $1
               )",
        )
        .bind(crate_version_id)
        .execute(&mut **tx)
        .await?;

        sqlx::query(
            "DELETE FROM crate_types
             WHERE crate_version_id = $1
               AND source_file_id IN (
                 SELECT id FROM source_files WHERE crate_version_id = $1
               )",
        )
        .bind(crate_version_id)
        .execute(&mut **tx)
        .await?;

        sqlx::query(
            "DELETE FROM crate_impls
             WHERE crate_version_id = $1
               AND source_file_id IN (
                 SELECT id FROM source_files WHERE crate_version_id = $1
               )",
        )
        .bind(crate_version_id)
        .execute(&mut **tx)
        .await?;
    } else {
        sqlx::query(
            "DELETE FROM symbols
             WHERE crate_version_id = $1
               AND source_file_id IN (
                 SELECT id
                 FROM source_files
                 WHERE crate_version_id = $1
                   AND NOT (path = ANY($2::TEXT[]))
               )",
        )
        .bind(crate_version_id)
        .bind(seen_paths)
        .execute(&mut **tx)
        .await?;

        sqlx::query(
            "DELETE FROM crate_types
             WHERE crate_version_id = $1
               AND source_file_id IN (
                 SELECT id
                 FROM source_files
                 WHERE crate_version_id = $1
                   AND NOT (path = ANY($2::TEXT[]))
               )",
        )
        .bind(crate_version_id)
        .bind(seen_paths)
        .execute(&mut **tx)
        .await?;

        sqlx::query(
            "DELETE FROM crate_impls
             WHERE crate_version_id = $1
               AND source_file_id IN (
                 SELECT id
                 FROM source_files
                 WHERE crate_version_id = $1
                   AND NOT (path = ANY($2::TEXT[]))
               )",
        )
        .bind(crate_version_id)
        .bind(seen_paths)
        .execute(&mut **tx)
        .await?;
    }

    Ok(())
}

/// Returns the set of all crate names currently in the database.
///
/// Used by the registry discovery scanner to identify which crates have
/// already been indexed and can be skipped.
pub async fn fetch_known_crate_names(db: &PgPool) -> Result<HashSet<String>, sqlx::Error> {
    let rows: Vec<(String,)> = sqlx::query_as("SELECT name FROM crates")
        .fetch_all(db)
        .await?;
    Ok(rows
        .into_iter()
        .map(|(name,)| name)
        .collect())
}

/// Deletes stale source file rows for a crate version.
pub async fn prune_source_files_for_crate_version(
    tx: &mut Transaction<'_, Postgres>,
    crate_version_id: i64,
    seen_paths: &[String],
) -> Result<u64, sqlx::Error> {
    let result = if seen_paths.is_empty() {
        sqlx::query("DELETE FROM source_files WHERE crate_version_id = $1")
            .bind(crate_version_id)
            .execute(&mut **tx)
            .await?
    } else {
        sqlx::query(
            "DELETE FROM source_files
             WHERE crate_version_id = $1
               AND NOT (path = ANY($2::TEXT[]))",
        )
        .bind(crate_version_id)
        .bind(seen_paths)
        .execute(&mut **tx)
        .await?
    };

    Ok(result.rows_affected())
}

// ── GitHub repo metadata ─────────────────────────────────────────────

/// Fetches stored GitHub repo metadata for a crate, if present.
pub async fn fetch_github_metadata(
    db: &PgPool,
    crate_id: i64,
) -> Result<Option<GitHubMetadataRow>, sqlx::Error> {
    sqlx::query_as::<_, GitHubMetadataRow>(
        "SELECT owner, repo, stargazers_count, forks_count, open_issues_count,
                archived, pushed_at::TEXT, license_spdx, contributor_count,
                last_commit_at::TEXT, last_commit_message, recent_commit_count,
                fetched_at::TEXT
         FROM github_repo_metadata
         WHERE crate_id = $1",
    )
    .bind(crate_id)
    .fetch_optional(db)
    .await
}

/// Upserts GitHub repo metadata for a crate.
pub async fn upsert_github_metadata(
    db: &PgPool,
    crate_id: i64,
    meta: &GitHubMetadataInsert<'_>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO github_repo_metadata
            (crate_id, owner, repo, stargazers_count, forks_count,
             open_issues_count, archived, pushed_at, license_spdx,
             contributor_count, fetched_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8::TIMESTAMPTZ, $9, $10, NOW())
         ON CONFLICT (crate_id)
         DO UPDATE SET
            stargazers_count = EXCLUDED.stargazers_count,
            forks_count = EXCLUDED.forks_count,
            open_issues_count = EXCLUDED.open_issues_count,
            archived = EXCLUDED.archived,
            pushed_at = EXCLUDED.pushed_at,
            license_spdx = EXCLUDED.license_spdx,
            contributor_count = EXCLUDED.contributor_count,
            fetched_at = NOW()",
    )
    .bind(crate_id)
    .bind(meta.owner)
    .bind(meta.repo)
    .bind(meta.stargazers_count)
    .bind(meta.forks_count)
    .bind(meta.open_issues_count)
    .bind(meta.archived)
    .bind(meta.pushed_at)
    .bind(meta.license_spdx)
    .bind(meta.contributor_count)
    .execute(db)
    .await?;
    Ok(())
}

/// Updates the git-probe commit liveness columns for a crate's GitHub
/// metadata row. The row must already exist (upserted via API metadata first).
pub async fn update_git_probe_data(
    db: &PgPool,
    crate_id: i64,
    last_commit_at: Option<&str>,
    last_commit_message: Option<&str>,
    recent_commit_count: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE github_repo_metadata
         SET last_commit_at = $2::TIMESTAMPTZ,
             last_commit_message = $3,
             recent_commit_count = $4
         WHERE crate_id = $1",
    )
    .bind(crate_id)
    .bind(last_commit_at)
    .bind(last_commit_message)
    .bind(recent_commit_count)
    .execute(db)
    .await?;
    Ok(())
}

/// Fetches crates that have a GitHub repository URL but no recent metadata.
pub async fn fetch_crates_needing_github_metadata(
    db: &PgPool,
    max_age_secs: i64,
    limit: i64,
) -> Result<Vec<GitHubCandidateRow>, sqlx::Error> {
    sqlx::query_as::<_, GitHubCandidateRow>(
        "SELECT c.id AS crate_id, c.name AS crate_name, c.repository_url
         FROM crates c
         LEFT JOIN github_repo_metadata g ON g.crate_id = c.id
         WHERE c.repository_url LIKE '%github.com/%'
           AND (g.id IS NULL
                OR g.fetched_at < NOW() - make_interval(secs => $1::DOUBLE PRECISION))
         ORDER BY c.id
         LIMIT $2",
    )
    .bind(max_age_secs)
    .bind(limit)
    .fetch_all(db)
    .await
}

/// Clears GHSA-sourced advisory matches for a crate before re-inserting fresh
/// data.
pub async fn clear_ghsa_advisory_matches(db: &PgPool, crate_id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM advisory_matches WHERE crate_id = $1 AND source = 'ghsa'")
        .bind(crate_id)
        .execute(db)
        .await?;
    Ok(())
}

// ── GitHub releases ─────────────────────────────────────────────────

/// A release tuple: `(tag_name, release_name, body, published_at, prerelease)`.
type ReleaseTuple = (String, Option<String>, Option<String>, Option<String>, bool);

/// Upserts a batch of GitHub releases for a crate, replacing stale entries.
pub async fn upsert_github_releases(
    db: &PgPool,
    crate_id: i64,
    releases: &[ReleaseTuple],
) -> Result<(), sqlx::Error> {
    // Clear existing releases for this crate, then bulk insert.
    sqlx::query("DELETE FROM github_releases WHERE crate_id = $1")
        .bind(crate_id)
        .execute(db)
        .await?;

    for (tag_name, release_name, body, published_at, prerelease) in releases {
        sqlx::query(
            "INSERT INTO github_releases
                (crate_id, tag_name, release_name, body, published_at, prerelease, fetched_at)
             VALUES ($1, $2, $3, $4, $5::TIMESTAMPTZ, $6, NOW())
             ON CONFLICT (crate_id, tag_name) DO UPDATE SET
                release_name = EXCLUDED.release_name,
                body = EXCLUDED.body,
                published_at = EXCLUDED.published_at,
                prerelease = EXCLUDED.prerelease,
                fetched_at = NOW()",
        )
        .bind(crate_id)
        .bind(tag_name)
        .bind(release_name)
        .bind(body)
        .bind(published_at.as_deref())
        .bind(prerelease)
        .execute(db)
        .await?;
    }

    Ok(())
}

/// Fetches stored GitHub releases for a crate, ordered by published_at
/// descending.
pub async fn fetch_github_releases(
    db: &PgPool,
    crate_id: i64,
    limit: i64,
) -> Result<Vec<GitHubReleaseRow>, sqlx::Error> {
    sqlx::query_as::<_, GitHubReleaseRow>(
        "SELECT tag_name, release_name, body, published_at::TEXT, prerelease
         FROM github_releases
         WHERE crate_id = $1
         ORDER BY published_at DESC NULLS LAST
         LIMIT $2",
    )
    .bind(crate_id)
    .bind(limit)
    .fetch_all(db)
    .await
}
