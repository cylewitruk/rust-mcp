use rmcp::Json;
pub use rust_mcp_types::types::krate::{
    CrateIntelAdvisory, CrateIntelDependency, CrateIntelDependent, CrateIntelRequest,
    CrateIntelResponse, CrateIntelVersion,
};

use crate::db::tools;
use crate::mcp::models::{ConfidenceAssessment, ConfidenceLevel, ResponseFreshnessSource};
use crate::mcp::server::McpServer;
use crate::mcp::utils::{
    dependents_limit, normalize_optional, normalize_required, readme_limit, truncate_optional_text,
    value_to_string_vec, version_limit,
};

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

        let crate_row = tools::fetch_crate_core_by_name(&self.state.db, &crate_name)
            .await
            .map_err(|e| format!("crate lookup failed for {crate_name}: {e}"))?
            .ok_or_else(|| {
                format!("crate '{crate_name}' is not indexed locally; run index.sync_crates first")
            })?;

        let latest_version = tools::fetch_latest_crate_version(&self.state.db, crate_row.id)
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
            tools::fetch_latest_crate_version(&self.state.db, crate_row.id)
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
            let selected =
                tools::fetch_crate_version_by_name(&self.state.db, crate_row.id, &version)
                    .await
                    .map_err(|e| {
                        format!(
                            "selected version lookup failed for {}@{}: {e}",
                            crate_row.name, version
                        )
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

                tools::fetch_crate_version_by_name(&self.state.db, crate_row.id, &version)
                    .await
                    .map_err(|e| {
                        format!(
                            "selected version lookup failed after backfill for {}@{}: {e}",
                            crate_row.name, version
                        )
                    })?
                    .ok_or_else(|| {
                        format!(
                            "version '{}' for crate '{}' is not indexed locally (refresh \
                             attempted)",
                            version, crate_row.name
                        )
                    })?
            }
        } else {
            latest_version.clone()
        };

        let total_downloads = tools::fetch_crate_total_downloads(&self.state.db, crate_row.id)
            .await
            .map_err(|e| format!("download aggregation failed for {}: {e}", crate_row.name))?;

        let last_updated_at = tools::fetch_crate_last_published_at(&self.state.db, crate_row.id)
            .await
            .map_err(|e| format!("last-updated lookup failed for {}: {e}", crate_row.name))?
            .or(crate_row.updated_at.clone());

        let version_history_rows = tools::fetch_crate_version_history(
            &self.state.db,
            crate_row.id,
            i64::from(versions_limit),
        )
        .await
        .map_err(|e| format!("version history query failed for {}: {e}", crate_row.name))?;

        let dependency_rows =
            tools::fetch_crate_dependencies_for_version(&self.state.db, selected_version.id)
                .await
                .map_err(|e| {
                    format!(
                        "dependency query failed for {}@{}: {e}",
                        crate_row.name, selected_version.version
                    )
                })?;

        let dependent_crate_count =
            tools::fetch_dependent_crate_count(&self.state.db, crate_row.id)
                .await
                .map_err(|e| {
                    format!("dependent crate count query failed for {}: {e}", crate_row.name)
                })?;

        let dependent_rows = tools::fetch_dependent_crates(
            &self.state.db,
            crate_row.id,
            i64::from(dependents_limit),
        )
        .await
        .map_err(|e| format!("dependent crate query failed for {}: {e}", crate_row.name))?;

        let advisory_rows = tools::fetch_crate_advisories_for_version(
            &self.state.db,
            crate_row.id,
            selected_version.id,
        )
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
