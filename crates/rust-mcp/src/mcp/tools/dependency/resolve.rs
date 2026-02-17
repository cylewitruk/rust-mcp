use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

use rmcp::Json;
pub use rust_mcp_types::types::dependency::{
    DependencyResolveConflict, DependencyResolveFeatureSummary, DependencyResolveInputDependency,
    DependencyResolveRequest, DependencyResolveResolvedDependency,
    DependencyResolveResolvedVersion, DependencyResolveResponse,
};
use semver::{Version, VersionReq};
use serde_json::Value as JsonValue;
use sqlx::FromRow;
use toml::Value;

use crate::mcp::models::{ConfidenceAssessment, ConfidenceLevel};
use crate::mcp::server::McpServer;
use crate::mcp::utils::{dependency_resolve_limit, normalize_required};

#[derive(Debug, Clone, FromRow)]
pub(crate) struct DependencyResolveCrateRow {
    pub(crate) id: i64,
    pub(crate) name: String,
}

#[derive(Debug, Clone, FromRow)]
pub(crate) struct DependencyResolveVersionRow {
    pub(crate) id: i64,
    pub(crate) version: String,
    pub(crate) yanked: bool,
}

#[derive(Debug, Clone, FromRow)]
pub(crate) struct DependencyResolveEdgeRow {
    pub(crate) to_crate_name: String,
    pub(crate) requirement: String,
    pub(crate) optional: bool,
    pub(crate) features: JsonValue,
}

#[derive(Debug, Clone)]
struct NormalizedDependency {
    name: String,
    version_req: Option<String>,
    source: String,
}

#[derive(Debug, Clone)]
struct SelectedDependency {
    crate_name: String,
    version_id: i64,
    version: String,
    yanked: bool,
}

fn parse_semver_for_matching(version: &str) -> Option<Version> {
    Version::parse(version)
        .ok()
        .or_else(|| Version::parse(version.trim_start_matches('v')).ok())
}

fn parse_requirement(value: &Value) -> Option<String> {
    if let Some(req) = value.as_str() {
        let trimmed = req.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    let table = value.as_table()?;
    let version = table
        .get("version")?
        .as_str()?;
    let trimmed = version.trim();
    if trimmed.is_empty() { None } else { Some(trimmed.to_string()) }
}

fn collect_manifest_dependencies(
    manifest: &str,
) -> Result<Vec<DependencyResolveInputDependency>, String> {
    let parsed =
        toml::from_str::<Value>(manifest).map_err(|e| format!("invalid Cargo.toml: {e}"))?;

    let mut out = Vec::<DependencyResolveInputDependency>::new();
    let Some(root) = parsed.as_table() else {
        return Ok(out);
    };

    let mut add_deps_from = |table: Option<&toml::map::Map<String, Value>>| {
        let Some(table) = table else {
            return;
        };

        for (name, spec) in table {
            let trimmed_name = name.trim();
            if trimmed_name.is_empty() {
                continue;
            }

            out.push(DependencyResolveInputDependency {
                name: trimmed_name.to_string(),
                version_req: parse_requirement(spec),
            });
        }
    };

    add_deps_from(
        root.get("dependencies")
            .and_then(Value::as_table),
    );
    add_deps_from(
        root.get("dev-dependencies")
            .and_then(Value::as_table),
    );
    add_deps_from(
        root.get("build-dependencies")
            .and_then(Value::as_table),
    );

    if let Some(targets) = root
        .get("target")
        .and_then(Value::as_table)
    {
        for target_value in targets.values() {
            let Some(target_table) = target_value.as_table() else {
                continue;
            };

            add_deps_from(
                target_table
                    .get("dependencies")
                    .and_then(Value::as_table),
            );
            add_deps_from(
                target_table
                    .get("dev-dependencies")
                    .and_then(Value::as_table),
            );
            add_deps_from(
                target_table
                    .get("build-dependencies")
                    .and_then(Value::as_table),
            );
        }
    }

    Ok(out)
}

fn resolve_manifest_path(raw_path: String) -> Result<PathBuf, String> {
    let normalized = normalize_required(raw_path, "cargo_toml_path")?;
    let candidate = PathBuf::from(&normalized);

    let final_path = if candidate.is_absolute() {
        candidate
    } else {
        std::env::current_dir()
            .map_err(|e| format!("failed to resolve current directory: {e}"))?
            .join(candidate)
    };

    if final_path
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.eq_ignore_ascii_case("Cargo.toml"))
        .unwrap_or(false)
    {
        Ok(final_path)
    } else {
        Err("cargo_toml_path must point to a Cargo.toml file".to_string())
    }
}

