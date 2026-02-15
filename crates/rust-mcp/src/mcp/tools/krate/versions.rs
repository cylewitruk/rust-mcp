use rmcp::{Json, schemars};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::mcp::models::{
    ConfidenceAssessment, ConfidenceLevel, CrateCoreRow, CrateVersionSelectionRow,
    ResponseFreshnessSource,
};
use crate::mcp::server::McpServer;
use crate::mcp::utils::{normalize_required, version_limit};

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CrateVersionsRequest {
    pub crate_name: String,
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateVersionsResponse {
    pub crate_name: String,
    pub latest_rust_version: Option<String>,
    pub count: usize,
    pub versions: Vec<CrateVersionTimelineItem>,
    pub freshness_check_performed: bool,
    pub freshness_check_result: String,
    pub refresh_enqueued: bool,
    pub refresh_job_id: Option<String>,
    pub freshness: Vec<ResponseFreshnessSource>,
    pub confidence: String,
    pub confidence_assessment: ConfidenceAssessment,
    pub next_best_calls: Vec<String>,
    pub provenance: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateVersionTimelineItem {
    pub version: String,
    pub rust_version: Option<String>,
    pub published_at: Option<String>,
    pub yanked: bool,
    pub downloads: i64,
    pub advisory_count: i64,
    pub release_age_days: Option<i64>,
    pub is_latest: bool,
    pub adoption_signal: String,
    pub markers: Vec<String>,
}

#[derive(Debug, FromRow)]
pub(crate) struct CrateVersionTimelineRow {
    pub(crate) version: String,
    pub(crate) rust_version: Option<String>,
    pub(crate) published_at: Option<String>,
    pub(crate) yanked: bool,
    pub(crate) total_downloads: i64,
    pub(crate) advisory_count: i64,
    pub(crate) release_age_days: Option<i64>,
}

fn adoption_signal(downloads: i64, yanked: bool) -> String {
    if yanked {
        return "deprecated".to_string();
    }
    if downloads >= 1_000_000 {
        return "high".to_string();
    }
    if downloads >= 100_000 {
        return "medium".to_string();
    }
    "low".to_string()
}

impl McpServer {
    pub(crate) async fn handle_crate_versions(
        &self,
        request: CrateVersionsRequest,
    ) -> Result<Json<CrateVersionsResponse>, String> {
        let crate_name = normalize_required(request.crate_name, "crate_name")?;
        let limit = version_limit(request.limit);

        let crate_row = sqlx::query_as::<_, CrateCoreRow>(
            "SELECT
                id,
                name,
                description,
                repository_url,
                docs_url,
                homepage_url,
                categories,
                keywords,
                updated_at::TEXT AS updated_at
             FROM crates
             WHERE name = $1",
        )
        .bind(&crate_name)
        .fetch_optional(&self.state.db)
        .await
        .map_err(|e| format!("crate lookup failed for {crate_name}: {e}"))?
        .ok_or_else(|| {
            format!("crate '{crate_name}' is not indexed locally; run index.sync_crates first")
        })?;

        let latest_version = sqlx::query_as::<_, CrateVersionSelectionRow>(
            "SELECT
                id,
                version,
                rust_version,
                published_at::TEXT AS published_at,
                readme
             FROM crate_versions
             WHERE crate_id = $1
             ORDER BY published_at DESC NULLS LAST, id DESC
             LIMIT 1",
        )
        .bind(crate_row.id)
        .fetch_optional(&self.state.db)
        .await
        .map_err(|e| format!("latest version lookup failed for {crate_name}: {e}"))?
        .ok_or_else(|| {
            format!(
                "crate '{}' has no indexed versions yet; run index.sync_crates first",
                crate_row.name
            )
        })?;

        let freshness_outcome = self
            .ensure_freshness_for_interaction(
                crate_row.id,
                &crate_row.name,
                &latest_version.version,
            )
            .await?;

        let latest_version = if freshness_outcome.freshness_check_result == "changed" {
            sqlx::query_as::<_, CrateVersionSelectionRow>(
                "SELECT
                    id,
                    version,
                    rust_version,
                    published_at::TEXT AS published_at,
                    readme
                 FROM crate_versions
                 WHERE crate_id = $1
                 ORDER BY published_at DESC NULLS LAST, id DESC
                 LIMIT 1",
            )
            .bind(crate_row.id)
            .fetch_optional(&self.state.db)
            .await
            .map_err(|e| format!("latest version relookup failed for {crate_name}: {e}"))?
            .ok_or_else(|| {
                format!(
                    "crate '{}' has no indexed versions yet; run index.sync_crates first",
                    crate_row.name
                )
            })?
        } else {
            latest_version
        };

        let rows = sqlx::query_as::<_, CrateVersionTimelineRow>(
            "SELECT
                cv.version,
                cv.rust_version,
                cv.published_at::TEXT AS published_at,
                cv.yanked,
                COALESCE(cv.total_downloads, 0)::BIGINT AS total_downloads,
                COUNT(am.id)::BIGINT AS advisory_count,
                CASE
                    WHEN cv.published_at IS NULL THEN NULL
                    ELSE EXTRACT(EPOCH FROM (NOW() - cv.published_at))::BIGINT / 86400
                END AS release_age_days
             FROM crate_versions cv
             LEFT JOIN advisory_matches am ON am.version_id = cv.id
             WHERE cv.crate_id = $1
             GROUP BY cv.id, cv.version, cv.published_at, cv.yanked, cv.total_downloads
             ORDER BY cv.published_at DESC NULLS LAST, cv.id DESC
             LIMIT $2",
        )
        .bind(crate_row.id)
        .bind(i64::from(limit))
        .fetch_all(&self.state.db)
        .await
        .map_err(|e| format!("version timeline query failed for {}: {e}", crate_row.name))?;

        let versions = rows
            .into_iter()
            .map(|row| {
                let mut markers = Vec::new();
                let is_latest = row.version == latest_version.version;

                if is_latest {
                    markers.push("latest".to_string());
                }
                if row.yanked {
                    markers.push("yanked".to_string());
                }
                if row.advisory_count > 0 {
                    markers.push("security_advisory".to_string());
                }
                if let Some(days) = row.release_age_days
                    && days >= 365
                {
                    markers.push("legacy".to_string());
                }

                CrateVersionTimelineItem {
                    version: row.version,
                    rust_version: row.rust_version,
                    published_at: row.published_at,
                    yanked: row.yanked,
                    downloads: row.total_downloads,
                    advisory_count: row.advisory_count,
                    release_age_days: row.release_age_days,
                    is_latest,
                    adoption_signal: adoption_signal(row.total_downloads, row.yanked),
                    markers,
                }
            })
            .collect::<Vec<_>>();

        let freshness_check_result = freshness_outcome
            .freshness_check_result
            .clone();

        let confidence_assessment = if versions.is_empty() {
            ConfidenceAssessment {
                level: ConfidenceLevel::Low,
                reason: "no versions were returned from local index for selected crate".to_string(),
            }
        } else {
            ConfidenceAssessment {
                level: ConfidenceLevel::High,
                reason: "version timeline resolved from indexed crate versions and advisories"
                    .to_string(),
            }
        };

        Ok(Json(CrateVersionsResponse {
            crate_name: crate_row.name,
            latest_rust_version: latest_version.rust_version,
            count: versions.len(),
            versions,
            freshness_check_performed: freshness_outcome.freshness_check_performed,
            freshness_check_result: freshness_check_result.clone(),
            refresh_enqueued: freshness_outcome.refresh_enqueued,
            refresh_job_id: freshness_outcome.refresh_job_id,
            freshness: vec![
                ResponseFreshnessSource {
                    source: "local_postgres_index".to_string(),
                    status: "fresh".to_string(),
                    checked_at: crate_row.updated_at,
                },
                ResponseFreshnessSource {
                    source: "crates.io".to_string(),
                    status: freshness_check_result,
                    checked_at: None,
                },
            ],
            confidence: confidence_assessment
                .level
                .as_str()
                .to_string(),
            confidence_assessment,
            next_best_calls: vec![
                "crate.intel".to_string(),
                "crate.graph".to_string(),
                "index.refresh".to_string(),
            ],
            provenance: "local_postgres_index".to_string(),
        }))
    }
}
