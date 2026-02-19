use rmcp::Json;
pub use rust_mcp_types::types::krate::{
    CrateCompareRequest, CrateCompareResponse, CrateCompareSide,
};

use crate::db::models::{CrateCompareCountsRow, CrateCompareVersionRow};
use crate::db::tools;
use crate::mcp::indexing::freshness::InteractionRefreshOutcome;
use crate::mcp::models::{
    ConfidenceAssessment, ConfidenceLevel, CrateCoreRow, ResponseFreshnessSource,
};
use crate::mcp::server::McpServer;
use crate::mcp::utils::{normalize_optional, normalize_required};

#[derive(Debug)]
struct CompareSnapshot {
    crate_row: CrateCoreRow,
    latest_version: CrateCompareVersionRow,
    selected_version: CrateCompareVersionRow,
    counts: CrateCompareCountsRow,
    freshness_outcome: InteractionRefreshOutcome,
}

fn maintenance_score(side: &CrateCompareSide) -> f64 {
    let downloads_score = (side.total_downloads.max(0) as f64 + 1.0).log10() * 8.0;
    let dependents_score = (side
        .dependent_crate_count
        .max(0) as f64
        + 1.0)
        .log10()
        * 10.0;
    let dependencies_penalty = side.dependency_count.max(0) as f64 * 0.15;
    let features_bonus = (side.feature_count.max(0) as f64).min(20.0) * 0.2;
    let advisories_penalty = side.advisory_count.max(0) as f64 * 12.0;
    let msrv_bonus = if side
        .selected_rust_version
        .is_some()
    {
        2.0
    } else {
        0.0
    };
    let yanked_penalty = if side.yanked { 40.0 } else { 0.0 };

    (50.0 + downloads_score + dependents_score + features_bonus + msrv_bonus
        - dependencies_penalty
        - advisories_penalty
        - yanked_penalty)
        .clamp(0.0, 100.0)
}

fn compare_recommendation(
    left: &CrateCompareSide,
    right: &CrateCompareSide,
) -> (Option<String>, Vec<String>) {
    let score_gap = (left.maintenance_score - right.maintenance_score).abs();
    if score_gap < 3.0 {
        return (None, vec!["both crates score similarly on available local signals".to_string()]);
    }

    let (winner, loser) = if left.maintenance_score > right.maintenance_score {
        (left, right)
    } else {
        (right, left)
    };

    let mut reasons = Vec::<String>::new();
    if winner.advisory_count < loser.advisory_count {
        reasons.push("fewer indexed advisories".to_string());
    }
    if !winner.yanked && loser.yanked {
        reasons.push("selected version is not yanked".to_string());
    }
    if winner.total_downloads > loser.total_downloads {
        reasons.push("higher adoption by downloads".to_string());
    }
    if winner.dependent_crate_count > loser.dependent_crate_count {
        reasons.push("higher ecosystem usage by dependents".to_string());
    }
    if winner
        .selected_rust_version
        .is_some()
        && loser
            .selected_rust_version
            .is_none()
    {
        reasons.push("declares MSRV metadata".to_string());
    }
    if reasons.is_empty() {
        reasons.push("higher maintenance score on local index signals".to_string());
    }

    (
        Some(winner.crate_name.clone()),
        reasons
            .into_iter()
            .take(3)
            .collect(),
    )
}

fn confidence_assessment(
    left: &CrateCompareSide,
    right: &CrateCompareSide,
) -> ConfidenceAssessment {
    if left.total_downloads == 0
        && left.dependent_crate_count == 0
        && right.total_downloads == 0
        && right.dependent_crate_count == 0
    {
        return ConfidenceAssessment {
            level: ConfidenceLevel::Low,
            reason: "both crates have sparse local adoption signals".to_string(),
        };
    }

    if left
        .selected_published_at
        .is_none()
        || right
            .selected_published_at
            .is_none()
    {
        return ConfidenceAssessment {
            level: ConfidenceLevel::Medium,
            reason: "one or both selected versions are missing publish timestamps".to_string(),
        };
    }

    ConfidenceAssessment {
        level: ConfidenceLevel::High,
        reason: "comparison uses indexed adoption, risk, and dependency metadata for both crates"
            .to_string(),
    }
}

