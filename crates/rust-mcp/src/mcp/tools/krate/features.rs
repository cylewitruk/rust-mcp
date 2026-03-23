use std::collections::{HashMap, HashSet, VecDeque};

use rmcp::Json;
pub use rust_mcp_types::types::krate::{
    CrateFeatureFlag, CrateFeaturesRequest, CrateFeaturesResponse,
};
use tracing::warn;

use crate::db::tools;
use crate::mcp::models::{ConfidenceAssessment, ConfidenceLevel};
use crate::mcp::progress::ToolCallContext;
use crate::mcp::server::McpServer;
use crate::mcp::utils::{
    build_crate_freshness_sources, normalize_optional, normalize_required, value_to_string_vec,
};

fn split_enable_targets(values: Vec<String>) -> (Vec<String>, Vec<String>) {
    let mut features = Vec::new();
    let mut dependencies = Vec::new();

    for value in values {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Some(dep) = trimmed.strip_prefix("dep:") {
            let dep_name = dep.trim();
            if !dep_name.is_empty() {
                dependencies.push(dep_name.to_string());
            }
        } else {
            features.push(trimmed.to_string());
        }
    }

    features.sort();
    features.dedup();
    dependencies.sort();
    dependencies.dedup();

    (features, dependencies)
}

fn transitive_feature_enables(
    root: &str,
    feature_graph: &HashMap<String, Vec<String>>,
) -> Vec<String> {
    let mut queue = VecDeque::new();
    let mut seen = HashSet::new();

    for next in feature_graph
        .get(root)
        .cloned()
        .unwrap_or_default()
    {
        queue.push_back(next);
    }

    while let Some(feature) = queue.pop_front() {
        if !seen.insert(feature.clone()) {
            continue;
        }

        if let Some(children) = feature_graph.get(&feature) {
            for child in children {
                if !seen.contains(child) {
                    queue.push_back(child.clone());
                }
            }
        }
    }

    let mut out = seen
        .into_iter()
        .collect::<Vec<_>>();
    out.sort();
    out
}

impl McpServer {
    /// Handles the `crate_features` tool call.
    pub async fn handle_crate_features(
        &self,
        request: CrateFeaturesRequest,
        tcx: ToolCallContext,
    ) -> Result<Json<CrateFeaturesResponse>, String> {
        let crate_name = normalize_required(request.crate_name, "crate_name")?;
        let requested_version = normalize_optional(request.version);

        let ctx = self
            .fetch_crate_context(&crate_name, &tcx)
            .await?;
        let resolution = self
            .resolve_version_or_latest(&ctx, requested_version.as_deref(), &tcx)
            .await?;

        let mut rows =
            tools::list_crate_features_for_version(&self.state.db, resolution.selected_version.id)
                .await
                .map_err(|e| {
                    format!(
                        "feature flag query failed for {}@{}: {e}",
                        ctx.crate_row.name,
                        resolution
                            .selected_version
                            .version
                    )
                })?;

        if rows.is_empty() {
            if let Err(error) = self
                .sync_single_crate(&ctx.crate_row.name, false)
                .await
            {
                warn!(
                    crate_name = %ctx.crate_row.name,
                    %error,
                    "best-effort crate refresh failed while fetching features"
                );
            }
            rows = tools::list_crate_features_for_version(
                &self.state.db,
                resolution.selected_version.id,
            )
            .await
            .map_err(|e| {
                format!(
                    "feature flag query failed after refresh for {}@{}: {e}",
                    ctx.crate_row.name,
                    resolution
                        .selected_version
                        .version
                )
            })?;
        }

        let mut feature_graph = HashMap::new();
        let mut feature_dependency_enables = HashMap::new();

        for row in &rows {
            let enables = value_to_string_vec(&row.enables);
            let (feature_enables, dependency_enables) = split_enable_targets(enables);
            feature_graph.insert(row.feature_name.clone(), feature_enables);
            feature_dependency_enables.insert(row.feature_name.clone(), dependency_enables);
        }

        let default_features = feature_graph
            .get("default")
            .cloned()
            .unwrap_or_default();
        let default_feature_set = default_features
            .iter()
            .cloned()
            .collect::<HashSet<_>>();

        let features = rows
            .into_iter()
            .map(|row| CrateFeatureFlag {
                is_default: default_feature_set.contains(&row.feature_name),
                enables_features: feature_graph
                    .get(&row.feature_name)
                    .cloned()
                    .unwrap_or_default(),
                enables_dependencies: feature_dependency_enables
                    .get(&row.feature_name)
                    .cloned()
                    .unwrap_or_default(),
                transitive_enables: transitive_feature_enables(&row.feature_name, &feature_graph),
                name: row.feature_name,
            })
            .collect::<Vec<_>>();

        let freshness_check_result = ctx
            .freshness_outcome
            .freshness_check_result
            .clone();

        let confidence_assessment = if default_feature_set.is_empty() {
            ConfidenceAssessment {
                level: ConfidenceLevel::Medium,
                reason: "indexed features are present but default feature set is empty".to_string(),
            }
        } else {
            ConfidenceAssessment {
                level: ConfidenceLevel::High,
                reason: "feature graph and default set resolved from indexed metadata".to_string(),
            }
        };

        let suggested_next_tools = if features.is_empty() {
            vec!["index_crates".to_string(), "index_refresh".to_string(), "crate_intel".to_string()]
        } else {
            vec![
                "crate_intel".to_string(),
                "crate_api".to_string(),
                "dependency_feature_impact".to_string(),
            ]
        };

        Ok(Json(CrateFeaturesResponse {
            crate_name: ctx.crate_row.name,
            selected_version: resolution
                .selected_version
                .version,
            latest_version: ctx.latest_version.version,
            default_features,
            feature_count: features.len(),
            features,
            freshness_check_performed: ctx
                .freshness_outcome
                .freshness_check_performed,
            freshness_check_result: freshness_check_result.clone(),
            refresh_enqueued: resolution.refresh_enqueued,
            refresh_job_id: resolution.refresh_job_id,
            freshness: build_crate_freshness_sources(
                ctx.crate_row.updated_at,
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{split_enable_targets, transitive_feature_enables};

    #[test]
    fn split_enable_targets_separates_dependency_targets() {
        let (features, dependencies) = split_enable_targets(vec![
            "derive".to_string(),
            "dep:std".to_string(),
            "alloc".to_string(),
            "dep:serde_derive".to_string(),
        ]);

        assert_eq!(features, vec!["alloc".to_string(), "derive".to_string()]);
        assert_eq!(dependencies, vec!["serde_derive".to_string(), "std".to_string()]);
    }

    #[test]
    fn transitive_feature_enables_builds_closure() {
        let mut graph = HashMap::new();
        graph.insert("default".to_string(), vec!["std".to_string(), "alloc".to_string()]);
        graph.insert("std".to_string(), vec!["alloc".to_string()]);
        graph.insert("alloc".to_string(), vec!["smallvec".to_string()]);
        graph.insert("smallvec".to_string(), Vec::new());

        let result = transitive_feature_enables("default", &graph);
        assert_eq!(result, vec!["alloc".to_string(), "smallvec".to_string(), "std".to_string()]);
    }
}
