use metrics::{counter, gauge};
use serde::Deserialize;
use serde_json::json;
use tokio::time::{Duration, sleep};
use tracing::{error, info, warn};

use crate::db::indexing::{
    claim_next_refresh_job, enqueue_or_get_refresh_job_id, fetch_refresh_job_gauge_counts,
    fetch_refresh_job_scope_pending_counts, mark_crate_refresh_error,
    mark_refresh_job_failed_or_requeued, mark_refresh_job_finished, mark_versions_locally_present,
};
use crate::mcp::indexing::coordinator::{JobOutcome, PRIORITY_PROACTIVE_ENRICH};
use crate::mcp::indexing::discovery::collect_local_versions_for_crate;
use crate::mcp::indexing::handlers::{IndexSyncCratesRequest, SyncCrateOutcome};
use crate::mcp::indexing::local_cache::LocalCacheRefreshOutcome;
use crate::mcp::indexing::rustdoc_json::RustdocJsonRefreshOutcome;
use crate::mcp::indexing::security::SecuritySyncOutcome;
use crate::mcp::progress::ToolCallContext;
use crate::mcp::server::McpServer;
use crate::mcp::tools::docs::DocsRefreshOutcome;
use crate::mcp::utils::{sync_page, sync_per_page};
use crate::state::AppState;

/// Typed outcome returned by each refresh job scope, carrying data for
/// Prometheus counter emission.
enum RefreshJobOutcome {
    CrateSync(SyncCrateOutcome),
    RustdocJson(RustdocJsonRefreshOutcome),
    LocalCache(LocalCacheRefreshOutcome),
    Docs(DocsRefreshOutcome),
    Security(SecuritySyncOutcome),
    /// `index_sync_crates` ("all") — outcome is already handled internally.
    AllSync,
}

#[derive(Debug, Default, Deserialize)]
struct RefreshJobPayload {
    crate_name: Option<String>,
    query: Option<String>,
    page: Option<u32>,
    per_page: Option<u32>,
    include_dependencies: Option<bool>,
    local_versions: Option<Vec<String>>,
    locally_present_only: Option<bool>,
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

        gauge!("rust_mcp_refresh_jobs_running").increment(1.0);

