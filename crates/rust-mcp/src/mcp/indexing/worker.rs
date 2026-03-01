use metrics::gauge;
use serde::Deserialize;
use serde_json::json;
use tokio::time::{Duration, sleep};
use tracing::{error, info, warn};

use crate::db::indexing::{
    claim_next_refresh_job, enqueue_or_get_refresh_job_id, fetch_refresh_job_gauge_counts,
    fetch_refresh_job_scope_pending_counts, mark_crate_refresh_error,
    mark_refresh_job_failed_or_requeued, mark_refresh_job_finished,
};
use crate::mcp::indexing::coordinator::{JobOutcome, PRIORITY_PROACTIVE_ENRICH};
use crate::mcp::indexing::handlers::IndexSyncCratesRequest;
use crate::mcp::server::McpServer;
use crate::mcp::utils::{sync_page, sync_per_page};
use crate::state::AppState;

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

/// Runs the durable background refresh worker loop.
pub async fn run_refresh_worker(state: AppState) {
    const MAX_ATTEMPTS: i32 = 3;

    loop {
        // Update refresh job gauges for Prometheus on each iteration.
        if let Ok(counts) = fetch_refresh_job_gauge_counts(&state.db).await {
            gauge!("rust_mcp_refresh_jobs_pending").set(counts.pending_jobs as f64);
            gauge!("rust_mcp_refresh_jobs_running").set(counts.running_jobs as f64);
            gauge!("rust_mcp_refresh_jobs_failed").set(counts.failed_jobs as f64);

            // Ratio of background (discovery/enrichment) jobs to total pending.
            // 0.0 when the queue is empty; 1.0 when all pending work is background.
            let ratio = if counts.pending_jobs > 0 {
                counts.background_pending_jobs as f64 / counts.pending_jobs as f64
            } else {
                0.0
            };
            gauge!("rust_mcp_refresh_jobs_background_ratio").set(ratio);
        }

        // Per-scope pending breakdown — emitted separately to avoid a single
        // query returning too many columns.
        if let Ok(scope_counts) = fetch_refresh_job_scope_pending_counts(&state.db).await {
            for (scope, count) in scope_counts {
                gauge!("rust_mcp_refresh_jobs_scope_pending", "scope" => scope).set(count as f64);
            }
        }

        let next_job = claim_next_refresh_job(&state.db).await;

        let Some(job) = (match next_job {
            Ok(value) => value,
            Err(error) => {
                error!(%error, "refresh worker failed to dequeue job");
                sleep(Duration::from_secs(2)).await;
                continue;
            }
        }) else {
            // No pending jobs — wait for a wake signal from the
            // coordinator (on-demand request) or fall back to the
            // 2-second poll interval.
            tokio::select! {
                () = state.indexing_coordinator.notified() => {},
                () = sleep(Duration::from_secs(2)) => {},
            }
            continue;
        };

        info!(
            job_id = job.id,
            crate_name = %job.crate_name,
            scope = %job.scope,
            attempt = job.attempts,
            "processing refresh job"
        );

        let job_started = std::time::Instant::now();
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

        let elapsed = job_started.elapsed();

        match result {
            Ok(()) => {
                info!(
                    job_id = job.id,
                    crate_name = %job.crate_name,
                    scope = %job.scope,
                    elapsed_ms = elapsed.as_millis() as u64,
                    "refresh job completed"
                );
                if let Err(error) = mark_refresh_job_finished(&state.db, job.id).await {
                    error!(job_id = job.id, %error, "failed to mark refresh job finished");
                }
                state
                    .indexing_coordinator
                    .signal_completion(job.id, JobOutcome::Completed);

                // After a successful crate-metadata job, proactively enqueue
                // source and rustdoc enrichment at lower priority so they are
                // ready before any client asks. enqueue_or_get_refresh_job_id
                // is idempotent — safe to call unconditionally.
                if job.scope == "crate" || job.scope == "crate_deep_refresh" {
                    for enrichment_scope in ["local_cache", "rustdoc_json"] {
                        if let Err(error) = enqueue_or_get_refresh_job_id(
                            &state.db,
                            &job.crate_name,
                            enrichment_scope,
                            PRIORITY_PROACTIVE_ENRICH,
                            false,
                            json!({"trigger": "proactive_enrich"}),
                        )
                        .await
                        {
                            warn!(
                                crate_name = %job.crate_name,
                                enrichment_scope,
                                %error,
                                "failed to enqueue proactive enrichment job"
                            );
                        }
                    }
                }
            }
            Err(error_message) => {
                let terminal = job.attempts >= MAX_ATTEMPTS;
                let retry_delay_seconds = jittered_retry_delay_seconds(job.id, job.attempts);

                if let Err(error) = mark_refresh_job_failed_or_requeued(
                    &state.db,
                    job.id,
                    terminal,
                    retry_delay_seconds,
                    &error_message,
                )
                .await
                {
                    error!(job_id = job.id, %error, "failed to persist refresh job failure");
                }

                if let Err(error) =
                    mark_crate_refresh_error(&state.db, &job.crate_name, &error_message).await
                {
                    error!(job_id = job.id, %error, "failed to persist crate refresh error");
                }

                // Signal waiting tool handlers immediately (even for
                // non-terminal failures — handlers should not wait across
                // retries).
                state
                    .indexing_coordinator
                    .signal_completion(job.id, JobOutcome::Failed(error_message.clone()));

                if terminal {
                    error!(
                        job_id = job.id,
                        crate_name = %job.crate_name,
                        attempt = job.attempts,
                        error = %error_message,
                        "refresh job permanently failed after max attempts"
                    );
                } else {
                    warn!(
                        job_id = job.id,
                        crate_name = %job.crate_name,
                        attempt = job.attempts,
                        error = %error_message,
                        "refresh job failed, queued for retry"
                    );
                }
            }
        }
    }
}

/// Runs a one-shot rustdoc JSON sync at startup with default page size.
pub async fn run_startup_rustdoc_json_refresh(state: AppState) {
    run_startup_rustdoc_json_refresh_with_page_size(state, 100).await;
}

/// Runs a one-shot rustdoc JSON sync at startup with a custom page size.
pub async fn run_startup_rustdoc_json_refresh_with_page_size(state: AppState, per_page: u32) {
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
