use std::collections::HashMap;

use metrics::gauge;
use reqwest::header::CONTENT_TYPE;
use rmcp::Json;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use sqlx::{Postgres, Transaction};
use tokio::time::{Duration, sleep};
use tracing::{error, warn};

use super::models::{
    CratesIoCrateDetailResponse, CratesIoDependenciesResponse, CratesIoDependency,
    CratesIoSearchResponse, IndexCoverage, IndexFailureByScope, IndexFreshness,
    IndexOperationalMetrics, IndexQueue, IndexRefreshRequest, IndexRefreshResponse,
    IndexRefreshResult, IndexRefreshScope, IndexRetryDistribution, IndexStatusRequest,
    IndexStatusResponse, IndexSyncCratesRequest, IndexSyncCratesResponse, ResponseFreshnessSource,
    SyncCrateOutcome,
};
use super::server::McpServer;
use super::utils::{
    DEFAULT_SYNC_QUERY, dedupe_strings, normalize_optional, normalize_required, sync_page,
    sync_per_page,
};
use crate::state::{AppState, OutboundSource};

fn now_epoch_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default()
}

#[derive(Debug, sqlx::FromRow)]
struct RefreshJobRow {
    id: i64,
    crate_name: String,
    scope: String,
    include_dependencies: bool,
    payload: Value,
    attempts: i32,
}

#[derive(Debug, Default, Deserialize)]
struct RefreshJobPayload {
    crate_name: Option<String>,
    query: Option<String>,
    page: Option<u32>,
    per_page: Option<u32>,
    include_dependencies: Option<bool>,
}

