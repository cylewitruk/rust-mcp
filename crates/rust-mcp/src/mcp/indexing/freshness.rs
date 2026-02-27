use serde_json::json;

use crate::db::indexing::{
    count_crate_releases_last_year, days_since_latest_crate_release, is_crate_refresh_due,
    mark_crate_freshness_checked, mark_crate_probe_failed,
};
use crate::integration::crates_io::{CratesIoClient, CratesIoCrateDetailResponse};
use crate::mcp::indexing::coordinator::{JobOutcome, PRIORITY_BACKFILL, PRIORITY_FRESHNESS};
use crate::mcp::server::McpServer;

#[derive(Debug, Default)]
pub(crate) struct InteractionRefreshOutcome {
    pub(crate) freshness_check_performed: bool,
    pub(crate) freshness_check_result: String,
    pub(crate) refresh_enqueued: bool,
    pub(crate) refresh_job_id: Option<String>,
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
    pub(crate) async fn ensure_freshness_for_interaction(
        &self,
        crate_id: i64,
        crate_name: &str,
        local_latest_version: &str,
    ) -> Result<InteractionRefreshOutcome, String> {
        let due = is_crate_refresh_due(&self.state.db, crate_id)
            .await
            .map_err(|e| format!("failed to evaluate freshness deadline for {crate_name}: {e}"))?;

        if !due {
            return Ok(InteractionRefreshOutcome {
                freshness_check_performed: false,
                freshness_check_result: "skipped".to_string(),
                ..Default::default()
            });
        }

        let releases_last_year = count_crate_releases_last_year(&self.state.db, crate_id)
            .await
            .map_err(|e| format!("failed to compute release cadence for {crate_name}: {e}"))?;

        let days_since_latest = days_since_latest_crate_release(&self.state.db, crate_id)
            .await
            .map_err(|e| format!("failed to compute recency for {crate_name}: {e}"))?;

        let (ttl_seconds, ttl_reason) = ttl_hint_seconds(days_since_latest, releases_last_year);

        let crates_io = CratesIoClient::new(&self.state);
        let detail: CratesIoCrateDetailResponse = match crates_io
            .fetch_crate_detail(crate_name)
            .await
        {
            Ok(detail) => detail,
            Err(error) => {
                mark_crate_probe_failed(&self.state.db, crate_id, &error)
                    .await
                    .map_err(|e| {
                        format!("failed to persist probe failure for {crate_name}: {e}")
                    })?;

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
            mark_crate_freshness_checked(&self.state.db, crate_id, ttl_seconds, ttl_reason)
                .await
                .map_err(|e| {
                    format!("failed to persist unchanged freshness for {crate_name}: {e}")
                })?;

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
                PRIORITY_FRESHNESS,
                true,
                json!({"trigger": "ttl_expired_changed"}),
            )
            .await?;

        mark_crate_freshness_checked(&self.state.db, crate_id, ttl_seconds, "changed_inline")
            .await
            .map_err(|e| format!("failed to persist changed freshness for {crate_name}: {e}"))?;

        Ok(InteractionRefreshOutcome {
            freshness_check_performed: true,
            freshness_check_result: "changed".to_string(),
            refresh_enqueued: true,
            refresh_job_id: Some(refresh_job_id),
        })
    }

    pub(crate) async fn backfill_missing_requested_version(
        &self,
        crate_name: &str,
    ) -> Result<Option<String>, String> {
        let job_id = self
            .state
            .indexing_coordinator
            .enqueue_on_demand(&self.state.db, crate_name, "crate")
            .await?;

        match self
            .state
            .indexing_coordinator
            .wait_for_job(job_id)
            .await
        {
            Ok(JobOutcome::Completed) => {}
            Ok(JobOutcome::Failed(msg)) => {
                return Err(format!("on-demand version backfill failed for '{crate_name}': {msg}"));
            }
            Ok(JobOutcome::Pending) => {
                return Err(format!(
                    "unexpected pending state after version backfill for '{crate_name}'"
                ));
            }
            Err(timeout_msg) => return Err(timeout_msg),
        }

        let deep_job_id = self
            .enqueue_refresh_job(
                crate_name,
                "crate_deep_refresh",
                PRIORITY_BACKFILL,
                true,
                json!({"trigger": "missing_requested_version"}),
            )
            .await?;
        Ok(Some(deep_job_id))
    }
}
