//! Periodic background security advisory synchronization.
//!
//! [`run_security_sync`] periodically fetches OSV, RustSec and GHSA advisory
//! data for all indexed crates. It runs once immediately at startup, then
//! repeats at the interval given by `SECURITY_SYNC_INTERVAL_SECS`. Setting
//! the interval to 0 disables the periodic loop (startup sync still runs).
//!
//! Each sync run pages through all indexed crates in batches of
//! `SECURITY_SYNC_BATCH_SIZE`, querying the OSV API, local RustSec
//! advisory-db, and the GitHub Security Advisories API for each crate.
//! The task uses its own rate limiter configuration and runs independently
//! of all other background tasks.
//!
//! Crates that were successfully checked are stamped with
//! `security_checked_at = NOW()`. The staleness threshold
//! (`SECURITY_SYNC_INTERVAL_SECS`) is used to skip recently-checked crates
//! so that restarts don't re-query every crate from scratch.

use metrics::{counter, histogram};
use tokio::time::{Duration, sleep};
use tracing::{info, warn};

use crate::db::indexing::{fetch_security_crates_page, stamp_security_checked_at};
use crate::mcp::indexing::security::SecuritySyncOutcome;
use crate::mcp::server::McpServer;
use crate::state::AppState;

/// Summary of a full security sync run (all pages).
#[derive(Debug, Default)]
pub struct SecuritySyncRunOutcome {
    /// Total crates processed across all pages.
    pub crates_processed: usize,
    /// Total advisory records written.
    pub advisories_written: usize,
    /// Total errors encountered.
    pub errors: usize,
    /// Number of pages fetched.
    pub pages: usize,
}

impl SecuritySyncRunOutcome {
    fn accumulate(&mut self, page_outcome: &SecuritySyncOutcome) {
        self.crates_processed += page_outcome.crates_processed;
        self.advisories_written += page_outcome.advisories_written;
        self.errors += page_outcome.errors.len();
        self.pages += 1;
    }
}

/// Runs the periodic security advisory sync loop.
pub async fn run_security_sync(state: AppState) {
    let max_age_secs = staleness_threshold(&state);
    let outcome = run_security_sync_scan(&state, max_age_secs).await;
    info!(
        crates_processed = outcome.crates_processed,
        advisories_written = outcome.advisories_written,
        errors = outcome.errors,
        pages = outcome.pages,
        "startup security sync complete"
    );

    let interval_secs = state
        .config
        .security_sync_interval_secs;
    if interval_secs == 0 {
        info!("SECURITY_SYNC_INTERVAL_SECS=0; periodic security sync disabled");
        return;
    }

    loop {
        sleep(Duration::from_secs(interval_secs)).await;
        let outcome = run_security_sync_scan(&state, max_age_secs).await;
        info!(
            crates_processed = outcome.crates_processed,
            advisories_written = outcome.advisories_written,
            errors = outcome.errors,
            pages = outcome.pages,
            "periodic security sync complete"
        );
    }
}

/// Performs a single full security sync, paging through all indexed crates.
pub async fn run_security_sync_scan(
    state: &AppState,
    max_age_secs: Option<i64>,
) -> SecuritySyncRunOutcome {
    let scan_started = std::time::Instant::now();
    let batch_size = state
        .config
        .security_sync_batch_size
        .max(1);
    let server = McpServer::new(state.clone());
    let mut run_outcome = SecuritySyncRunOutcome::default();
    // Snapshot before stamping: changing freshness during offset pagination
    // would skip crates, including legacy advisory rows awaiting repair.
    let crates = match fetch_security_crates_page(&state.db, i64::MAX, 0, max_age_secs).await {
        Ok(crates) => crates,
        Err(error) => {
            warn!(%error, "failed to select crates for security sync");
            run_outcome.errors += 1;
            return run_outcome;
        }
    };
    for crates in crates.chunks(batch_size as usize) {
        let page_outcome = server
            .sync_security_crates(crates)
            .await;

        // Stamp successfully checked crates so they're skipped until stale.
        for crate_id in &page_outcome.checked_crate_ids {
            if let Err(error) = stamp_security_checked_at(&state.db, *crate_id).await {
                warn!(crate_id, %error, "failed to stamp security_checked_at");
            }
        }

        run_outcome.accumulate(&page_outcome);
    }

    let elapsed_ms = scan_started
        .elapsed()
        .as_millis() as f64;
    histogram!("rust_mcp_security_sync_duration_ms").record(elapsed_ms);
    counter!("rust_mcp_security_sync_scans_total").increment(1);
    counter!("rust_mcp_security_advisories_written_total")
        .increment(run_outcome.advisories_written as u64);

    run_outcome
}

/// Derives the staleness threshold from config.
///
/// Uses `SECURITY_SYNC_INTERVAL_SECS` as the max age — crates checked
/// within that window are considered fresh and skipped. Returns `None`
/// (check all) when the interval is 0 (one-shot mode).
fn staleness_threshold(state: &AppState) -> Option<i64> {
    let secs = state
        .config
        .security_sync_interval_secs;
    if secs == 0 { None } else { Some(secs as i64) }
}