        let job_started = std::time::Instant::now();
        let server = McpServer::new(state.clone());
        let payload =
            serde_json::from_value::<RefreshJobPayload>(job.payload.clone()).unwrap_or_default();
        let locally_present_only = payload
            .locally_present_only
            .unwrap_or(false);
        let result: Result<RefreshJobOutcome, String> = match job.scope.as_str() {
            "crate" | "crate_deep_refresh" => server
                .sync_single_crate(&job.crate_name, job.include_dependencies)
                .await
                .map(RefreshJobOutcome::CrateSync),
            "all" => server
                .handle_index_sync_crates(
                    IndexSyncCratesRequest {
                        query: payload.query,
                        page: payload.page,
                        per_page: payload.per_page,
                        include_dependencies: payload
                            .include_dependencies
                            .or(Some(job.include_dependencies)),
                    },
                    ToolCallContext::no_op(),
                )
                .await
                .map(|_| RefreshJobOutcome::AllSync),
            "security" => {
                let page = sync_page(payload.page);
                let per_page = sync_per_page(payload.per_page);
                let offset = page.saturating_sub(1) * per_page;
                match server
                    .sync_osv_security(per_page, offset, None)
                    .await
                {
                    Ok(mut osv) => match server
                        .sync_rustsec_db_security(per_page, offset, None)
                        .await
                    {
                        Ok(rustsec) => {
                            osv.merge(rustsec);
                            Ok(RefreshJobOutcome::Security(osv))
                        }
                        Err(error) => Err(error),
                    },
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
                .map(RefreshJobOutcome::Docs),
            "local_cache" => server
                .sync_local_source_cache(
                    optional_job_crate_name(&job.crate_name, payload.crate_name),
                    payload.query,
                    payload.page,
                    payload.per_page,
                    locally_present_only,
                )
                .await
                .map(RefreshJobOutcome::LocalCache),
            "rustdoc_json" => server
                .sync_rustdoc_json_cache(
                    optional_job_crate_name(&job.crate_name, payload.crate_name),
                    payload.page,
                    payload.per_page,
                    locally_present_only,
                    true,
                    Some(
                        state
                            .config
                            .rustdoc_retry_cooldown_secs as i64,
                    ),
                )
                .await
                .map(RefreshJobOutcome::RustdocJson),
            other => {
                Err(format!("unsupported refresh scope '{}' for crate '{}'", other, job.crate_name))
            }
        };

        gauge!("rust_mcp_refresh_jobs_running").decrement(1.0);
        let elapsed = job_started.elapsed();

        match result {
            Ok(outcome) => {
                emit_outcome_counters(&outcome);

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
                counter!("rust_mcp_refresh_jobs_completed_total", "scope" => job.scope.clone())
                    .increment(1);
                state
                    .indexing_coordinator
                    .signal_completion(job.id, JobOutcome::Completed);

                // After a successful crate-metadata job, mark which versions
                // are locally present in the cargo registry, then proactively
                // enqueue source and rustdoc enrichment (locally-present only)
                // at lower priority. enqueue_or_get_refresh_job_id is
                // idempotent — safe to call unconditionally.
                if job.scope == "crate" || job.scope == "crate_deep_refresh" {
                    let local_versions = match payload.local_versions {
                        Some(ref versions) if !versions.is_empty() => versions.clone(),
                        _ => {
                            let src_root = state
                                .config
                                .cargo_registry_dir
                                .join("src");
                            collect_local_versions_for_crate(&src_root, &job.crate_name)
                        }
                    };

                    if let Err(error) =
                        mark_versions_locally_present(&state.db, &job.crate_name, &local_versions)
                            .await
                    {
                        warn!(
                            crate_name = %job.crate_name,
                            %error,
                            "failed to mark locally-present versions"
                        );
                    }

                    for enrichment_scope in ["local_cache", "rustdoc_json"] {
                        if let Err(error) = enqueue_or_get_refresh_job_id(
                            &state.db,
                            &job.crate_name,
                            enrichment_scope,
                            PRIORITY_PROACTIVE_ENRICH,
                            false,
                            json!({"trigger": "proactive_enrich", "locally_present_only": true}),
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
                counter!("rust_mcp_refresh_jobs_errored_total", "scope" => job.scope.clone())
                    .increment(1);

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

fn emit_outcome_counters(outcome: &RefreshJobOutcome) {
    match outcome {
        RefreshJobOutcome::CrateSync(o) => {
            counter!("rust_mcp_crate_versions_synced_total").increment(o.versions_synced as u64);
            counter!("rust_mcp_dependencies_synced_total").increment(o.dependencies_synced as u64);
        }
        RefreshJobOutcome::RustdocJson(o) => {
            counter!("rust_mcp_rustdoc_symbols_written_total").increment(o.symbols_written as u64);
            counter!("rust_mcp_rustdoc_types_written_total").increment(o.types_written as u64);
            counter!("rust_mcp_rustdoc_impls_written_total").increment(o.impls_written as u64);
            counter!("rust_mcp_rustdoc_traits_written_total").increment(o.traits_written as u64);
        }
        RefreshJobOutcome::LocalCache(o) => {
            counter!("rust_mcp_source_files_upserted_total").increment(o.upserted_files as u64);
        }
        RefreshJobOutcome::Docs(o) => {
            counter!("rust_mcp_docs_pages_written_total").increment(o.pages_written as u64);
        }
        RefreshJobOutcome::Security(o) => {
            counter!("rust_mcp_security_advisories_written_total")
                .increment(o.advisories_written as u64);
        }
        RefreshJobOutcome::AllSync => {}
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