fn merge_freshness_result(
    left: &InteractionRefreshOutcome,
    right: &InteractionRefreshOutcome,
) -> String {
    let statuses = [&left.freshness_check_result, &right.freshness_check_result];
    if statuses
        .iter()
        .any(|status| status.as_str() == "failed")
    {
        return "failed".to_string();
    }
    if statuses
        .iter()
        .any(|status| status.as_str() == "changed")
    {
        return "changed".to_string();
    }
    if statuses
        .iter()
        .any(|status| status.as_str() == "unchanged")
    {
        return "unchanged".to_string();
    }
    "skipped".to_string()
}

impl McpServer {
    async fn crate_compare_snapshot(
        &self,
        crate_name: String,
        requested_version: Option<String>,
    ) -> Result<CompareSnapshot, String> {
        let ctx = self
            .fetch_crate_context(&crate_name)
            .await?;

        let latest_version = tools::fetch_compare_latest_version(&self.state.db, ctx.crate_row.id)
            .await
            .map_err(|e| format!("latest version lookup failed for {crate_name}: {e}"))?
            .ok_or_else(|| {
                format!(
                    "crate '{}' has no indexed versions yet; run index.sync_crates first",
                    ctx.crate_row.name
                )
            })?;

        let selected_version = if let Some(version) = requested_version {
            let selected =
                tools::fetch_compare_version_by_name(&self.state.db, ctx.crate_row.id, &version)
                    .await
                    .map_err(|e| {
                        format!(
                            "selected version lookup failed for {}@{}: {e}",
                            ctx.crate_row.name, version
                        )
                    })?;

            if let Some(selected) = selected {
                selected
            } else {
                let _ = self
                    .backfill_missing_requested_version(&ctx.crate_row.name)
                    .await?;

                tools::fetch_compare_version_by_name(&self.state.db, ctx.crate_row.id, &version)
                    .await
                    .map_err(|e| {
                        format!(
                            "selected version lookup failed after backfill for {}@{}: {e}",
                            ctx.crate_row.name, version
                        )
                    })?
                    .ok_or_else(|| {
                        format!(
                            "version '{}' for crate '{}' is not indexed locally (refresh \
                             attempted)",
                            version, ctx.crate_row.name
                        )
                    })?
            }
        } else {
            latest_version.clone()
        };

        let counts =
            tools::fetch_compare_counts(&self.state.db, selected_version.id, ctx.crate_row.id)
                .await
                .map_err(|e| {
                    format!(
                        "crate.compare count aggregation failed for {}: {e}",
                        ctx.crate_row.name
                    )
                })?;

        Ok(CompareSnapshot {
            crate_row: ctx.crate_row,
            latest_version,
            selected_version,
            counts,
            freshness_outcome: ctx.freshness_outcome,
        })
    }

