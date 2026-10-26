use std::collections::BTreeSet;

use rmcp::Json;
pub use rust_mcp_types::types::krate::{
    CrateIntelAdvisory, CrateIntelDependency, CrateIntelDependent, CrateIntelGitHub,
    CrateIntelRequest, CrateIntelResponse, CrateIntelVersion,
};

use crate::db::indexing::fetch_github_metadata;
use crate::db::tools;
use crate::mcp::advisories::{self, Applicability};
use crate::mcp::models::{ConfidenceAssessment, ConfidenceLevel};
use crate::mcp::progress::ToolCallContext;
use crate::mcp::server::McpServer;
use crate::mcp::utils::{
    build_crate_freshness_sources, dependents_limit, normalize_optional, normalize_required,
    readme_limit, truncate_optional_text, value_to_string_vec, version_limit,
};

impl McpServer {
    /// Handles the `crate_intel` tool call.
    pub async fn handle_crate_intel(
        &self,
        request: CrateIntelRequest,
        tcx: ToolCallContext,
    ) -> Result<Json<CrateIntelResponse>, String> {
        let crate_name = normalize_required(request.crate_name, "crate_name")?;
        let requested_version = normalize_optional(request.version);
        let versions_limit = version_limit(request.versions_limit);
        let dependents_limit = dependents_limit(request.dependents_limit);
        let readme_max_chars = readme_limit(request.readme_max_chars);
        let include_advisory_details = request
            .include_advisory_details
            .unwrap_or(false);

        let ctx = self
            .fetch_crate_context(&crate_name, &tcx)
            .await?;
        let resolution = self
            .resolve_version_or_latest(&ctx, requested_version.as_deref(), &tcx)
            .await?;

        let total_downloads = tools::fetch_crate_total_downloads(&self.state.db, ctx.crate_row.id)
            .await
            .map_err(|e| format!("download aggregation failed for {}: {e}", ctx.crate_row.name))?;

        let last_updated_at =
            tools::fetch_crate_last_published_at(&self.state.db, ctx.crate_row.id)
                .await
                .map_err(|e| format!("last-updated lookup failed for {}: {e}", ctx.crate_row.name))?
                .or(ctx
                    .crate_row
                    .updated_at
                    .clone());

        let version_history_rows = tools::fetch_crate_version_history(
            &self.state.db,
            ctx.crate_row.id,
            i64::from(versions_limit),
        )
        .await
        .map_err(|e| format!("version history query failed for {}: {e}", ctx.crate_row.name))?;

        let dependency_rows = tools::fetch_crate_dependencies_for_version(
            &self.state.db,
            resolution.selected_version.id,
        )
        .await
        .map_err(|e| {
            format!(
                "dependency query failed for {}@{}: {e}",
                ctx.crate_row.name,
                resolution
                    .selected_version
                    .version
            )
        })?;

        let dependent_crate_count =
            tools::fetch_dependent_crate_count(&self.state.db, ctx.crate_row.id)
                .await
                .map_err(|e| {
                    format!("dependent crate count query failed for {}: {e}", ctx.crate_row.name)
                })?;

        let dependent_rows = tools::fetch_dependent_crates(
            &self.state.db,
            ctx.crate_row.id,
            i64::from(dependents_limit),
        )
        .await
        .map_err(|e| format!("dependent crate query failed for {}: {e}", ctx.crate_row.name))?;

        let advisory_rows = tools::fetch_advisory_rows(&self.state.db, &[ctx.crate_row.id])
            .await
            .map_err(|e| format!("advisory query failed for {}: {e}", ctx.crate_row.name))?;

        let dependencies_synced =
            tools::fetch_dependencies_synced(&self.state.db, resolution.selected_version.id)
                .await
                .map_err(|e| {
                    format!("dependency coverage lookup failed for {}: {e}", ctx.crate_row.name)
                })?;

        let github = match fetch_github_metadata(&self.state.db, ctx.crate_row.id).await {
            Ok(Some(meta)) => Some(CrateIntelGitHub {
                owner: meta.owner,
                repo: meta.repo,
                stars: meta.stargazers_count as u64,
                forks: meta.forks_count as u64,
                open_issues: meta.open_issues_count as u64,
                archived: meta.archived,
                last_push: meta.pushed_at,
                license: meta.license_spdx,
                contributors: meta
                    .contributor_count
                    .map(|c| c as u64),
                last_commit_at: meta.last_commit_at,
                last_commit_message: meta.last_commit_message,
                recent_commit_count: meta
                    .recent_commit_count
                    .map(|c| c as u64),
            }),
            _ => None,
        };

        let (readme, readme_truncated) = match readme_max_chars {
            Some(max_chars) => truncate_optional_text(
                resolution
                    .selected_version
                    .readme
                    .clone(),
                max_chars,
            ),
            None => (None, false),
        };

        let version_history = version_history_rows
            .into_iter()
            .map(|row| {
                let counts =
                    advisories::advisory_counts(&advisory_rows, row.version_id, &row.version);
                CrateIntelVersion {
                    has_advisory: counts.affected > 0,
                    advisory_unknown_count: counts.unknown,
                    version: row.version,
                    rust_version: row.rust_version,
                    published_at: row.published_at,
                    yanked: row.yanked,
                    downloads: row.total_downloads,
                }
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

        let selected = &resolution.selected_version;
        let applicable =
            advisories::advisories_for_version(&advisory_rows, selected.id, &selected.version);
        let advisory_ids = advisory_rows
            .iter()
            .map(|row| row.advisory_id.as_str())
            .collect::<BTreeSet<_>>();
        let advisories_not_affecting = advisory_ids.len() - applicable.len();
        let unknown_advisories = applicable
            .iter()
            .filter(|advisory| advisory.applicability == Applicability::Unknown)
            .count();
        let advisories = applicable
            .into_iter()
            .map(|advisory| {
                let row = advisory.row;
                CrateIntelAdvisory {
                    advisory_id: row.advisory_id.clone(),
                    title: row.title.clone(),
                    severity: row.severity.clone(),
                    url: row.url.clone(),
                    affected_range: row.affected_range.clone(),
                    fixed_versions: value_to_string_vec(&row.fixed_versions),
                    source: row.source.clone(),
                    applicability: advisory
                        .applicability
                        .as_str()
                        .to_string(),
                    details: row
                        .details
                        .clone()
                        .filter(|_| include_advisory_details),
                    affected_functions: row.affected_functions.clone(),
                    cwe_ids: row.cwe_ids.clone(),
                }
            })
            .collect::<Vec<_>>();

        let freshness_check_result = ctx
            .freshness_outcome
            .freshness_check_result
            .clone();
        let mut gaps = Vec::new();
        if !dependencies_synced {
            gaps.push(format!(
                "dependencies for {} have not been synced (index_crates with this version fetches \
                 them)",
                selected.version
            ));
        }
        if unknown_advisories > 0 {
            gaps.push(format!(
                "{unknown_advisories} advisories could not be evaluated against {}",
                selected.version
            ));
        }
        let confidence_assessment = if gaps.is_empty() {
            ConfidenceAssessment {
                level: ConfidenceLevel::High,
                reason: "versions, dependencies and advisory applicability resolved from the index"
                    .to_string(),
            }
        } else {
            ConfidenceAssessment {
                level: ConfidenceLevel::Medium,
                reason: gaps.join("; "),
            }
        };

        let mut suggested_next_tools =
            vec!["crate_features".to_string(), "crate_api".to_string(), "crate_graph".to_string()];
        if !dependencies_synced {
            suggested_next_tools.insert(0, "index_crates".to_string());
        }

        Ok(Json(CrateIntelResponse {
            crate_name: ctx.crate_row.name,
            selected_version: resolution
                .selected_version
                .version
                .clone(),
            selected_rust_version: resolution
                .selected_version
                .rust_version,
            selected_version_published_at: resolution
                .selected_version
                .published_at,
            latest_version: ctx.latest_version.version,
            latest_rust_version: ctx
                .latest_version
                .rust_version,
            total_downloads,
            last_updated_at,
            description: ctx.crate_row.description,
            repository_url: ctx.crate_row.repository_url,
            docs_url: ctx.crate_row.docs_url,
            homepage_url: ctx.crate_row.homepage_url,
            categories: ctx.crate_row.categories,
            keywords: ctx.crate_row.keywords,
            readme,
            readme_truncated,
            version_history,
            dependencies,
            dependency_coverage: if dependencies_synced { "synced" } else { "not_synced" }
                .to_string(),
            dependents,
            dependent_crate_count,
            github,
            advisories,
            advisories_not_affecting,
            freshness_check_performed: ctx
                .freshness_outcome
                .freshness_check_performed,
            freshness_check_result: freshness_check_result.clone(),
            refresh_enqueued: resolution.refresh_enqueued,
            refresh_job_id: resolution.refresh_job_id,
            freshness: build_crate_freshness_sources(
                ctx.crate_row
                    .updated_at
                    .clone(),
                &freshness_check_result,
            ),
            confidence: confidence_assessment
                .level
                .as_str()
                .to_string(),
            confidence_assessment,
            suggested_next_tools,
            provenance: "local_postgres_index".to_string(),
        }))
    }
}
