use metrics::gauge;
use serde::Deserialize;
use serde_json::Value;
use tokio::time::{Duration, sleep};
use tracing::{error, info, warn};

use crate::mcp::indexing::handlers::IndexSyncCratesRequest;
use crate::mcp::server::McpServer;
use crate::mcp::utils::{sync_page, sync_per_page};
use crate::state::AppState;

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

pub(crate) async fn run_startup_rustdoc_json_refresh(state: AppState) {
    run_startup_rustdoc_json_refresh_with_page_size(state, 100).await;
}

pub(crate) async fn run_startup_rustdoc_json_refresh_with_page_size(
    state: AppState,
    per_page: u32,
) {
    let page_size = sync_per_page(Some(per_page));
    let server = McpServer::new(state);

    let mut page = 1_u32;
    let mut total_scanned_files = 0_usize;
    let mut total_synced_versions = 0_usize;
    let mut total_symbols_written = 0_usize;
    let mut total_types_written = 0_usize;
    let mut total_impls_written = 0_usize;
    let mut total_traits_written = 0_usize;
    let mut total_errors = 0_usize;

    loop {
        let outcome = match server
            .sync_rustdoc_json_cache(None, Some(page), Some(page_size))
            .await
        {
            Ok(outcome) => outcome,
            Err(error) => {
                warn!(page, %error, "startup rustdoc JSON refresh failed");
                break;
            }
        };

        total_scanned_files += outcome.scanned_files;
        total_synced_versions += outcome.synced_versions;
        total_symbols_written += outcome.symbols_written;
        total_types_written += outcome.types_written;
        total_impls_written += outcome.impls_written;
        total_traits_written += outcome.traits_written;
        total_errors += outcome.errors.len();

        if outcome.scanned_files < page_size as usize {
            break;
        }

        page = page.saturating_add(1);
    }

    info!(
        scanned_files = total_scanned_files,
        synced_versions = total_synced_versions,
        symbols_written = total_symbols_written,
        types_written = total_types_written,
        impls_written = total_impls_written,
        traits_written = total_traits_written,
        errors = total_errors,
        "startup rustdoc JSON refresh finished"
    );
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
