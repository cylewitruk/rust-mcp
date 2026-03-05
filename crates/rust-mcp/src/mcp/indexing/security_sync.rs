//! Periodic background security advisory synchronization.
//!
//! [`run_security_sync`] periodically fetches OSV and (optionally) RustSec
//! advisory data for all indexed crates. It runs once immediately at startup,
//! then repeats at the interval given by `SECURITY_SYNC_INTERVAL_SECS`.
//! Setting the interval to 0 disables the periodic loop (startup sync still
//! runs).
//!
//! Each sync run pages through all indexed crates in batches of
//! `SECURITY_SYNC_BATCH_SIZE`, querying the OSV API and local RustSec
//! advisory-db for each crate. The task uses its own rate limiter
//! configuration and runs independently of all other background tasks.

use metrics::{counter, histogram};
use tokio::time::{Duration, sleep};
use tracing::{info, warn};

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
    let outcome = run_security_sync_scan(&state).await;
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
        let outcome = run_security_sync_scan(&state).await;
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
pub async fn run_security_sync_scan(state: &AppState) -> SecuritySyncRunOutcome {
    let scan_started = std::time::Instant::now();
    let batch_size = state
        .config
        .security_sync_batch_size
        .max(1);
    let server = McpServer::new(state.clone());
    let mut run_outcome = SecuritySyncRunOutcome::default();
    let mut offset: u32 = 0;

    loop {
        let osv_result = server
            .sync_osv_security(batch_size, offset)
            .await;
        let rustsec_result = server
            .sync_rustsec_db_security(batch_size, offset)
            .await;

        let page_outcome = match (osv_result, rustsec_result) {
            (Ok(mut osv), Ok(rustsec)) => {
                osv.merge(rustsec);
                osv
            }
            (Ok(mut osv), Err(error)) => {
                warn!(%error, "security sync: RustSec DB sync failed for page");
                osv.errors.push(error);
                osv
            }
            (Err(error), Ok(mut rustsec)) => {
                warn!(%error, "security sync: OSV sync failed for page");
                rustsec.errors.push(error);
                rustsec
            }
            (Err(osv_error), Err(rustsec_error)) => {
                warn!(
                    osv_error = %osv_error,
                    rustsec_error = %rustsec_error,
                    "security sync: both OSV and RustSec sync failed for page"
                );
                counter!("rust_mcp_security_sync_errors_total").increment(1);
                run_outcome.errors += 2;
                run_outcome.pages += 1;
                break;
            }
        };

        let page_crates = page_outcome.crates_processed;
        run_outcome.accumulate(&page_outcome);

        // If we got fewer crates than the batch size, we've reached the end.
        if page_crates < batch_size as usize {
            break;
        }

        offset += batch_size;
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
