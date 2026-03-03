//! Coordinator that bridges tool handlers waiting for on-demand indexing
//! with the background refresh worker.
//!
//! When a tool discovers a requested crate isn't indexed locally, it calls
//! [`IndexingCoordinator::enqueue_on_demand`] to submit a high-priority
//! refresh job and [`IndexingCoordinator::wait_for_job`] to block until the
//! worker completes it. The worker signals completion via
//! [`IndexingCoordinator::signal_completion`].

use std::collections::HashMap;
use std::sync::RwLock;
use std::time::Duration;

use serde_json::json;
use sqlx::PgPool;
use tokio::sync::{Notify, watch};

use crate::db::indexing::enqueue_or_get_refresh_job_id;

/// Maximum time a tool handler will wait for an on-demand indexing job.
const ON_DEMAND_TIMEOUT: Duration = Duration::from_secs(45);

// ──────────────────────────────────────────────────────────────────────────────
// Job priority levels (lower = higher urgency)
// ──────────────────────────────────────────────────────────────────────────────

/// Interactive on-demand requests from tool handlers.
pub const PRIORITY_ON_DEMAND: i32 = 1;
/// Follow-up deep refresh after a missing-version backfill.
pub const PRIORITY_BACKFILL: i32 = 5;
/// Background freshness-triggered deep refresh.
pub const PRIORITY_FRESHNESS: i32 = 10;
/// Near-real-time detection of new `.crate` files in the cargo cache.
pub const PRIORITY_CACHE_WATCH: i32 = 20;
/// Pre-warm list crates indexed first at startup.
pub const PRIORITY_PRE_WARM: i32 = 25;
/// Crates discovered via the periodic registry scan.
pub const PRIORITY_DISCOVERY: i32 = 50;
/// Proactive source/rustdoc enrichment enqueued after a crate job succeeds.
pub const PRIORITY_PROACTIVE_ENRICH: i32 = 75;
/// Default / explicitly scheduled refresh jobs.
#[allow(dead_code)]
pub const PRIORITY_DEFAULT: i32 = 100;

/// Terminal outcome of a tracked indexing job.
#[derive(Debug, Clone, PartialEq)]
pub enum JobOutcome {
    /// Job has not completed yet.
    Pending,
    /// Job finished successfully.
    Completed,
    /// Job failed (terminal or non-terminal — the tool handler does not wait
    /// across retries).
    Failed(String),
}

/// In-process coordinator that bridges tool-handler waiters with the
/// background refresh worker.
#[derive(Debug, Default)]
pub struct IndexingCoordinator {
    /// Wakes the worker when a high-priority on-demand job is enqueued.
    worker_notify: Notify,
    /// Per-job completion channels, keyed by `refresh_jobs.id`.
    /// Multiple tool calls for the same crate coalesce onto the same
    /// channel because [`enqueue_or_get_refresh_job_id`] returns an
    /// existing job ID when a pending/running job already exists.
    waiters: RwLock<HashMap<i64, watch::Sender<JobOutcome>>>,
}

impl IndexingCoordinator {
    /// Creates a new coordinator with no pending waiters.
    pub fn new() -> Self {
        Self {
            worker_notify: Notify::new(),
            waiters: RwLock::new(HashMap::new()),
        }
    }

    /// Enqueue a high-priority on-demand indexing job for the given scope and
    /// register a completion waiter. Returns the raw job ID for subsequent
    /// [`wait_for_job`](Self::wait_for_job) calls.
    pub async fn enqueue_on_demand(
        &self,
        db: &PgPool,
        crate_name: &str,
        scope: &str,
    ) -> Result<i64, String> {
        let job_id = enqueue_or_get_refresh_job_id(
            db,
            crate_name,
            scope,
            PRIORITY_ON_DEMAND,
            false,
            json!({"trigger": "on_demand", "scope": scope}),
        )
        .await
        .map_err(|e| format!("failed to enqueue on-demand {scope} job for '{crate_name}': {e}"))?
        .job_id;

        // Register waiter — idempotent; coalesced jobs reuse the same ID.
        {
            let mut waiters = self
                .waiters
                .write()
                .unwrap_or_else(|e| e.into_inner());
            waiters
                .entry(job_id)
                .or_insert_with(|| watch::channel(JobOutcome::Pending).0);
        }

        // Wake the worker so it picks up this job immediately.
        self.worker_notify
            .notify_one();

        Ok(job_id)
    }

