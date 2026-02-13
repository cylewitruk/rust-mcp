use std::collections::{HashMap, HashSet, VecDeque};

use rmcp::Json;

use super::models::{
    CrateCoreRow, CrateFeatureFlag, CrateFeatureRow, CrateFeaturesRequest, CrateFeaturesResponse,
    CrateVersionSelectionRow, ResponseFreshnessSource,
};
use super::server::McpServer;
use super::utils::{normalize_optional, normalize_required, value_to_string_vec};

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
    pub(super) async fn handle_crate_features(
        &self,
        request: CrateFeaturesRequest,
    ) -> Result<Json<CrateFeaturesResponse>, String> {
        let crate_name = normalize_required(request.crate_name, "crate_name")?;
        let requested_version = normalize_optional(request.version);

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

        let latest_version_row = sqlx::query_as::<_, CrateVersionSelectionRow>(
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

        let latest_version = latest_version_row
            .version
            .clone();
        let freshness_outcome = self
            .ensure_freshness_for_interaction(crate_row.id, &crate_row.name, &latest_version)
            .await?;

        let latest_version_row = if freshness_outcome.freshness_check_result == "changed" {
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
            latest_version_row
        };

        let mut refresh_enqueued = freshness_outcome.refresh_enqueued;
        let mut refresh_job_id = freshness_outcome
            .refresh_job_id
            .clone();

        let selected_version = if let Some(version) = requested_version {
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
            latest_version_row.clone()
        };

        let mut rows = sqlx::query_as::<_, CrateFeatureRow>(
            "SELECT feature_name, enables
             FROM crate_version_features
             WHERE crate_version_id = $1
             ORDER BY feature_name ASC",
        )
        .bind(selected_version.id)
        .fetch_all(&self.state.db)
        .await
        .map_err(|e| {
            format!(
                "feature flag query failed for {}@{}: {e}",
                crate_row.name, selected_version.version
            )
        })?;

        if rows.is_empty() {
            let _ = self
                .sync_single_crate(&crate_row.name, false)
                .await;
            rows = sqlx::query_as::<_, CrateFeatureRow>(
                "SELECT feature_name, enables
                 FROM crate_version_features
                 WHERE crate_version_id = $1
                 ORDER BY feature_name ASC",
            )
            .bind(selected_version.id)
            .fetch_all(&self.state.db)
            .await
            .map_err(|e| {
                format!(
                    "feature flag query failed after refresh for {}@{}: {e}",
                    crate_row.name, selected_version.version
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

        let freshness_check_result = freshness_outcome
            .freshness_check_result
            .clone();

        Ok(Json(CrateFeaturesResponse {
            crate_name: crate_row.name,
            selected_version: selected_version.version,
            latest_version: latest_version_row.version,
            default_features,
            feature_count: features.len(),
            features,
            freshness_check_performed: freshness_outcome.freshness_check_performed,
            freshness_check_result: freshness_check_result.clone(),
            refresh_enqueued,
            refresh_job_id,
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
            confidence: if default_feature_set.is_empty() {
                "medium".to_string()
            } else {
                "high".to_string()
            },
            next_best_calls: vec![
                "crate.intel".to_string(),
                "crate.versions".to_string(),
                "crate.graph".to_string(),
            ],
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
