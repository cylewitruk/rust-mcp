use rmcp::{Json, schemars};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::FromRow;

use crate::mcp::models::{
    ConfidenceAssessment, ConfidenceLevel, CrateCoreRow, CrateVersionSelectionRow,
    ResponseFreshnessSource,
};
use crate::mcp::server::McpServer;
use crate::mcp::utils::{
    dependents_limit, normalize_optional, normalize_required, readme_limit, truncate_optional_text,
    value_to_string_vec, version_limit,
};

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CrateIntelRequest {
    pub crate_name: String,
    pub version: Option<String>,
    pub versions_limit: Option<u32>,
    pub dependents_limit: Option<u32>,
    pub readme_max_chars: Option<u32>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateIntelResponse {
    pub crate_name: String,
    pub selected_version: String,
    pub selected_rust_version: Option<String>,
    pub selected_version_published_at: Option<String>,
    pub latest_version: String,
    pub latest_rust_version: Option<String>,
    pub total_downloads: i64,
    pub last_updated_at: Option<String>,
    pub description: Option<String>,
    pub repository_url: Option<String>,
    pub docs_url: Option<String>,
    pub homepage_url: Option<String>,
    pub categories: Vec<String>,
    pub keywords: Vec<String>,
    pub readme: Option<String>,
    pub readme_truncated: bool,
    pub version_history: Vec<CrateIntelVersion>,
    pub dependencies: Vec<CrateIntelDependency>,
    pub dependents: Vec<CrateIntelDependent>,
    pub dependent_crate_count: i64,
    pub advisories: Vec<CrateIntelAdvisory>,
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
pub struct CrateIntelVersion {
    pub version: String,
    pub rust_version: Option<String>,
    pub published_at: Option<String>,
    pub yanked: bool,
    pub downloads: i64,
    pub has_advisory: bool,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateIntelDependency {
    pub crate_name: String,
    pub requirement: String,
    pub dependency_kind: String,
    pub optional: bool,
    pub features: Vec<String>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateIntelDependent {
    pub crate_name: String,
    pub latest_version: Option<String>,
    pub total_downloads: i64,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateIntelAdvisory {
    pub advisory_id: String,
    pub title: String,
    pub severity: Option<String>,
    pub url: Option<String>,
    pub affected_range: String,
    pub fixed_versions: Vec<String>,
    pub source: String,
}

#[derive(Debug, FromRow)]
pub(crate) struct CrateVersionHistoryRow {
    pub(crate) version: String,
    pub(crate) rust_version: Option<String>,
    pub(crate) published_at: Option<String>,
    pub(crate) yanked: bool,
    pub(crate) total_downloads: i64,
    pub(crate) has_advisory: bool,
}

#[derive(Debug, FromRow)]
pub(crate) struct CrateDependencyRow {
    pub(crate) dependency_name: String,
    pub(crate) requirement: String,
    pub(crate) dependency_kind: String,
    pub(crate) optional: bool,
    pub(crate) features: Value,
}

#[derive(Debug, FromRow)]
pub(crate) struct CrateDependentRow {
    pub(crate) crate_name: String,
    pub(crate) latest_version: Option<String>,
    pub(crate) total_downloads: i64,
}

#[derive(Debug, FromRow)]
pub(crate) struct CrateAdvisoryRow {
    pub(crate) advisory_id: String,
    pub(crate) title: String,
    pub(crate) severity: Option<String>,
    pub(crate) url: Option<String>,
    pub(crate) affected_range: String,
    pub(crate) fixed_versions: Value,
    pub(crate) source: String,
}

impl McpServer {
    pub(crate) async fn handle_crate_intel(
        &self,
        request: CrateIntelRequest,
    ) -> Result<Json<CrateIntelResponse>, String> {
        let crate_name = normalize_required(request.crate_name, "crate_name")?;
        let requested_version = normalize_optional(request.version);
        let versions_limit = version_limit(request.versions_limit);
        let dependents_limit = dependents_limit(request.dependents_limit);
        let readme_max_chars = readme_limit(request.readme_max_chars);

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

        let mut refresh_enqueued = freshness_outcome.refresh_enqueued;
        let mut refresh_job_id = freshness_outcome
            .refresh_job_id
            .clone();

        let selected_version = if let Some(version) = requested_version.clone() {
            let selected = sqlx::query_as::<_, CrateVersionSelectionRow>(
                "SELECT
                    id,
                    version,
                    rust_version,
                    published_at::TEXT AS published_at,
                    readme
                 FROM crate_versions
                 WHERE crate_id = $1 AND version = $2
                 LIMIT 1",
            )
            .bind(crate_row.id)
            .bind(&version)
            .fetch_optional(&self.state.db)
            .await
            .map_err(|e| {
                format!("selected version lookup failed for {}@{}: {e}", crate_row.name, version)
            })?;

            if let Some(selected) = selected {
                selected
            } else {
                let queued_job_id = self
                    .backfill_missing_requested_version(&crate_row.name)
                    .await?;
                if let Some(job_id) = queued_job_id {
                    refresh_enqueued = true;
                    refresh_job_id = Some(job_id);
                }

                sqlx::query_as::<_, CrateVersionSelectionRow>(
                    "SELECT
                        id,
                        version,
                        rust_version,
                        published_at::TEXT AS published_at,
                        readme
                     FROM crate_versions
                     WHERE crate_id = $1 AND version = $2
                     LIMIT 1",
                )
                .bind(crate_row.id)
                .bind(&version)
                .fetch_optional(&self.state.db)
                .await
                .map_err(|e| {
                    format!(
                        "selected version lookup failed after backfill for {}@{}: {e}",
                        crate_row.name, version
                    )
                })?
                .ok_or_else(|| {
                    format!(
                        "version '{}' for crate '{}' is not indexed locally (refresh attempted)",
                        version, crate_row.name
                    )
                })?
            }
        } else {
            latest_version.clone()
        };

        let total_downloads = sqlx::query_scalar::<_, i64>(
            "SELECT COALESCE(SUM(total_downloads), 0)::BIGINT
             FROM crate_versions
             WHERE crate_id = $1",
        )
        .bind(crate_row.id)
        .fetch_one(&self.state.db)
        .await
        .map_err(|e| format!("download aggregation failed for {}: {e}", crate_row.name))?;

        let last_updated_at = sqlx::query_scalar::<_, Option<String>>(
            "SELECT MAX(published_at)::TEXT
             FROM crate_versions
             WHERE crate_id = $1",
        )
        .bind(crate_row.id)
        .fetch_one(&self.state.db)
        .await
        .map_err(|e| format!("last-updated lookup failed for {}: {e}", crate_row.name))?
        .or(crate_row.updated_at.clone());

        let version_history_rows = sqlx::query_as::<_, CrateVersionHistoryRow>(
            "SELECT
                cv.version,
                cv.rust_version,
                cv.published_at::TEXT AS published_at,
                cv.yanked,
                cv.total_downloads,
                EXISTS(
                    SELECT 1
                    FROM advisory_matches am
                    WHERE am.version_id = cv.id
                ) AS has_advisory
             FROM crate_versions cv
             WHERE cv.crate_id = $1
             ORDER BY cv.published_at DESC NULLS LAST, cv.id DESC
             LIMIT $2",
        )
        .bind(crate_row.id)
        .bind(i64::from(versions_limit))
        .fetch_all(&self.state.db)
        .await
        .map_err(|e| format!("version history query failed for {}: {e}", crate_row.name))?;

        let dependency_rows = sqlx::query_as::<_, CrateDependencyRow>(
            "SELECT
                c.name AS dependency_name,
                d.requirement,
                d.dependency_kind,
                d.optional,
                d.features
             FROM dependency_edges d
             JOIN crates c ON c.id = d.to_crate_id
             WHERE d.from_version_id = $1
             ORDER BY c.name ASC",
        )
        .bind(selected_version.id)
        .fetch_all(&self.state.db)
        .await
        .map_err(|e| {
            format!(
                "dependency query failed for {}@{}: {e}",
                crate_row.name, selected_version.version
            )
        })?;

        let dependent_crate_count = sqlx::query_scalar::<_, i64>(
            "SELECT COALESCE(COUNT(DISTINCT cv_from.crate_id), 0)::BIGINT
             FROM dependency_edges d
             JOIN crate_versions cv_from ON cv_from.id = d.from_version_id
             WHERE d.to_crate_id = $1",
        )
        .bind(crate_row.id)
        .fetch_one(&self.state.db)
        .await
        .map_err(|e| format!("dependent crate count query failed for {}: {e}", crate_row.name))?;

        let dependent_rows = sqlx::query_as::<_, CrateDependentRow>(
            "WITH dependent_crates AS (
                 SELECT DISTINCT cv_from.crate_id
                 FROM dependency_edges d
                 JOIN crate_versions cv_from ON cv_from.id = d.from_version_id
                 WHERE d.to_crate_id = $1
             )
             SELECT
                 c.name AS crate_name,
                 lv.version AS latest_version,
                 COALESCE(lv.total_downloads, 0)::BIGINT AS total_downloads
             FROM dependent_crates dc
             JOIN crates c ON c.id = dc.crate_id
             LEFT JOIN LATERAL (
                 SELECT
                     cv.version,
                     cv.total_downloads
                 FROM crate_versions cv
                 WHERE cv.crate_id = c.id
                 ORDER BY cv.published_at DESC NULLS LAST, cv.id DESC
                 LIMIT 1
             ) lv ON true
             ORDER BY total_downloads DESC, c.name ASC
             LIMIT $2",
        )
        .bind(crate_row.id)
        .bind(i64::from(dependents_limit))
        .fetch_all(&self.state.db)
        .await
        .map_err(|e| format!("dependent crate query failed for {}: {e}", crate_row.name))?;

        let advisory_rows = sqlx::query_as::<_, CrateAdvisoryRow>(
            "SELECT
                advisory_id,
                title,
                severity,
                url,
                affected_range,
                fixed_versions,
                source
             FROM advisory_matches
             WHERE crate_id = $1 AND (version_id = $2 OR version_id IS NULL)
             ORDER BY advisory_id ASC",
        )
        .bind(crate_row.id)
        .bind(selected_version.id)
        .fetch_all(&self.state.db)
        .await
        .map_err(|e| format!("advisory query failed for {}: {e}", crate_row.name))?;

        let (readme, readme_truncated) =
            truncate_optional_text(selected_version.readme, readme_max_chars);

        let version_history = version_history_rows
            .into_iter()
            .map(|row| CrateIntelVersion {
                version: row.version,
                rust_version: row.rust_version,
                published_at: row.published_at,
                yanked: row.yanked,
                downloads: row.total_downloads,
                has_advisory: row.has_advisory,
            })
            .collect::<Vec<_>>();

        let dependencies = dependency_rows
            .into_iter()
            .map(|row| CrateIntelDependency {
                crate_name: row.dependency_name,
                requirement: row.requirement,
                dependency_kind: row.dependency_kind,
                optional: row.optional,
                features: value_to_string_vec(&row.features),
            })
            .collect::<Vec<_>>();

        let dependents = dependent_rows
            .into_iter()
            .map(|row| CrateIntelDependent {
                crate_name: row.crate_name,
                latest_version: row.latest_version,
                total_downloads: row.total_downloads,
            })
            .collect::<Vec<_>>();

        let advisories = advisory_rows
            .into_iter()
            .map(|row| CrateIntelAdvisory {
                advisory_id: row.advisory_id,
                title: row.title,
                severity: row.severity,
                url: row.url,
                affected_range: row.affected_range,
                fixed_versions: value_to_string_vec(&row.fixed_versions),
                source: row.source,
            })
            .collect::<Vec<_>>();

        let freshness_check_result = freshness_outcome
            .freshness_check_result
            .clone();
        let confidence_assessment = ConfidenceAssessment {
            level: ConfidenceLevel::High,
            reason: "crate intelligence assembled from indexed versions, deps, dependents, and \
                     advisories"
                .to_string(),
        };

        Ok(Json(CrateIntelResponse {
            crate_name: crate_row.name,
            selected_version: selected_version
                .version
                .clone(),
            selected_rust_version: selected_version.rust_version,
            selected_version_published_at: selected_version.published_at,
            latest_version: latest_version.version,
            latest_rust_version: latest_version.rust_version,
            total_downloads,
            last_updated_at,
            description: crate_row.description,
            repository_url: crate_row.repository_url,
            docs_url: crate_row.docs_url,
            homepage_url: crate_row.homepage_url,
            categories: crate_row.categories,
            keywords: crate_row.keywords,
            readme,
            readme_truncated,
            version_history,
            dependencies,
            dependents,
            dependent_crate_count,
            advisories,
            freshness_check_performed: freshness_outcome.freshness_check_performed,
            freshness_check_result: freshness_check_result.clone(),
            refresh_enqueued,
            refresh_job_id,
            freshness: vec![
                ResponseFreshnessSource {
                    source: "local_postgres_index".to_string(),
                    status: "fresh".to_string(),
                    checked_at: crate_row.updated_at.clone(),
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
                "crate.versions".to_string(),
                "crate.graph".to_string(),
                "index.refresh".to_string(),
            ],
            provenance: "local_postgres_index".to_string(),
        }))
    }
}
