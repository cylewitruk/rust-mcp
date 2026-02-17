use serde_json::json;

use crate::integration::crates_io::{CratesIoClient, CratesIoCrateDetailResponse};
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

        let crates_io = CratesIoClient::new(&self.state);
        let detail: CratesIoCrateDetailResponse = match crates_io
            .fetch_crate_detail(crate_name)
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

    pub(crate) async fn backfill_missing_requested_version(
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
}