    pub(crate) async fn handle_crate_compare(
        &self,
        request: CrateCompareRequest,
    ) -> Result<Json<CrateCompareResponse>, String> {
        let left_crate = normalize_required(request.left_crate, "left_crate")?;
        let right_crate = normalize_required(request.right_crate, "right_crate")?;
        let left_version = normalize_optional(request.left_version);
        let right_version = normalize_optional(request.right_version);

        if left_crate.eq_ignore_ascii_case(&right_crate) {
            return Err("left_crate and right_crate must be different values".to_string());
        }

        let left_snapshot = self
            .crate_compare_snapshot(left_crate.clone(), left_version)
            .await?;
        let right_snapshot = self
            .crate_compare_snapshot(right_crate.clone(), right_version)
            .await?;

        let mut left = CrateCompareSide {
            crate_name: left_snapshot
                .crate_row
                .name
                .clone(),
            selected_version: left_snapshot
                .selected_version
                .version,
            latest_version: left_snapshot
                .latest_version
                .version,
            selected_rust_version: left_snapshot
                .selected_version
                .rust_version,
            selected_published_at: left_snapshot
                .selected_version
                .published_at,
            license_expression: left_snapshot
                .selected_version
                .license_expression,
            total_downloads: left_snapshot
                .selected_version
                .total_downloads,
            dependent_crate_count: left_snapshot
                .counts
                .dependent_count,
            dependency_count: left_snapshot
                .counts
                .dependency_count,
            feature_count: left_snapshot
                .counts
                .feature_count,
            advisory_count: left_snapshot
                .counts
                .advisory_count,
            yanked: left_snapshot
                .selected_version
                .yanked,
            maintenance_score: 0.0,
        };
        left.maintenance_score = maintenance_score(&left);

        let mut right = CrateCompareSide {
            crate_name: right_snapshot
                .crate_row
                .name
                .clone(),
            selected_version: right_snapshot
                .selected_version
                .version,
            latest_version: right_snapshot
                .latest_version
                .version,
            selected_rust_version: right_snapshot
                .selected_version
                .rust_version,
            selected_published_at: right_snapshot
                .selected_version
                .published_at,
            license_expression: right_snapshot
                .selected_version
                .license_expression,
            total_downloads: right_snapshot
                .selected_version
                .total_downloads,
            dependent_crate_count: right_snapshot
                .counts
                .dependent_count,
            dependency_count: right_snapshot
                .counts
                .dependency_count,
            feature_count: right_snapshot
                .counts
                .feature_count,
            advisory_count: right_snapshot
                .counts
                .advisory_count,
            yanked: right_snapshot
                .selected_version
                .yanked,
            maintenance_score: 0.0,
        };
        right.maintenance_score = maintenance_score(&right);

        let (recommendation, recommendation_reasons) = compare_recommendation(&left, &right);
        let confidence_assessment = confidence_assessment(&left, &right);

        Ok(Json(CrateCompareResponse {
            left,
            right,
            recommendation,
            recommendation_reasons,
            freshness: vec![
                ResponseFreshnessSource {
                    source: format!("crates_io:{}", left_snapshot.crate_row.name),
                    status: left_snapshot
                        .freshness_outcome
                        .freshness_check_result
                        .clone(),
                    checked_at: left_snapshot
                        .crate_row
                        .updated_at,
                },
                ResponseFreshnessSource {
                    source: format!("crates_io:{}", right_snapshot.crate_row.name),
                    status: right_snapshot
                        .freshness_outcome
                        .freshness_check_result
                        .clone(),
                    checked_at: right_snapshot
                        .crate_row
                        .updated_at,
                },
                ResponseFreshnessSource {
                    source: "rustsec".to_string(),
                    status: "checked".to_string(),
                    checked_at: None,
                },
            ],
            freshness_check_performed: left_snapshot
                .freshness_outcome
                .freshness_check_performed
                || right_snapshot
                    .freshness_outcome
                    .freshness_check_performed,
            freshness_check_result: merge_freshness_result(
                &left_snapshot.freshness_outcome,
                &right_snapshot.freshness_outcome,
            ),
            refresh_enqueued: left_snapshot
                .freshness_outcome
                .refresh_enqueued
                || right_snapshot
                    .freshness_outcome
                    .refresh_enqueued,
            refresh_job_id: left_snapshot
                .freshness_outcome
                .refresh_job_id
                .or(right_snapshot
                    .freshness_outcome
                    .refresh_job_id),
            confidence: confidence_assessment
                .level
                .as_str()
                .to_string(),
            confidence_assessment,
            next_best_calls: vec![
                format!("crate.intel(crate_name='{}')", left_crate),
                format!("crate.intel(crate_name='{}')", right_crate),
                format!("crate.api(crate_name='{}')", left_crate),
                format!("crate.api(crate_name='{}')", right_crate),
            ],
            provenance: "local_postgres_index+derived_compare_score:v1".to_string(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::{CrateCompareSide, compare_recommendation, maintenance_score};

    fn side(crate_name: &str) -> CrateCompareSide {
        CrateCompareSide {
            crate_name: crate_name.to_string(),
            selected_version: "1.0.0".to_string(),
            latest_version: "1.0.0".to_string(),
            selected_rust_version: Some("1.70".to_string()),
            selected_published_at: Some("2025-01-01T00:00:00Z".to_string()),
            license_expression: Some("MIT".to_string()),
            total_downloads: 10_000,
            dependent_crate_count: 50,
            dependency_count: 12,
            feature_count: 8,
            advisory_count: 0,
            yanked: false,
            maintenance_score: 0.0,
        }
    }

    #[test]
    fn maintenance_score_penalizes_advisories_and_yanks() {
        let mut healthy = side("healthy");
        let mut risky = side("risky");
        risky.advisory_count = 2;
        risky.yanked = true;

        healthy.maintenance_score = maintenance_score(&healthy);
        risky.maintenance_score = maintenance_score(&risky);

        assert!(healthy.maintenance_score > risky.maintenance_score);
    }

    #[test]
    fn recommendation_uses_higher_score_side() {
        let mut left = side("lefty");
        let mut right = side("righty");
        right.advisory_count = 3;
        left.maintenance_score = maintenance_score(&left);
        right.maintenance_score = maintenance_score(&right);

        let (winner, reasons) = compare_recommendation(&left, &right);

        assert_eq!(winner, Some("lefty".to_string()));
        assert!(!reasons.is_empty());
    }
}
