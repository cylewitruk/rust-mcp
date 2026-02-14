use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

use rmcp::Json;

use super::models::{
    ConfidenceAssessment, ConfidenceLevel, CrateCoreRow, CrateVersionSelectionRow,
    DependencyFeatureImpactEntry, DependencyFeatureImpactRequest, DependencyFeatureImpactResponse,
    FeatureImpactDependencyRow, FeatureImpactFeatureRow, ResponseFreshnessSource,
};
use super::server::McpServer;
use super::utils::{
    feature_impact_heavy_threshold, normalize_optional, normalize_required, value_to_string_vec,
};

fn split_feature_targets(values: Vec<String>) -> (Vec<String>, Vec<String>) {
    let mut enabled_features = Vec::new();
    let mut enabled_dependencies = Vec::new();

    for value in values {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Some(dep) = trimmed.strip_prefix("dep:") {
            let dep = dep.trim();
            if !dep.is_empty() {
                enabled_dependencies.push(dep.to_string());
            }
        } else {
            enabled_features.push(trimmed.to_string());
        }
    }

    enabled_features.sort();
    enabled_features.dedup();
    enabled_dependencies.sort();
    enabled_dependencies.dedup();

    (enabled_features, enabled_dependencies)
}

fn expand_feature_dependencies(
    roots: &[String],
    feature_to_features: &HashMap<String, Vec<String>>,
    feature_to_dependencies: &HashMap<String, Vec<String>>,
) -> BTreeSet<String> {
    let mut queue = VecDeque::new();
    let mut visited = HashSet::<String>::new();
    let mut dependencies = BTreeSet::<String>::new();

    for root in roots {
        queue.push_back(root.clone());
    }

    while let Some(feature) = queue.pop_front() {
        if !visited.insert(feature.clone()) {
            continue;
        }

        if let Some(deps) = feature_to_dependencies.get(&feature) {
            for dep in deps {
                dependencies.insert(dep.clone());
            }
        }

        if let Some(children) = feature_to_features.get(&feature) {
            for child in children {
                if !visited.contains(child) {
                    queue.push_back(child.clone());
                }
            }
        }
    }

    dependencies
}

impl McpServer {
    pub(super) async fn handle_dependency_feature_impact(
        &self,
        request: DependencyFeatureImpactRequest,
    ) -> Result<Json<DependencyFeatureImpactResponse>, String> {
        let crate_name = normalize_required(request.crate_name, "crate_name")?;
        let requested_version = normalize_optional(request.version);
        let heavy_threshold = feature_impact_heavy_threshold(request.heavy_threshold);

        let mut features = request
            .features
            .into_iter()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>();
        features.sort();
        features.dedup();

        if features.is_empty() {
            return Err("features must include at least one non-empty feature name".to_string());
        }

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
            latest_version.clone()
        };

        let feature_rows = sqlx::query_as::<_, FeatureImpactFeatureRow>(
            "SELECT feature_name, enables
             FROM crate_version_features
             WHERE crate_version_id = $1
             ORDER BY feature_name ASC",
        )
        .bind(selected_version.id)
        .fetch_all(&self.state.db)
        .await
        .map_err(|e| format!("dependency.feature_impact feature query failed: {e}"))?;

        let mut feature_to_features = HashMap::<String, Vec<String>>::new();
        let mut feature_to_dependencies = HashMap::<String, Vec<String>>::new();
        for row in feature_rows {
            let enables = value_to_string_vec(&row.enables);
            let (enabled_features, enabled_dependencies) = split_feature_targets(enables);
            feature_to_features.insert(row.feature_name.clone(), enabled_features);
            feature_to_dependencies.insert(row.feature_name, enabled_dependencies);
        }

        let dependency_rows = sqlx::query_as::<_, FeatureImpactDependencyRow>(
            "SELECT
                c.name AS dependency_name,
                de.optional
             FROM dependency_edges de
             JOIN crates c ON c.id = de.to_crate_id
             WHERE de.from_version_id = $1",
        )
        .bind(selected_version.id)
        .fetch_all(&self.state.db)
        .await
        .map_err(|e| format!("dependency.feature_impact dependency query failed: {e}"))?;

        let mut all_dependencies = BTreeSet::<String>::new();
        let mut baseline_dependencies = BTreeSet::<String>::new();
        for row in dependency_rows {
            all_dependencies.insert(row.dependency_name.clone());
            if !row.optional {
                baseline_dependencies.insert(row.dependency_name);
            }
        }

        let mut per_feature = Vec::<DependencyFeatureImpactEntry>::new();
        let mut heavy_features = Vec::<String>::new();
        let mut combined_extras = BTreeSet::<String>::new();

        for feature in &features {
            let enabled_dependencies = expand_feature_dependencies(
                std::slice::from_ref(feature),
                &feature_to_features,
                &feature_to_dependencies,
            );

            let additional_dependencies = enabled_dependencies
                .into_iter()
                .filter(|dep| {
                    all_dependencies.contains(dep) && !baseline_dependencies.contains(dep)
                })
                .collect::<Vec<_>>();

            if additional_dependencies.len() as u32 >= heavy_threshold {
                heavy_features.push(feature.clone());
            }

            for dep in &additional_dependencies {
                combined_extras.insert(dep.clone());
            }

            per_feature.push(DependencyFeatureImpactEntry {
                feature: feature.clone(),
                additional_dependency_count: additional_dependencies.len(),
                additional_dependencies,
            });
        }

        let confidence_assessment = if per_feature
            .iter()
            .all(|entry| entry.additional_dependency_count == 0)
        {
            ConfidenceAssessment {
                level: ConfidenceLevel::Medium,
                reason: "feature enables were resolved but no additional indexed dependencies \
                         were detected"
                    .to_string(),
            }
        } else {
            ConfidenceAssessment {
                level: ConfidenceLevel::High,
                reason: "feature impact computed from indexed feature metadata and dependency \
                         edges"
                    .to_string(),
            }
        };

        let freshness_check_result = freshness_outcome
            .freshness_check_result
            .clone();

        Ok(Json(DependencyFeatureImpactResponse {
            crate_name: crate_row.name,
            selected_version: selected_version.version,
            latest_version: latest_version.version,
            features,
            heavy_threshold,
            baseline_dependency_count: baseline_dependencies.len(),
            combined_dependency_count: baseline_dependencies.len() + combined_extras.len(),
            per_feature,
            heavy_features,
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
            confidence: confidence_assessment
                .level
                .as_str()
                .to_string(),
            confidence_assessment,
            next_best_calls: vec![
                "crate.features".to_string(),
                "crate.graph".to_string(),
                "dependency.resolve".to_string(),
            ],
            provenance: "local_postgres_index(crate_version_features, dependency_edges)"
                .to_string(),
        }))
    }
}
