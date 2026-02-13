use rmcp::Json;

use super::models::{
    CrateCoreRow, CrateVersionTimelineItem, CrateVersionTimelineRow, CrateVersionsRequest,
    CrateVersionsResponse,
};
use super::server::McpServer;
use super::utils::{normalize_required, version_limit};

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
    pub(super) async fn handle_crate_versions(
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

        let latest_version = sqlx::query_scalar::<_, Option<String>>(
            "SELECT version
             FROM crate_versions
             WHERE crate_id = $1
             ORDER BY published_at DESC NULLS LAST, id DESC
             LIMIT 1",
        )
        .bind(crate_row.id)
        .fetch_one(&self.state.db)
        .await
        .map_err(|e| format!("latest version lookup failed for {crate_name}: {e}"))?
        .ok_or_else(|| {
            format!(
                "crate '{}' has no indexed versions yet; run index.sync_crates first",
                crate_row.name
            )
        })?;

        let freshness_outcome = self
            .ensure_freshness_for_interaction(crate_row.id, &crate_row.name, &latest_version)
            .await?;

        let latest_version = if freshness_outcome.freshness_check_result == "changed" {
            sqlx::query_scalar::<_, Option<String>>(
                "SELECT version
                 FROM crate_versions
                 WHERE crate_id = $1
                 ORDER BY published_at DESC NULLS LAST, id DESC
                 LIMIT 1",
            )
            .bind(crate_row.id)
            .fetch_one(&self.state.db)
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
                let is_latest = row.version == latest_version;

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

        Ok(Json(CrateVersionsResponse {
            crate_name: crate_row.name,
            count: versions.len(),
            versions,
            freshness_check_performed: freshness_outcome.freshness_check_performed,
            freshness_check_result: freshness_check_result.clone(),
            refresh_enqueued: freshness_outcome.refresh_enqueued,
            refresh_job_id: freshness_outcome.refresh_job_id,
            freshness: vec![
                super::models::ResponseFreshnessSource {
                    source: "local_postgres_index".to_string(),
                    status: "fresh".to_string(),
                    checked_at: crate_row.updated_at,
                },
                super::models::ResponseFreshnessSource {
                    source: "crates.io".to_string(),
                    status: freshness_check_result,
                    checked_at: None,
                },
            ],
            confidence: "high".to_string(),
            next_best_calls: vec![
                "crate.intel".to_string(),
                "crate.graph".to_string(),
                "index.refresh".to_string(),
            ],
            provenance: "local_postgres_index".to_string(),
        }))
    }
}