fn normalize_inputs(
    explicit_dependencies: Option<Vec<DependencyResolveInputDependency>>,
    manifest_dependencies: Vec<DependencyResolveInputDependency>,
    additions: Option<Vec<DependencyResolveInputDependency>>,
) -> Vec<NormalizedDependency> {
    let mut out = Vec::<NormalizedDependency>::new();

    out.extend(
        explicit_dependencies
            .unwrap_or_default()
            .into_iter()
            .map(|entry| NormalizedDependency {
                name: entry.name,
                version_req: entry.version_req,
                source: "explicit".to_string(),
            }),
    );

    out.extend(
        manifest_dependencies
            .into_iter()
            .map(|entry| NormalizedDependency {
                name: entry.name,
                version_req: entry.version_req,
                source: "cargo_toml".to_string(),
            }),
    );

    out.extend(
        additions
            .unwrap_or_default()
            .into_iter()
            .map(|entry| NormalizedDependency {
                name: entry.name,
                version_req: entry.version_req,
                source: "addition".to_string(),
            }),
    );

    let mut merged = HashMap::<String, NormalizedDependency>::new();
    for entry in out {
        let name = entry.name.trim();
        if name.is_empty() {
            continue;
        }

        let key = name.to_ascii_lowercase();
        let normalized = NormalizedDependency {
            name: name.to_string(),
            version_req: entry
                .version_req
                .map(|req| req.trim().to_string())
                .filter(|req| !req.is_empty()),
            source: entry.source,
        };

        match merged.get(&key) {
            None => {
                merged.insert(key, normalized);
            }
            Some(existing)
                if existing.version_req.is_none()
                    && normalized
                        .version_req
                        .is_some() =>
            {
                merged.insert(key, normalized);
            }
            _ => {}
        }
    }

    let mut values = merged
        .into_values()
        .collect::<Vec<_>>();
    values.sort_by(|left, right| {
        left.name
            .to_ascii_lowercase()
            .cmp(
                &right
                    .name
                    .to_ascii_lowercase(),
            )
    });
    values
}

fn collect_feature_names(value: &JsonValue) -> BTreeSet<String> {
    match value {
        JsonValue::Array(entries) => entries
            .iter()
            .filter_map(JsonValue::as_str)
            .map(ToString::to_string)
            .collect(),
        _ => BTreeSet::new(),
    }
}

fn evaluate_confidence(
    selected_count: usize,
    conflict_count: usize,
    unindexed_count: usize,
) -> ConfidenceAssessment {
    if selected_count == 0 {
        return ConfidenceAssessment {
            level: ConfidenceLevel::Low,
            reason: "no requested dependencies could be resolved from local index".to_string(),
        };
    }

    if unindexed_count > 0 {
        return ConfidenceAssessment {
            level: ConfidenceLevel::Medium,
            reason: format!(
                "{unindexed_count} requested dependencies were not present in the local index"
            ),
        };
    }

    if conflict_count > 0 {
        return ConfidenceAssessment {
            level: ConfidenceLevel::Medium,
            reason: format!(
                "resolution found {conflict_count} compatibility conflicts across selected \
                 versions"
            ),
        };
    }

    ConfidenceAssessment {
        level: ConfidenceLevel::High,
        reason: "selected versions and indexed dependency requirements were mutually compatible"
            .to_string(),
    }
}

