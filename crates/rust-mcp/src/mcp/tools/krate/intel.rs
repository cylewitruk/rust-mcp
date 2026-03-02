use rmcp::Json;
pub use rust_mcp_types::types::krate::{
    CrateIntelAdvisory, CrateIntelDependency, CrateIntelDependent, CrateIntelRequest,
    CrateIntelResponse, CrateIntelVersion,
};

use crate::db::tools;
use crate::mcp::models::{ConfidenceAssessment, ConfidenceLevel};
use crate::mcp::server::McpServer;
use crate::mcp::utils::{
    build_crate_freshness_sources, dependents_limit, normalize_optional, normalize_required,
    readme_limit, truncate_optional_text, value_to_string_vec, version_limit,
};

impl McpServer {
    /// Handles the `crate.intel` tool call.
    pub async fn handle_crate_intel(
        &self,
        request: CrateIntelRequest,
    ) -> Result<Json<CrateIntelResponse>, String> {
        let crate_name = normalize_required(request.crate_name, "crate_name")?;
        let requested_version = normalize_optional(request.version);
        let versions_limit = version_limit(request.versions_limit);
        let dependents_limit = dependents_limit(request.dependents_limit);
        let readme_max_chars = readme_limit(request.readme_max_chars);

        let ctx = self
            .fetch_crate_context(&crate_name)
            .await?;
        let resolution = self
            .resolve_version_or_latest(&ctx, requested_version.as_deref())
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

        let advisory_rows = tools::fetch_crate_advisories_for_version(
            &self.state.db,
            ctx.crate_row.id,
            resolution.selected_version.id,
        )
        .await
        .map_err(|e| format!("advisory query failed for {}: {e}", ctx.crate_row.name))?;

        let (readme, readme_truncated) = truncate_optional_text(
            resolution
                .selected_version
                .readme,
            readme_max_chars,
        );

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

        let freshness_check_result = ctx
            .freshness_outcome
            .freshness_check_result
            .clone();
        let confidence_assessment = ConfidenceAssessment {
            level: ConfidenceLevel::High,
            reason: "crate intelligence assembled from indexed versions, deps, dependents, and \
                     advisories"
                .to_string(),
        };

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
            dependents,
            dependent_crate_count,
            advisories,
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
            next_best_calls: vec![
                "crate_versions".to_string(),
                "crate_graph".to_string(),
                "index_refresh".to_string(),
            ],
            provenance: "local_postgres_index".to_string(),
        }))
    }
}
