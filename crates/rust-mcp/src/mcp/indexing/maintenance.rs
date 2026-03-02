//! Periodic enrichment maintenance task.
//!
//! [`run_enrichment_maintenance`] scans for crate versions that have not yet
//! been enriched with rustdoc JSON data (or whose last attempt exceeded the
//! configured retry cooldown) and enqueues `rustdoc_json` jobs into the
//! existing refresh queue. The worker processes them in priority order — this
//! module only manages queue state, not direct enrichment.
//!
//! The task runs once immediately at startup, then repeats at the interval
//! given by `ENRICHMENT_MAINTENANCE_INTERVAL_SECS`. Setting the interval to 0
//! disables the periodic loop (startup scan still runs).

use metrics::{counter, histogram};
use serde_json::json;
use tokio::time::{Duration, sleep};
use tracing::{info, warn};

use crate::db::indexing::{enqueue_or_get_refresh_job_id, fetch_unenriched_crate_names};
use crate::mcp::indexing::coordinator::PRIORITY_PROACTIVE_ENRICH;
use crate::state::AppState;

/// Summary of a single enrichment maintenance scan.
#[derive(Debug, Default)]
pub struct EnrichmentMaintenanceOutcome {
    /// Number of crate names found needing enrichment.
    pub scanned: usize,
    /// New `rustdoc_json` jobs created this run.
    pub enqueued: usize,
    /// Crates that already had a pending/running job (deduplicated).
    pub already_queued: usize,
}

// ──────────────────────────────────────────────────────────────────────────────
// Entry point
// ──────────────────────────────────────────────────────────────────────────────

/// Long-running background task.
///
/// Runs an immediate scan at startup, then sleeps for
/// `config.enrichment_maintenance_interval_secs` between subsequent scans.
/// If the interval is 0, the function exits after the startup scan.
pub async fn run_enrichment_maintenance(state: AppState) {
    let outcome = run_enrichment_maintenance_scan(&state).await;
    info!(
        scanned = outcome.scanned,
        enqueued = outcome.enqueued,
        already_queued = outcome.already_queued,
        "startup enrichment maintenance scan complete"
    );

    let interval_secs = state
        .config
        .enrichment_maintenance_interval_secs;
    if interval_secs == 0 {
        info!("ENRICHMENT_MAINTENANCE_INTERVAL_SECS=0; periodic enrichment maintenance disabled");
        return;
    }

    loop {
        sleep(Duration::from_secs(interval_secs)).await;
        let outcome = run_enrichment_maintenance_scan(&state).await;
        info!(
            scanned = outcome.scanned,
            enqueued = outcome.enqueued,
            already_queued = outcome.already_queued,
            "periodic enrichment maintenance scan complete"
        );
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Core scan logic
// ──────────────────────────────────────────────────────────────────────────────

/// Perform one enrichment maintenance scan: find crate names with unenriched
/// versions and enqueue `rustdoc_json` jobs for each.
pub async fn run_enrichment_maintenance_scan(state: &AppState) -> EnrichmentMaintenanceOutcome {
    let scan_started = std::time::Instant::now();
    let mut outcome = EnrichmentMaintenanceOutcome::default();

    let cooldown = state
        .config
        .rustdoc_retry_cooldown_secs as i64;

    let crate_names = match fetch_unenriched_crate_names(&state.db, cooldown).await {
        Ok(names) => names,
        Err(error) => {
            warn!(%error, "enrichment maintenance scan: failed to fetch unenriched crate names");
            counter!("rust_mcp_enrichment_maintenance_errors_total").increment(1);
            return outcome;
        }
    };

    outcome.scanned = crate_names.len();

    for crate_name in &crate_names {
        match enqueue_or_get_refresh_job_id(
            &state.db,
            crate_name,
            "rustdoc_json",
            PRIORITY_PROACTIVE_ENRICH,
            false,
            json!({
                "trigger": "enrichment_maintenance",
                "locally_present_only": true,
            }),
        )
        .await
        {
            Ok(enqueue_outcome) => {
                if enqueue_outcome.is_new {
                    outcome.enqueued += 1;
                } else {
                    outcome.already_queued += 1;
                }
            }
            Err(error) => {
                warn!(%error, %crate_name, "enrichment maintenance: failed to enqueue job");
            }
        }
    }

    // Wake the worker if we enqueued anything.
    if outcome.enqueued > 0 {
        state
            .indexing_coordinator
            .notify_worker();
    }

    let elapsed_ms = scan_started
        .elapsed()
        .as_millis() as f64;
    histogram!("rust_mcp_enrichment_maintenance_duration_ms").record(elapsed_ms);
    counter!("rust_mcp_enrichment_maintenance_scans_total").increment(1);
    counter!("rust_mcp_enrichment_maintenance_jobs_enqueued_total")
        .increment(outcome.enqueued as u64);

    outcome
}