impl McpServer {
    pub(crate) async fn handle_dependency_resolve(
        &self,
        request: DependencyResolveRequest,
    ) -> Result<Json<DependencyResolveResponse>, String> {
        let limit = dependency_resolve_limit(request.limit);
        let check_features = request
            .check_features
            .unwrap_or(false);

        let manifest_dependencies = if let Some(path) = request.cargo_toml_path {
            let resolved = resolve_manifest_path(path)?;
            let manifest = tokio::fs::read_to_string(&resolved)
                .await
                .map_err(|e| format!("failed to read {}: {e}", resolved.display()))?;
            collect_manifest_dependencies(&manifest)?
        } else {
            Vec::new()
        };

        let mut requested =
            normalize_inputs(request.dependencies, manifest_dependencies, request.additions);

        if requested.is_empty() {
            return Err("provide dependencies, cargo_toml_path, or additions to evaluate \
                        resolution"
                .to_string());
        }

        if requested.len() > limit as usize {
            requested.truncate(limit as usize);
        }

        let mut conflicts = Vec::<DependencyResolveConflict>::new();
        let mut selected = Vec::<SelectedDependency>::new();
        let mut unindexed_count = 0usize;

        for dependency in &requested {
            let crate_row = sqlx::query_as::<_, DependencyResolveCrateRow>(
                "SELECT id, name FROM crates WHERE name = $1 LIMIT 1",
            )
            .bind(&dependency.name)
            .fetch_optional(&self.state.db)
            .await
            .map_err(|e| format!("crate lookup failed for {}: {e}", dependency.name))?;

            let Some(crate_row) = crate_row else {
                unindexed_count += 1;
                conflicts.push(DependencyResolveConflict {
                    dependency_name: dependency.name.clone(),
                    requirement: dependency.version_req.clone(),
                    message: "dependency is not present in local index; run index.sync_crates \
                              first"
                        .to_string(),
                });
                continue;
            };

            let versions = sqlx::query_as::<_, DependencyResolveVersionRow>(
                "SELECT id, version, yanked
                 FROM crate_versions
                 WHERE crate_id = $1
                 ORDER BY published_at DESC NULLS LAST, id DESC",
            )
            .bind(crate_row.id)
            .fetch_all(&self.state.db)
            .await
            .map_err(|e| format!("version lookup failed for {}: {e}", crate_row.name))?;

            let selected_version = if let Some(version_req) = dependency
                .version_req
                .as_deref()
            {
                let req = VersionReq::parse(version_req).map_err(|e| {
                    format!(
                        "invalid version requirement '{}' for {}: {e}",
                        version_req, dependency.name
                    )
                })?;

                versions
                    .iter()
                    .filter_map(|row| {
                        let parsed = parse_semver_for_matching(&row.version)?;
                        if req.matches(&parsed) { Some((parsed, row.clone())) } else { None }
                    })
                    .max_by(|left, right| left.0.cmp(&right.0))
                    .map(|(_, row)| row)
            } else {
                versions.first().cloned()
            };

            let Some(selected_version) = selected_version else {
                conflicts.push(DependencyResolveConflict {
                    dependency_name: dependency.name.clone(),
                    requirement: dependency.version_req.clone(),
                    message: "no indexed version satisfies the declared version requirement"
                        .to_string(),
                });
                continue;
            };

            selected.push(SelectedDependency {
                crate_name: crate_row.name,
                version_id: selected_version.id,
                version: selected_version.version,
                yanked: selected_version.yanked,
            });
        }

        let selected_ids = selected
            .iter()
            .map(|entry| entry.version_id)
            .collect::<Vec<_>>();
        let selected_by_crate = selected
            .iter()
            .map(|entry| {
                (
                    entry
                        .crate_name
                        .to_ascii_lowercase(),
                    entry.clone(),
                )
            })
            .collect::<HashMap<_, _>>();

        let mut feature_edges_evaluated = 0usize;
        let mut optional_targets = BTreeSet::<String>::new();
        let mut feature_names = BTreeSet::<String>::new();

        if !selected_ids.is_empty() {
            let edges = sqlx::query_as::<_, DependencyResolveEdgeRow>(
                "SELECT
                    to_c.name AS to_crate_name,
                    de.requirement,
                    de.optional,
                    de.features
                 FROM dependency_edges de
                 JOIN crates to_c ON to_c.id = de.to_crate_id
                 WHERE de.from_version_id = ANY($1::BIGINT[])",
            )
            .bind(&selected_ids)
            .fetch_all(&self.state.db)
            .await
            .map_err(|e| format!("dependency edge lookup failed: {e}"))?;

            feature_edges_evaluated = edges.len();

            let mut target_requirements = HashMap::<String, Vec<String>>::new();

            for edge in edges {
                if edge.optional {
                    optional_targets.insert(edge.to_crate_name.clone());
                }
                for feature in collect_feature_names(&edge.features) {
                    feature_names.insert(feature);
                }

                target_requirements
                    .entry(
                        edge.to_crate_name
                            .to_ascii_lowercase(),
                    )
                    .or_default()
                    .push(edge.requirement.clone());

                if let Some(target_selected) = selected_by_crate.get(
                    &edge
                        .to_crate_name
                        .to_ascii_lowercase(),
                ) {
                    let target_version = parse_semver_for_matching(&target_selected.version);
                    let requirement = VersionReq::parse(&edge.requirement).ok();

                    if let (Some(target_version), Some(requirement)) = (target_version, requirement)
                        && !requirement.matches(&target_version)
                    {
                        conflicts.push(DependencyResolveConflict {
                            dependency_name: edge.to_crate_name.clone(),
                            requirement: Some(edge.requirement.clone()),
                            message: format!(
                                "selected version {} does not satisfy transitive requirement '{}'",
                                target_selected.version, edge.requirement
                            ),
                        });
                    }
                }
            }

            for (target_crate_key, requirements) in target_requirements {
                if requirements.len() < 2 {
                    continue;
                }

                let parsed_requirements = requirements
                    .iter()
                    .filter_map(|req| VersionReq::parse(req).ok())
                    .collect::<Vec<_>>();

                if parsed_requirements.len() < 2 {
                    continue;
                }

                let candidate_versions = sqlx::query_as::<_, DependencyResolveVersionRow>(
                    "SELECT cv.id, cv.version, cv.yanked
                     FROM crate_versions cv
                     JOIN crates c ON c.id = cv.crate_id
                     WHERE LOWER(c.name) = $1",
                )
                .bind(&target_crate_key)
                .fetch_all(&self.state.db)
                .await
                .map_err(|e| {
                    format!("version lookup failed for transitive crate {target_crate_key}: {e}")
                })?;

                let has_satisfying = candidate_versions
                    .iter()
                    .any(|row| {
                        let Some(parsed) = parse_semver_for_matching(&row.version) else {
                            return false;
                        };
                        parsed_requirements
                            .iter()
                            .all(|req| req.matches(&parsed))
                    });

                if !has_satisfying {
                    conflicts.push(DependencyResolveConflict {
                        dependency_name: target_crate_key,
                        requirement: None,
                        message: "no indexed version satisfies all transitive requirements"
                            .to_string(),
                    });
                }
            }
        }

        let resolved_versions = selected
            .iter()
            .map(|entry| DependencyResolveResolvedVersion {
                name: entry.crate_name.clone(),
                version: entry.version.clone(),
                yanked: entry.yanked,
            })
            .collect::<Vec<_>>();

        let confidence_assessment =
            evaluate_confidence(resolved_versions.len(), conflicts.len(), unindexed_count);

        Ok(Json(DependencyResolveResponse {
            input_dependencies: requested
                .into_iter()
                .map(|entry| DependencyResolveResolvedDependency {
                    name: entry.name,
                    version_req: entry.version_req,
                    source: entry.source,
                })
                .collect(),
            resolvable: conflicts.is_empty(),
            resolved_versions,
            conflicts,
            check_features,
            feature_unification_summary: check_features.then_some(
                DependencyResolveFeatureSummary {
                    dependency_edges_evaluated: feature_edges_evaluated,
                    unique_optional_dependencies: optional_targets.len(),
                    unique_feature_flags_referenced: feature_names.len(),
                },
            ),
            confidence: confidence_assessment
                .level
                .as_str()
                .to_string(),
            confidence_assessment,
            next_best_calls: vec![
                "dependency.audit".to_string(),
                "crate.graph".to_string(),
                "crate.features".to_string(),
            ],
            provenance: "local_postgres_index(crates, crate_versions, dependency_edges)"
                .to_string(),
        }))
    }
}