    /// Block until the given job reaches a terminal state or the timeout
    /// expires. Returns the observed [`JobOutcome`].
    pub async fn wait_for_job(&self, job_id: i64) -> Result<JobOutcome, String> {
        let rx = {
            let waiters = self
                .waiters
                .read()
                .unwrap_or_else(|e| e.into_inner());
            waiters
                .get(&job_id)
                .map(|tx| tx.subscribe())
        };

        let Some(mut rx) = rx else {
            // No waiter registered — the job may have completed before we
            // subscribed. Treat optimistically; the caller will re-query
            // the DB and fail with a clear message if the crate is still
            // missing.
            return Ok(JobOutcome::Completed);
        };

        match tokio::time::timeout(ON_DEMAND_TIMEOUT, async {
            loop {
                rx.changed()
                    .await
                    .map_err(|_| {
                        "on-demand job completion channel closed unexpectedly".to_string()
                    })?;
                let outcome = rx.borrow().clone();
                if outcome != JobOutcome::Pending {
                    return Ok(outcome);
                }
            }
        })
        .await
        {
            Ok(inner) => inner,
            Err(_elapsed) => Err(format!(
                "on-demand indexing timed out after {}s (job {job_id}); please retry shortly",
                ON_DEMAND_TIMEOUT.as_secs(),
            )),
        }
    }

    /// Called by the refresh worker after a job reaches a terminal state
    /// (success or failure). Sends the outcome to all waiting subscribers
    /// and removes the entry from the map.
    pub fn signal_completion(&self, job_id: i64, outcome: JobOutcome) {
        let mut waiters = self
            .waiters
            .write()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(tx) = waiters.remove(&job_id) {
            // send() returns Err only when there are no receivers, which is
            // harmless (the tool handler may have timed out already).
            let _ = tx.send(outcome);
        }
    }

    /// Returns a future that resolves when a worker wake-up is requested.
    /// Intended for use in the worker's `tokio::select!` alongside the
    /// poll-interval sleep.
    pub fn notified(&self) -> tokio::sync::futures::Notified<'_> {
        self.worker_notify.notified()
    }

    /// Wake the refresh worker without registering a completion waiter.
    /// Used by background tasks (e.g. registry discovery) that enqueue jobs
    /// via [`enqueue_or_get_refresh_job_id`] directly and do not wait for
    /// the result.
    pub fn notify_worker(&self) {
        self.worker_notify
            .notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn signal_before_wait_is_not_lost() {
        let coordinator = IndexingCoordinator::new();

        // Simulate: register a waiter, then signal before anyone calls
        // wait_for_job.
        {
            let mut waiters = coordinator
                .waiters
                .write()
                .unwrap();
            let (tx, _rx) = watch::channel(JobOutcome::Pending);
            waiters.insert(42, tx);
        }

        // Subscribe BEFORE signaling.
        let rx = {
            let waiters = coordinator
                .waiters
                .read()
                .unwrap();
            waiters
                .get(&42)
                .map(|tx| tx.subscribe())
        };

        coordinator.signal_completion(42, JobOutcome::Completed);

        // The receiver should see Completed even though we subscribed
        // before the signal.
        let mut rx = rx.unwrap();
        let _ = rx.changed().await;
        assert_eq!(*rx.borrow(), JobOutcome::Completed);
    }

    #[tokio::test]
    async fn wait_for_missing_waiter_returns_completed() {
        let coordinator = IndexingCoordinator::new();
        let result = coordinator
            .wait_for_job(999)
            .await;
        assert_eq!(result, Ok(JobOutcome::Completed));
    }
}