fn optional_job_crate_name(
    job_crate_name: &str,
    payload_crate_name: Option<String>,
) -> Option<String> {
    let normalized_payload = payload_crate_name.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() || trimmed == "*" || trimmed.eq_ignore_ascii_case("all") {
            None
        } else {
            Some(trimmed.to_string())
        }
    });
    if normalized_payload.is_some() {
        return normalized_payload;
    }

    let trimmed = job_crate_name.trim();
    if trimmed.is_empty() || trimmed == "*" || trimmed.eq_ignore_ascii_case("all") {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[derive(Debug, sqlx::FromRow)]
struct RetryDistributionRow {
    inflight_attempt_1: i64,
    inflight_attempt_2: i64,
    inflight_attempt_3_plus: i64,
    failed_attempt_1: i64,
    failed_attempt_2: i64,
    failed_attempt_3_plus: i64,
}

fn jittered_retry_delay_seconds(job_id: i64, attempts: i32) -> i64 {
    let base = 5_i64;
    let exponent = i64::from((attempts.saturating_sub(1)).clamp(0, 5));
    let backoff = (base * (1_i64 << exponent)).min(300);

    let now_nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or_default() as i64;
    let seed = now_nanos.wrapping_add(job_id.wrapping_mul(97));
    let jitter_percent = (seed.rem_euclid(31)) - 15;

    let jittered = backoff + ((backoff * jitter_percent) / 100);
    jittered.clamp(1, 600)
}

pub(crate) async fn run_refresh_worker(state: AppState) {
    const MAX_ATTEMPTS: i32 = 3;

    loop {
        // Update refresh job gauges for Prometheus on each iteration.
        if let Ok(counts) = sqlx::query_as::<_, (i64, i64, i64)>(
            "SELECT
                COUNT(*) FILTER (WHERE status = 'pending')::BIGINT,
                COUNT(*) FILTER (WHERE status = 'running')::BIGINT,
                COUNT(*) FILTER (WHERE status = 'failed')::BIGINT
             FROM refresh_jobs",
        )
        .fetch_one(&state.db)
        .await
        {
            gauge!("rust_mcp_refresh_jobs_pending").set(counts.0 as f64);
            gauge!("rust_mcp_refresh_jobs_running").set(counts.1 as f64);
            gauge!("rust_mcp_refresh_jobs_failed").set(counts.2 as f64);
        }

        let next_job = sqlx::query_as::<_, RefreshJobRow>(
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
        .fetch_optional(&state.db)
        .await;

        let Some(job) = (match next_job {
            Ok(value) => value,
            Err(error) => {
                error!(%error, "refresh worker failed to dequeue job");
                sleep(Duration::from_secs(2)).await;
                continue;
            }
        }) else {
            sleep(Duration::from_secs(2)).await;
            continue;
        };

        let server = McpServer::new(state.clone());
        let payload =
            serde_json::from_value::<RefreshJobPayload>(job.payload.clone()).unwrap_or_default();
        let result = match job.scope.as_str() {
            "crate" | "crate_deep_refresh" => server
                .sync_single_crate(&job.crate_name, job.include_dependencies)
                .await
                .map(|_| ()),
            "all" => server
                .handle_index_sync_crates(IndexSyncCratesRequest {
                    query: payload.query,
                    page: payload.page,
                    per_page: payload.per_page,
                    include_dependencies: payload
                        .include_dependencies
                        .or(Some(job.include_dependencies)),
                })
                .await
                .map(|_| ()),
            "security" => {
                let page = sync_page(payload.page);
                let per_page = sync_per_page(payload.per_page);
                let offset = page.saturating_sub(1) * per_page;
                match server
                    .sync_osv_security(per_page, offset)
                    .await
                {
                    Ok(_) => server
                        .sync_rustsec_db_security(per_page, offset)
                        .await
                        .map(|_| ()),
                    Err(error) => Err(error),
                }
            }
            "docs" => server
                .sync_docs_pages(
                    optional_job_crate_name(&job.crate_name, payload.crate_name),
                    payload.page,
                    payload.per_page,
                )
                .await
                .map(|_| ()),
            "local_cache" => server
                .sync_local_source_cache(
                    optional_job_crate_name(&job.crate_name, payload.crate_name),
                    payload.query,
                    payload.page,
                    payload.per_page,
                )
                .await
                .map(|_| ()),
            "rustdoc_json" => server
                .sync_rustdoc_json_cache(
                    optional_job_crate_name(&job.crate_name, payload.crate_name),
                    payload.page,
                    payload.per_page,
                )
                .await
                .map(|_| ()),
            other => {
                Err(format!("unsupported refresh scope '{}' for crate '{}'", other, job.crate_name))
            }
        };

        match result {
            Ok(()) => {
                if let Err(error) = sqlx::query(
                    "UPDATE refresh_jobs
                     SET status = 'finished',
                         finished_at = NOW(),
                         last_error = NULL
                     WHERE id = $1",
                )
                .bind(job.id)
                .execute(&state.db)
                .await
                {
                    error!(job_id = job.id, %error, "failed to mark refresh job finished");
                }
            }
            Err(error_message) => {
                let terminal = job.attempts >= MAX_ATTEMPTS;
                let retry_delay_seconds = jittered_retry_delay_seconds(job.id, job.attempts);

                if let Err(error) = sqlx::query(
                    "UPDATE refresh_jobs
                     SET status = CASE WHEN $1 THEN 'failed' ELSE 'pending' END,
                         requested_at = CASE WHEN $1 THEN requested_at ELSE NOW() + ($2 * INTERVAL \
                     '1 second') END,
                         finished_at = CASE WHEN $1 THEN NOW() ELSE NULL END,
                         last_error = $3
                     WHERE id = $4",
                )
                .bind(terminal)
                .bind(retry_delay_seconds)
                .bind(&error_message)
                .bind(job.id)
                .execute(&state.db)
                .await
                {
                    error!(job_id = job.id, %error, "failed to persist refresh job failure");
                }

                if let Err(error) = sqlx::query(
                    "UPDATE crates
                     SET last_refresh_error = $1,
                         updated_at = NOW()
                     WHERE name = $2",
                )
                .bind(&error_message)
                .bind(&job.crate_name)
                .execute(&state.db)
                .await
                {
                    error!(job_id = job.id, %error, "failed to persist crate refresh error");
                }

                if !terminal {
                    warn!(
                        job_id = job.id,
                        crate_name = %job.crate_name,
                        attempt = job.attempts,
                        "refresh job failed, queued for retry"
                    );
                }
            }
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct InteractionRefreshOutcome {
    pub(super) freshness_check_performed: bool,
    pub(super) freshness_check_result: String,
    pub(super) refresh_enqueued: bool,
    pub(super) refresh_job_id: Option<String>,
}

fn ttl_hint_seconds(
    days_since_latest_release: Option<i64>,
    releases_last_year: i64,
) -> (i32, &'static str) {
    let ttl = if releases_last_year >= 24 || days_since_latest_release.unwrap_or(0) < 30 {
        (6 * 60 * 60, "high_activity")
    } else if releases_last_year >= 6 || days_since_latest_release.unwrap_or(365) < 180 {
        (2 * 24 * 60 * 60, "moderate_activity")
    } else if days_since_latest_release.unwrap_or(0) > 365 * 5 {
        (60 * 24 * 60 * 60, "long_stable")
    } else {
        (14 * 24 * 60 * 60, "default")
    };

    (
        ttl.0
            .clamp(60 * 60, 90 * 24 * 60 * 60),
        ttl.1,
    )
}

impl McpServer {
    fn crates_io_url(&self, path: &str) -> String {
        let base = self
            .state
            .config
            .crates_io_base_url
            .trim_end_matches('/');
        let suffix = path.trim_start_matches('/');
        format!("{base}/{suffix}")
    }

    async fn crates_io_get_json<T: DeserializeOwned>(
        &self,
        path: &str,
        params: &[(&str, String)],
    ) -> Result<T, String> {
        self.state
            .acquire_outbound_slot(OutboundSource::CratesIo)
            .await;
        let url = self.crates_io_url(path);
        let response = self
            .state
            .http
            .get(&url)
            .query(params)
            .send()
            .await
            .map_err(|e| format!("request to {url} failed: {e}"))?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<body unavailable>".to_string());
            return Err(format!("request to {url} failed with status {}: {}", status, body));
        }

        response
            .json::<T>()
            .await
            .map_err(|e| format!("failed to decode JSON response from {url}: {e}"))
    }

    async fn crates_io_get_readme(
        &self,
        crate_name: &str,
        version: &str,
    ) -> Result<Option<String>, String> {
        self.state
            .acquire_outbound_slot(OutboundSource::CratesIo)
            .await;
        let url = self.crates_io_url(&format!("api/v1/crates/{crate_name}/{version}/readme"));
        let response = self
            .state
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("readme request failed for {crate_name}@{version}: {e}"))?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<body unavailable>".to_string());
            return Err(format!(
                "readme request failed for {crate_name}@{version} with status {}: {}",
                status, body
            ));
        }

        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let body = response
            .text()
            .await
            .map_err(|e| format!("failed to read readme body for {crate_name}@{version}: {e}"))?;

        if body.trim().is_empty() {
            return Ok(None);
        }

        let maybe_readme = if content_type.contains("application/json") {
            let parsed = serde_json::from_str::<Value>(&body).ok();
            parsed.and_then(|value| {
                value
                    .get("content")
                    .and_then(Value::as_str)
                    .or_else(|| {
                        value
                            .get("readme")
                            .and_then(Value::as_str)
                    })
                    .or_else(|| {
                        value
                            .get("message")
                            .and_then(Value::as_str)
                    })
                    .map(ToString::to_string)
            })
        } else {
            Some(body)
        };

        Ok(maybe_readme.and_then(|value| {
            let trimmed = value.trim().to_string();
            if trimmed.is_empty() {
                None
            } else if trimmed.len() > 1_000_000 {
                Some(
                    trimmed
                        .chars()
                        .take(1_000_000)
                        .collect(),
                )
            } else {
                Some(trimmed)
            }
        }))
    }

    async fn fetch_crate_dependencies(
        &self,
        crate_name: &str,
        version: &str,
    ) -> Result<Vec<CratesIoDependency>, String> {
        let path = format!("api/v1/crates/{crate_name}/{version}/dependencies");
        let response: CratesIoDependenciesResponse = self
            .crates_io_get_json(&path, &[])
            .await?;
        Ok(response.dependencies)
    }

    fn pick_primary_version(detail: &CratesIoCrateDetailResponse) -> Option<String> {
        if let Some(max_version) = detail
            .krate
            .max_version
            .clone()
            && detail
                .versions
                .iter()
                .any(|v| v.num == max_version)
        {
            return Some(max_version);
        }

        detail
            .versions
            .iter()
            .find(|v| !v.yanked)
            .map(|v| v.num.clone())
            .or_else(|| {
                detail
                    .versions
                    .first()
                    .map(|v| v.num.clone())
            })
    }

    async fn ensure_crate_stub(
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

    pub(super) async fn sync_single_crate(
        &self,
        crate_name: &str,
        include_dependencies: bool,
    ) -> Result<SyncCrateOutcome, String> {
        let detail: CratesIoCrateDetailResponse = self
            .crates_io_get_json(&format!("api/v1/crates/{crate_name}"), &[])
            .await?;

        let selected_version = Self::pick_primary_version(&detail);
        let readme = if let Some(version) = selected_version.as_deref() {
            self.crates_io_get_readme(crate_name, version)
                .await?
        } else {
            None
        };

        let dependencies = if include_dependencies {
            if let Some(version) = selected_version.as_deref() {
                Some(
                    self.fetch_crate_dependencies(crate_name, version)
                        .await?,
                )
            } else {
                None
            }
        } else {
            None
        };

        let categories = dedupe_strings(
            detail
                .categories
                .iter()
                .map(|c| {
                    c.slug
                        .clone()
                        .or(c.category.clone())
                        .unwrap_or_else(|| c.id.clone())
                })
                .collect(),
        );

        let keywords = dedupe_strings(
            detail
                .keywords
                .iter()
                .map(|k| {
                    k.keyword
                        .clone()
                        .unwrap_or_else(|| k.id.clone())
                })
                .collect(),
        );

        let mut tx = self
            .state
            .db
            .begin()
            .await
            .map_err(|e| format!("failed to begin transaction for {crate_name}: {e}"))?;

        let crate_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO crates (
                     name, description, repository_url, docs_url, homepage_url, categories, \
             keywords,
                     last_checked_at, next_check_at, ttl_hint_seconds, ttl_reason, \
             last_refresh_error,
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
        .bind(&categories)
        .bind(&keywords)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| format!("failed to upsert crate {crate_name}: {e}"))?;

        let mut version_ids = HashMap::new();
        for version in &detail.versions {
            let published_at = version
                .created_at
                .clone()
                .or(version.updated_at.clone());
            let version_id = sqlx::query_scalar::<_, i64>(
                "INSERT INTO crate_versions (
                    crate_id, version, published_at, yanked, total_downloads, rust_version, \
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
            .await
            .map_err(|e| {
                format!("failed to upsert version {} for {crate_name}: {e}", version.num)
            })?;

            sqlx::query("DELETE FROM crate_version_features WHERE crate_version_id = $1")
                .bind(version_id)
                .execute(&mut *tx)
                .await
                .map_err(|e| {
                    format!(
                        "failed to clear feature flags for {}@{}: {e}",
                        detail.krate.name, version.num
                    )
                })?;

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
                .await
                .map_err(|e| {
                    format!(
                        "failed to persist feature '{}' for {}@{}: {e}",
                        feature_name, detail.krate.name, version.num
                    )
                })?;
            }

            version_ids.insert(version.num.clone(), version_id);
        }

        let mut dependencies_synced = 0_usize;
        if let Some(selected_version_name) = selected_version.as_deref()
            && let Some(selected_version_id) = version_ids
                .get(selected_version_name)
                .copied()
        {
            if let Some(readme_text) = readme {
                sqlx::query(
                    "UPDATE crate_versions SET readme = $1, updated_at = NOW() WHERE id = $2",
                )
                .bind(readme_text)
                .bind(selected_version_id)
                .execute(&mut *tx)
                .await
                .map_err(|e| {
                    format!(
                        "failed to store readme for {}@{}: {e}",
                        detail.krate.name, selected_version_name
                    )
                })?;
            }

            if let Some(dependencies) = dependencies {
                sqlx::query("DELETE FROM dependency_edges WHERE from_version_id = $1")
                    .bind(selected_version_id)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| {
                        format!(
                            "failed to clear dependencies for {}@{}: {e}",
                            detail.krate.name, selected_version_name
                        )
                    })?;

                for dependency in dependencies {
                    let dependency_name = dependency.crate_id.trim();
                    if dependency_name.is_empty() {
                        continue;
                    }

                    let to_crate_id = Self::ensure_crate_stub(&mut tx, dependency_name)
                        .await
                        .map_err(|e| {
                            format!(
                                "failed to ensure dependency crate {} for {}@{}: {e}",
                                dependency_name, detail.krate.name, selected_version_name
                            )
                        })?;

                    let features = Value::Array(
                        dependency
                            .features
                            .into_iter()
                            .map(Value::String)
                            .collect::<Vec<_>>(),
                    );
                    let dependency_kind = dependency
                        .kind
                        .unwrap_or_else(|| "normal".to_string());

                    sqlx::query(
                        "INSERT INTO dependency_edges (
                            from_version_id, to_crate_id, requirement, dependency_kind, optional, \
                         features, created_at
                         ) VALUES (
                            $1, $2, $3, $4, $5, $6, NOW()
                         )",
                    )
                    .bind(selected_version_id)
                    .bind(to_crate_id)
                    .bind(dependency.req)
                    .bind(dependency_kind)
                    .bind(dependency.optional)
                    .bind(features)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| {
                        format!(
                            "failed to insert dependency edge for {}@{}: {e}",
                            detail.krate.name, selected_version_name
                        )
                    })?;

                    dependencies_synced += 1;
                }
            }
        }

        tx.commit()
            .await
            .map_err(|e| format!("failed to commit sync for {crate_name}: {e}"))?;

        Ok(SyncCrateOutcome {
            versions_synced: detail.versions.len(),
            dependencies_synced,
            selected_version,
        })
    }

    async fn enqueue_refresh_job(
        &self,
        crate_name: &str,
        scope: &str,
        priority: i32,
        include_dependencies: bool,
        payload: Value,
    ) -> Result<String, String> {
        if let Some(existing_id) = sqlx::query_scalar::<_, Option<i64>>(
            "SELECT id
             FROM refresh_jobs
             WHERE crate_name = $1 AND scope = $2 AND status IN ('pending', 'running')
             ORDER BY id ASC
             LIMIT 1",
        )
        .bind(crate_name)
        .bind(scope)
        .fetch_one(&self.state.db)
        .await
        .map_err(|e| format!("failed to query existing refresh job for {crate_name}: {e}"))?
        {
            return Ok(format!("refresh-job-{existing_id}"));
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
        .fetch_one(&self.state.db)
        .await
        .map_err(|e| format!("failed to enqueue refresh job for {crate_name}: {e}"))?;

        Ok(format!("refresh-job-{job_id}"))
    }

    pub(super) async fn ensure_freshness_for_interaction(
        &self,
        crate_id: i64,
        crate_name: &str,
        local_latest_version: &str,
    ) -> Result<InteractionRefreshOutcome, String> {
        let due = sqlx::query_scalar::<_, bool>(
            "SELECT COALESCE(next_check_at IS NULL OR next_check_at <= NOW(), TRUE)
             FROM crates
             WHERE id = $1",
        )
        .bind(crate_id)
        .fetch_one(&self.state.db)
        .await
        .map_err(|e| format!("failed to evaluate freshness deadline for {crate_name}: {e}"))?;

        if !due {
            return Ok(InteractionRefreshOutcome {
                freshness_check_performed: false,
                freshness_check_result: "skipped".to_string(),
                ..Default::default()
            });
        }

        let releases_last_year = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*)::BIGINT
             FROM crate_versions
             WHERE crate_id = $1
               AND published_at >= NOW() - INTERVAL '365 days'",
        )
        .bind(crate_id)
        .fetch_one(&self.state.db)
        .await
        .map_err(|e| format!("failed to compute release cadence for {crate_name}: {e}"))?;

        let days_since_latest = sqlx::query_scalar::<_, Option<i64>>(
            "SELECT EXTRACT(EPOCH FROM (NOW() - MAX(published_at)))::BIGINT / 86400
             FROM crate_versions
             WHERE crate_id = $1",
        )
        .bind(crate_id)
        .fetch_one(&self.state.db)
        .await
        .map_err(|e| format!("failed to compute recency for {crate_name}: {e}"))?;

        let (ttl_seconds, ttl_reason) = ttl_hint_seconds(days_since_latest, releases_last_year);

        let detail: CratesIoCrateDetailResponse = match self
            .crates_io_get_json(&format!("api/v1/crates/{crate_name}"), &[])
            .await
        {
            Ok(detail) => detail,
            Err(error) => {
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
                .bind(&error)
                .bind(crate_id)
                .execute(&self.state.db)
                .await
                .map_err(|e| format!("failed to persist probe failure for {crate_name}: {e}"))?;

                return Ok(InteractionRefreshOutcome {
                    freshness_check_performed: true,
                    freshness_check_result: "failed".to_string(),
                    ..Default::default()
                });
            }
        };

        let remote_latest = detail
            .krate
            .max_version
            .unwrap_or_default();
        let changed = !remote_latest.is_empty() && remote_latest != local_latest_version;

        if !changed {
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
            .execute(&self.state.db)
            .await
            .map_err(|e| format!("failed to persist unchanged freshness for {crate_name}: {e}"))?;

            return Ok(InteractionRefreshOutcome {
                freshness_check_performed: true,
                freshness_check_result: "unchanged".to_string(),
                ..Default::default()
            });
        }

        self.sync_single_crate(crate_name, false)
            .await
            .map_err(|e| format!("inline minimal refresh failed for {crate_name}: {e}"))?;

        let refresh_job_id = self
            .enqueue_refresh_job(
                crate_name,
                "crate_deep_refresh",
                10,
                true,
                json!({"trigger": "ttl_expired_changed"}),
            )
            .await?;

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
        .bind("changed_inline")
        .bind(crate_id)
        .execute(&self.state.db)
        .await
        .map_err(|e| format!("failed to persist changed freshness for {crate_name}: {e}"))?;

        Ok(InteractionRefreshOutcome {
            freshness_check_performed: true,
            freshness_check_result: "changed".to_string(),
            refresh_enqueued: true,
            refresh_job_id: Some(refresh_job_id),
        })
    }

    pub(super) async fn backfill_missing_requested_version(
        &self,
        crate_name: &str,
    ) -> Result<Option<String>, String> {
        self.sync_single_crate(crate_name, false)
            .await?;
        let job_id = self
            .enqueue_refresh_job(
                crate_name,
                "crate_deep_refresh",
                5,
                true,
                json!({"trigger": "missing_requested_version"}),
            )
            .await?;
        Ok(Some(job_id))
    }

    pub(super) async fn handle_index_sync_crates(
        &self,
        request: IndexSyncCratesRequest,
    ) -> Result<Json<IndexSyncCratesResponse>, String> {
        let query =
            normalize_optional(request.query).unwrap_or_else(|| DEFAULT_SYNC_QUERY.to_string());
        let page = sync_page(request.page);
        let per_page = sync_per_page(request.per_page);
        let include_dependencies = request
            .include_dependencies
            .unwrap_or(true);

        let params = vec![
            ("q", query.clone()),
            ("page", page.to_string()),
            ("per_page", per_page.to_string()),
        ];
        let search_response: CratesIoSearchResponse = self
            .crates_io_get_json("api/v1/crates", &params)
            .await?;

        let mut synced_crates = 0_usize;
        let mut synced_versions = 0_usize;
        let mut synced_dependencies = 0_usize;
        let mut selected_versions = Vec::new();
        let mut errors = Vec::new();

        for item in search_response.crates {
            let crate_name = if item.id.trim().is_empty() {
                item.name.trim().to_string()
            } else {
                item.id.trim().to_string()
            };

            if crate_name.is_empty() {
                continue;
            }

            match self
                .sync_single_crate(&crate_name, include_dependencies)
                .await
            {
                Ok(outcome) => {
                    synced_crates += 1;
                    synced_versions += outcome.versions_synced;
                    synced_dependencies += outcome.dependencies_synced;
                    if let Some(version) = outcome.selected_version {
                        selected_versions.push(format!("{crate_name}@{version}"));
                    }
                }
                Err(error) => {
                    errors.push(format!("{crate_name}: {error}"));
                }
            }
        }

        Ok(Json(IndexSyncCratesResponse {
            query,
            page,
            per_page,
            total_candidates: search_response.meta.total,
            synced_crates,
            synced_versions,
            synced_dependencies,
            selected_versions,
            errors,
            freshness: vec![
                ResponseFreshnessSource {
                    source: "crates.io".to_string(),
                    status: "refreshed".to_string(),
                    checked_at: None,
                },
                ResponseFreshnessSource {
                    source: "local_postgres_index".to_string(),
                    status: "updated".to_string(),
                    checked_at: None,
                },
            ],
            provenance: "crates.io + local_postgres_index".to_string(),
        }))
    }

    pub(super) async fn handle_index_status(
        &self,
        _request: IndexStatusRequest,
    ) -> Result<Json<IndexStatusResponse>, String> {
        let crates = sqlx::query_scalar::<_, i64>("SELECT COUNT(*)::BIGINT FROM crates")
            .fetch_one(&self.state.db)
            .await
            .map_err(|e| format!("index.status failed to count crates: {e}"))?;
        let crate_versions =
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*)::BIGINT FROM crate_versions")
                .fetch_one(&self.state.db)
                .await
                .map_err(|e| format!("index.status failed to count crate_versions: {e}"))?;
        let dependency_edges =
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*)::BIGINT FROM dependency_edges")
                .fetch_one(&self.state.db)
                .await
                .map_err(|e| format!("index.status failed to count dependency_edges: {e}"))?;
        let advisory_matches =
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*)::BIGINT FROM advisory_matches")
                .fetch_one(&self.state.db)
                .await
                .map_err(|e| format!("index.status failed to count advisory_matches: {e}"))?;
        let source_files =
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*)::BIGINT FROM source_files")
                .fetch_one(&self.state.db)
                .await
                .map_err(|e| format!("index.status failed to count source_files: {e}"))?;
        let symbols = sqlx::query_scalar::<_, i64>("SELECT COUNT(*)::BIGINT FROM symbols")
            .fetch_one(&self.state.db)
            .await
            .map_err(|e| format!("index.status failed to count symbols: {e}"))?;
        let docs_pages = sqlx::query_scalar::<_, i64>("SELECT COUNT(*)::BIGINT FROM docs_pages")
            .fetch_one(&self.state.db)
            .await
            .map_err(|e| format!("index.status failed to count docs_pages: {e}"))?;

        let pending_jobs = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*)::BIGINT FROM refresh_jobs WHERE status = 'pending' AND requested_at \
             <= NOW()",
        )
        .fetch_one(&self.state.db)
        .await
        .map_err(|e| format!("index.status failed to count pending jobs: {e}"))?;
        let delayed_jobs = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*)::BIGINT FROM refresh_jobs WHERE status = 'pending' AND requested_at \
             > NOW()",
        )
        .fetch_one(&self.state.db)
        .await
        .map_err(|e| format!("index.status failed to count pending jobs: {e}"))?;
        let retrying_jobs = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*)::BIGINT FROM refresh_jobs WHERE status = 'pending' AND attempts > 0",
        )
        .fetch_one(&self.state.db)
        .await
        .map_err(|e| format!("index.status failed to count retrying jobs: {e}"))?;
        let running_jobs = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*)::BIGINT FROM refresh_jobs WHERE status = 'running'",
        )
        .fetch_one(&self.state.db)
        .await
        .map_err(|e| format!("index.status failed to count running jobs: {e}"))?;
        let failed_jobs = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*)::BIGINT FROM refresh_jobs WHERE status = 'failed'",
        )
        .fetch_one(&self.state.db)
        .await
        .map_err(|e| format!("index.status failed to count failed jobs: {e}"))?;

        let retry_distribution = sqlx::query_as::<_, RetryDistributionRow>(
            "SELECT
                COUNT(*) FILTER (WHERE status IN ('pending', 'running') AND attempts = 1)::BIGINT \
             AS inflight_attempt_1,
                COUNT(*) FILTER (WHERE status IN ('pending', 'running') AND attempts = 2)::BIGINT \
             AS inflight_attempt_2,
                COUNT(*) FILTER (WHERE status IN ('pending', 'running') AND attempts >= 3)::BIGINT \
             AS inflight_attempt_3_plus,
                COUNT(*) FILTER (WHERE status = 'failed' AND attempts = 1)::BIGINT AS \
             failed_attempt_1,
                COUNT(*) FILTER (WHERE status = 'failed' AND attempts = 2)::BIGINT AS \
             failed_attempt_2,
                COUNT(*) FILTER (WHERE status = 'failed' AND attempts >= 3)::BIGINT AS \
             failed_attempt_3_plus
             FROM refresh_jobs",
        )
        .fetch_one(&self.state.db)
        .await
        .map_err(|e| format!("index.status failed to compute retry distribution: {e}"))?;

        let failures_by_scope = sqlx::query_as::<_, IndexFailureByScope>(
            "SELECT
                scope,
                COUNT(*)::BIGINT AS failed_jobs
             FROM refresh_jobs
             WHERE status = 'failed'
             GROUP BY scope
             ORDER BY failed_jobs DESC, scope ASC",
        )
        .fetch_all(&self.state.db)
        .await
        .map_err(|e| format!("index.status failed to compute failure-by-scope: {e}"))?;

        let last_errors = sqlx::query_scalar::<_, String>(
            "SELECT last_error
             FROM refresh_jobs
             WHERE last_error IS NOT NULL AND last_error <> ''
             ORDER BY finished_at DESC NULLS LAST, id DESC
             LIMIT 10",
        )
        .fetch_all(&self.state.db)
        .await
        .map_err(|e| format!("index.status failed to fetch job errors: {e}"))?;

        let crates_updated_at =
            sqlx::query_scalar::<_, Option<String>>("SELECT MAX(updated_at)::TEXT FROM crates")
                .fetch_one(&self.state.db)
                .await
                .map_err(|e| format!("index.status failed to read crates freshness: {e}"))?;
        let source_indexed_at = sqlx::query_scalar::<_, Option<String>>(
            "SELECT MAX(indexed_at)::TEXT FROM source_files",
        )
        .fetch_one(&self.state.db)
        .await
        .map_err(|e| format!("index.status failed to read source freshness: {e}"))?;
        let symbols_indexed_at =
            sqlx::query_scalar::<_, Option<String>>("SELECT MAX(indexed_at)::TEXT FROM symbols")
                .fetch_one(&self.state.db)
                .await
                .map_err(|e| format!("index.status failed to read symbol freshness: {e}"))?;
        let docs_indexed_at =
            sqlx::query_scalar::<_, Option<String>>("SELECT MAX(indexed_at)::TEXT FROM docs_pages")
                .fetch_one(&self.state.db)
                .await
                .map_err(|e| format!("index.status failed to read docs freshness: {e}"))?;
        let advisories_updated_at = sqlx::query_scalar::<_, Option<String>>(
            "SELECT MAX(created_at)::TEXT FROM advisory_matches",
        )
        .fetch_one(&self.state.db)
        .await
        .map_err(|e| format!("index.status failed to read advisory freshness: {e}"))?;

        let query_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*)::BIGINT
             FROM tool_invocations
             WHERE created_at >= NOW() - INTERVAL '24 hours'",
        )
        .fetch_one(&self.state.db)
        .await
        .map_err(|e| format!("index.status failed to compute query_count: {e}"))?;

        let average_latency_ms = sqlx::query_scalar::<_, Option<f64>>(
            "SELECT AVG(latency_ms)::DOUBLE PRECISION
             FROM tool_invocations
             WHERE created_at >= NOW() - INTERVAL '24 hours'",
        )
        .fetch_one(&self.state.db)
        .await
        .map_err(|e| format!("index.status failed to compute average latency: {e}"))?;

        let error_rate = sqlx::query_scalar::<_, Option<f64>>(
            "SELECT
                CASE WHEN COUNT(*) = 0 THEN NULL
                     ELSE (COUNT(*) FILTER (WHERE success = FALSE))::DOUBLE PRECISION / COUNT(*)
                END
             FROM tool_invocations
             WHERE created_at >= NOW() - INTERVAL '24 hours'",
        )
        .fetch_one(&self.state.db)
        .await
        .map_err(|e| format!("index.status failed to compute error_rate: {e}"))?;

        let cache_hit_rate = sqlx::query_scalar::<_, Option<f64>>(
            "SELECT
                CASE WHEN COUNT(*) = 0 THEN NULL
                     ELSE (COUNT(*) FILTER (WHERE hit = TRUE))::DOUBLE PRECISION / COUNT(*)
                END
             FROM query_cache_events
             WHERE created_at >= NOW() - INTERVAL '24 hours'",
        )
        .fetch_one(&self.state.db)
        .await
        .map_err(|e| format!("index.status failed to compute cache_hit_rate: {e}"))?;

        let index_lag_seconds = sqlx::query_scalar::<_, Option<i64>>(
            "WITH latest AS (
                 SELECT GREATEST(
                     COALESCE((SELECT MAX(updated_at) FROM crates), TO_TIMESTAMP(0)),
                     COALESCE((SELECT MAX(indexed_at) FROM source_files), TO_TIMESTAMP(0)),
                     COALESCE((SELECT MAX(indexed_at) FROM symbols), TO_TIMESTAMP(0)),
                     COALESCE((SELECT MAX(indexed_at) FROM docs_pages), TO_TIMESTAMP(0)),
                     COALESCE((SELECT MAX(created_at) FROM advisory_matches), TO_TIMESTAMP(0))
                 ) AS latest_ts
             )
             SELECT EXTRACT(EPOCH FROM (NOW() - latest_ts))::BIGINT
             FROM latest",
        )
        .fetch_one(&self.state.db)
        .await
        .map_err(|e| format!("index.status failed to compute index_lag_seconds: {e}"))?;

        Ok(Json(IndexStatusResponse {
            freshness: IndexFreshness {
                crates_updated_at,
                source_indexed_at,
                symbols_indexed_at,
                docs_indexed_at,
                advisories_updated_at,
            },
            coverage: IndexCoverage {
                crates,
                crate_versions,
                dependency_edges,
                advisory_matches,
                source_files,
                symbols,
                docs_pages,
            },
            operational_metrics: IndexOperationalMetrics {
                window: "24h".to_string(),
                query_count,
                average_latency_ms,
                error_rate,
                cache_hit_rate,
                index_lag_seconds,
            },
            queue: IndexQueue {
                pending_jobs,
                delayed_jobs,
                retrying_jobs,
                running_jobs,
                failed_jobs,
            },
            retry_distribution: IndexRetryDistribution {
                inflight_attempt_1: retry_distribution.inflight_attempt_1,
                inflight_attempt_2: retry_distribution.inflight_attempt_2,
                inflight_attempt_3_plus: retry_distribution.inflight_attempt_3_plus,
                failed_attempt_1: retry_distribution.failed_attempt_1,
                failed_attempt_2: retry_distribution.failed_attempt_2,
                failed_attempt_3_plus: retry_distribution.failed_attempt_3_plus,
            },
            failures_by_scope,
            last_errors,
            provenance: "local_postgres_index".to_string(),
        }))
    }

    async fn estimated_refresh_duration_seconds(
        &self,
        scope: IndexRefreshScope,
    ) -> Result<Option<u32>, String> {
        let scope_key = match scope {
            IndexRefreshScope::Crate => "crate",
            IndexRefreshScope::All => "all",
            IndexRefreshScope::Security => "security",
            IndexRefreshScope::Docs => "docs",
            IndexRefreshScope::LocalCache => "local_cache",
            IndexRefreshScope::RustdocJson => "rustdoc_json",
        };

        let (sample_count, average_seconds) = sqlx::query_as::<_, (i64, Option<f64>)>(
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
        .fetch_one(&self.state.db)
        .await
        .map_err(|e| format!("index.refresh ETA estimate failed for scope {scope_key}: {e}"))?;

        if sample_count < 3 {
            return Ok(None);
        }

        let Some(average_seconds) = average_seconds else {
            return Ok(None);
        };

        let clamped = average_seconds
            .round()
            .clamp(1.0, 86_400.0);
        Ok(Some(clamped as u32))
    }

    pub(super) async fn handle_index_refresh(
        &self,
        request: IndexRefreshRequest,
    ) -> Result<Json<IndexRefreshResponse>, String> {
        let scope = request
            .scope
            .unwrap_or(IndexRefreshScope::Crate);
        let started_at_epoch_ms = now_epoch_millis();
        let job_id = format!("job-{started_at_epoch_ms}");
        let estimated_seconds = self
            .estimated_refresh_duration_seconds(scope)
            .await?;

        match scope {
            IndexRefreshScope::Crate => {
                let crate_name = normalize_required(
                    normalize_optional(request.crate_name).unwrap_or_default(),
                    "crate_name",
                )?;

                match self
                    .sync_single_crate(
                        &crate_name,
                        request
                            .include_dependencies
                            .unwrap_or(true),
                    )
                    .await
                {
                    Ok(outcome) => Ok(Json(IndexRefreshResponse {
                        job_id,
                        scope,
                        accepted: true,
                        status: "completed".to_string(),
                        message: format!("refreshed {crate_name}"),
                        estimated_seconds,
                        estimated_seconds_remaining: Some(0),
                        started_at_epoch_ms,
                        finished_at_epoch_ms: Some(now_epoch_millis()),
                        result: Some(IndexRefreshResult {
                            synced_crates: 1,
                            synced_versions: outcome.versions_synced,
                            synced_dependencies: outcome.dependencies_synced,
                            selected_versions: outcome
                                .selected_version
                                .into_iter()
                                .map(|v| format!("{crate_name}@{v}"))
                                .collect(),
                            errors: Vec::new(),
                            synced_types: None,
                            synced_impls: None,
                            synced_traits: None,
                        }),
                        freshness: vec![
                            ResponseFreshnessSource {
                                source: "crates.io".to_string(),
                                status: "refreshed".to_string(),
                                checked_at: None,
                            },
                            ResponseFreshnessSource {
                                source: "local_postgres_index".to_string(),
                                status: "updated".to_string(),
                                checked_at: None,
                            },
                        ],
                        provenance: "crates.io + local_postgres_index".to_string(),
                    })),
                    Err(error) => Ok(Json(IndexRefreshResponse {
                        job_id,
                        scope,
                        accepted: true,
                        status: "failed".to_string(),
                        message: format!("refresh failed for {crate_name}"),
                        estimated_seconds,
                        estimated_seconds_remaining: Some(0),
                        started_at_epoch_ms,
                        finished_at_epoch_ms: Some(now_epoch_millis()),
                        result: Some(IndexRefreshResult {
                            synced_crates: 0,
                            synced_versions: 0,
                            synced_dependencies: 0,
                            selected_versions: Vec::new(),
                            errors: vec![error],
                            synced_types: None,
                            synced_impls: None,
                            synced_traits: None,
                        }),
                        freshness: vec![
                            ResponseFreshnessSource {
                                source: "crates.io".to_string(),
                                status: "failed".to_string(),
                                checked_at: None,
                            },
                            ResponseFreshnessSource {
                                source: "local_postgres_index".to_string(),
                                status: "unchanged".to_string(),
                                checked_at: None,
                            },
                        ],
                        provenance: "crates.io + local_postgres_index".to_string(),
                    })),
                }
            }
            IndexRefreshScope::All => {
                let Json(sync_response) = self
                    .handle_index_sync_crates(IndexSyncCratesRequest {
                        query: request.query,
                        page: request.page,
                        per_page: request.per_page,
                        include_dependencies: request.include_dependencies,
                    })
                    .await?;

                Ok(Json(IndexRefreshResponse {
                    job_id,
                    scope,
                    accepted: true,
                    status: "completed".to_string(),
                    message: "completed sync over search page".to_string(),
                    estimated_seconds,
                    estimated_seconds_remaining: Some(0),
                    started_at_epoch_ms,
                    finished_at_epoch_ms: Some(now_epoch_millis()),
                    result: Some(IndexRefreshResult {
                        synced_crates: sync_response.synced_crates,
                        synced_versions: sync_response.synced_versions,
                        synced_dependencies: sync_response.synced_dependencies,
                        selected_versions: sync_response.selected_versions,
                        errors: sync_response.errors,
                        synced_types: None,
                        synced_impls: None,
                        synced_traits: None,
                    }),
                    freshness: sync_response.freshness,
                    provenance: sync_response.provenance,
                }))
            }
            IndexRefreshScope::Security => {
                let page = sync_page(request.page);
                let per_page = sync_per_page(request.per_page);
                let offset = page.saturating_sub(1) * per_page;
                let mut outcome = self
                    .sync_osv_security(per_page, offset)
                    .await?;
                let rustsec_outcome = self
                    .sync_rustsec_db_security(per_page, offset)
                    .await?;
                let rustsec_enabled = self
                    .state
                    .config
                    .rustsec_db_dir
                    .is_some();
                outcome.merge(rustsec_outcome);

                Ok(Json(IndexRefreshResponse {
                    job_id,
                    scope,
                    accepted: true,
                    status: if outcome.errors.is_empty() {
                        "completed".to_string()
                    } else {
                        "completed_with_errors".to_string()
                    },
                    message: format!(
                        "security sync processed {} crates and wrote {} advisory matches",
                        outcome.crates_processed, outcome.advisories_written
                    ),
                    estimated_seconds,
                    estimated_seconds_remaining: Some(0),
                    started_at_epoch_ms,
                    finished_at_epoch_ms: Some(now_epoch_millis()),
                    result: Some(IndexRefreshResult {
                        synced_crates: outcome.crates_processed,
                        synced_versions: outcome.advisories_written,
                        synced_dependencies: 0,
                        selected_versions: outcome.touched_crates,
                        errors: outcome.errors,
                        synced_types: None,
                        synced_impls: None,
                        synced_traits: None,
                    }),
                    freshness: vec![
                        ResponseFreshnessSource {
                            source: "osv.dev".to_string(),
                            status: "refreshed".to_string(),
                            checked_at: None,
                        },
                        ResponseFreshnessSource {
                            source: "rustsec-db".to_string(),
                            status: if rustsec_enabled {
                                "refreshed".to_string()
                            } else {
                                "skipped".to_string()
                            },
                            checked_at: None,
                        },
                        ResponseFreshnessSource {
                            source: "local_postgres_index".to_string(),
                            status: "updated".to_string(),
                            checked_at: None,
                        },
                    ],
                    provenance: if rustsec_enabled {
                        "osv.dev + rustsec-db + local_postgres_index".to_string()
                    } else {
                        "osv.dev + local_postgres_index".to_string()
                    },
                }))
            }
            IndexRefreshScope::LocalCache => {
                let outcome = self
                    .sync_local_source_cache(
                        request.crate_name,
                        request.query,
                        request.page,
                        request.per_page,
                    )
                    .await?;

                Ok(Json(IndexRefreshResponse {
                    job_id,
                    scope,
                    accepted: true,
                    status: if outcome.errors.is_empty() {
                        "completed".to_string()
                    } else {
                        "completed_with_errors".to_string()
                    },
                    message: format!(
                        "scanned {} crate versions ({} files), upserted {}, pruned {}",
                        outcome.scanned_versions,
                        outcome.scanned_files,
                        outcome.upserted_files,
                        outcome.deleted_files
                    ),
                    estimated_seconds,
                    estimated_seconds_remaining: Some(0),
                    started_at_epoch_ms,
                    finished_at_epoch_ms: Some(now_epoch_millis()),
                    result: Some(IndexRefreshResult {
                        synced_crates: outcome.scanned_versions,
                        synced_versions: outcome.upserted_files,
                        synced_dependencies: outcome.deleted_files,
                        selected_versions: outcome.touched_versions,
                        errors: outcome.errors,
                        synced_types: None,
                        synced_impls: None,
                        synced_traits: None,
                    }),
                    freshness: vec![
                        ResponseFreshnessSource {
                            source: "cargo_registry".to_string(),
                            status: "scanned".to_string(),
                            checked_at: None,
                        },
                        ResponseFreshnessSource {
                            source: "local_postgres_index".to_string(),
                            status: "updated".to_string(),
                            checked_at: None,
                        },
                    ],
                    provenance: "cargo_registry + local_postgres_index".to_string(),
                }))
            }
            IndexRefreshScope::Docs => {
                let outcome = self
                    .sync_docs_pages(request.crate_name, request.page, request.per_page)
                    .await?;

                Ok(Json(IndexRefreshResponse {
                    job_id,
                    scope,
                    accepted: true,
                    status: if outcome.errors.is_empty() {
                        "completed".to_string()
                    } else {
                        "completed_with_errors".to_string()
                    },
                    message: format!(
                        "docs sync processed {} crate versions and wrote {} docs pages",
                        outcome.versions_processed, outcome.pages_written
                    ),
                    estimated_seconds,
                    estimated_seconds_remaining: Some(0),
                    started_at_epoch_ms,
                    finished_at_epoch_ms: Some(now_epoch_millis()),
                    result: Some(IndexRefreshResult {
                        synced_crates: outcome.versions_processed,
                        synced_versions: outcome.pages_written,
                        synced_dependencies: 0,
                        selected_versions: outcome.touched_versions,
                        errors: outcome.errors,
                        synced_types: None,
                        synced_impls: None,
                        synced_traits: None,
                    }),
                    freshness: vec![
                        ResponseFreshnessSource {
                            source: "docs.rs".to_string(),
                            status: "refreshed".to_string(),
                            checked_at: None,
                        },
                        ResponseFreshnessSource {
                            source: "local_postgres_index".to_string(),
                            status: "updated".to_string(),
                            checked_at: None,
                        },
                    ],
                    provenance: "docs.rs + local_postgres_index".to_string(),
                }))
            }
            IndexRefreshScope::RustdocJson => {
                let outcome = self
                    .sync_rustdoc_json_cache(request.crate_name, request.page, request.per_page)
                    .await?;

                Ok(Json(IndexRefreshResponse {
                    job_id,
                    scope,
                    accepted: true,
                    status: if outcome.errors.is_empty() {
                        "completed".to_string()
                    } else {
                        "completed_with_errors".to_string()
                    },
                    message: format!(
                        "rustdoc JSON sync scanned {} files, synced {} crate versions, wrote {} \
                         symbols, {} types, {} impls, {} traits",
                        outcome.scanned_files,
                        outcome.synced_versions,
                        outcome.symbols_written,
                        outcome.types_written,
                        outcome.impls_written,
                        outcome.traits_written,
                    ),
                    estimated_seconds,
                    estimated_seconds_remaining: Some(0),
                    started_at_epoch_ms,
                    finished_at_epoch_ms: Some(now_epoch_millis()),
                    result: Some(IndexRefreshResult {
                        synced_crates: outcome.scanned_files,
                        synced_versions: outcome.synced_versions,
                        synced_dependencies: outcome.symbols_written,
                        selected_versions: outcome.touched_versions,
                        errors: outcome.errors,
                        synced_types: Some(outcome.types_written),
                        synced_impls: Some(outcome.impls_written),
                        synced_traits: Some(outcome.traits_written),
                    }),
                    freshness: vec![
                        ResponseFreshnessSource {
                            source: "rustdoc_json".to_string(),
                            status: "scanned".to_string(),
                            checked_at: None,
                        },
                        ResponseFreshnessSource {
                            source: "local_postgres_index".to_string(),
                            status: "updated".to_string(),
                            checked_at: None,
                        },
                    ],
                    provenance: "rustdoc_json + local_postgres_index".to_string(),
                }))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::optional_job_crate_name;

    #[test]
    fn optional_job_crate_name_prefers_payload() {
        let result = optional_job_crate_name("serde", Some("tokio".to_string()));
        assert_eq!(result.as_deref(), Some("tokio"));
    }

    #[test]
    fn optional_job_crate_name_treats_all_as_none() {
        assert!(optional_job_crate_name("all", None).is_none());
        assert!(optional_job_crate_name("*", Some("all".to_string())).is_none());
    }
}
