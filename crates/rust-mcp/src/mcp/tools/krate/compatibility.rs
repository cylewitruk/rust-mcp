use rmcp::Json;
pub use rust_mcp_types::types::krate::{
    CrateCompatibilityMatrixEntry, CrateCompatibilityMatrixRequest,
    CrateCompatibilityMatrixResponse, CrateCompatibilityRequest, CrateCompatibilityResponse,
};

use crate::db::tools;
use crate::mcp::models::{ConfidenceAssessment, ConfidenceLevel};
use crate::mcp::server::McpServer;
use crate::mcp::tools::dependency::resolve::{
    DependencyResolveInputDependency, DependencyResolveRequest,
};
use crate::mcp::utils::{
    compatibility_matrix_max_pairs, compatibility_matrix_version_limit, dedupe_strings,
    normalize_required,
};

fn exact_req(version: Option<String>) -> Option<String> {
    version
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(|value| format!("={value}"))
}

impl McpServer {
    /// Handles the `crate.compatibility` tool call.
    pub async fn handle_crate_compatibility(
        &self,
        request: CrateCompatibilityRequest,
    ) -> Result<Json<CrateCompatibilityResponse>, String> {
        let left_crate = normalize_required(request.left_crate, "left_crate")?;
        let right_crate = normalize_required(request.right_crate, "right_crate")?;

        let left_version = request
            .left_version
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let right_version = request
            .right_version
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let check_features = request
            .check_features
            .unwrap_or(true);

        let resolve_request = DependencyResolveRequest {
            dependencies: Some(vec![
                DependencyResolveInputDependency {
                    name: left_crate.clone(),
                    version_req: exact_req(left_version.clone()),
                },
                DependencyResolveInputDependency {
                    name: right_crate.clone(),
                    version_req: exact_req(right_version.clone()),
                },
            ]),
            cargo_toml: None,
            additions: None,
            check_features: Some(check_features),
            limit: Some(2),
        };

        let resolve_response = self
            .handle_dependency_resolve(resolve_request)
            .await?
            .0;

        Ok(Json(CrateCompatibilityResponse {
            left_crate,
            left_version,
            right_crate,
            right_version,
            resolvable: resolve_response.resolvable,
            resolved_versions: resolve_response.resolved_versions,
            conflicts: resolve_response.conflicts,
            check_features: resolve_response.check_features,
            feature_unification_summary: resolve_response.feature_unification_summary,
            confidence: resolve_response.confidence,
            confidence_assessment: resolve_response.confidence_assessment,
            next_best_calls: vec![
                "dependency_resolve".to_string(),
                "crate_graph".to_string(),
                "crate_versions".to_string(),
            ],
            provenance: "dependency_resolve wrapper with exact version constraints".to_string(),
        }))
    }

    /// Handles the `crate.compatibility_matrix` tool call.
    pub async fn handle_crate_compatibility_matrix(
        &self,
        request: CrateCompatibilityMatrixRequest,
    ) -> Result<Json<CrateCompatibilityMatrixResponse>, String> {
        let left_crate = normalize_required(request.left_crate, "left_crate")?;
        let right_crate = normalize_required(request.right_crate, "right_crate")?;
        let check_features = request
            .check_features
            .unwrap_or(true);
        let version_limit = compatibility_matrix_version_limit(request.version_limit);
        let max_pairs = compatibility_matrix_max_pairs(request.max_pairs);

        let left_versions = if let Some(values) = request.left_versions {
            dedupe_strings(values)
        } else {
            let crate_row = tools::fetch_dependency_crate_by_name(&self.state.db, &left_crate)
                .await
                .map_err(|e| format!("failed to load crate row for {left_crate}: {e}"))?
                .ok_or_else(|| format!("crate '{left_crate}' is not indexed locally"))?;
            tools::list_dependency_versions_by_crate(&self.state.db, crate_row.id)
                .await
                .map_err(|e| format!("failed to load versions for {left_crate}: {e}"))?
                .into_iter()
                .take(version_limit as usize)
                .map(|row| row.version)
                .collect::<Vec<_>>()
        };

        let right_versions = if let Some(values) = request.right_versions {
            dedupe_strings(values)
        } else {
            let crate_row = tools::fetch_dependency_crate_by_name(&self.state.db, &right_crate)
                .await
                .map_err(|e| format!("failed to load crate row for {right_crate}: {e}"))?
                .ok_or_else(|| format!("crate '{right_crate}' is not indexed locally"))?;
            tools::list_dependency_versions_by_crate(&self.state.db, crate_row.id)
                .await
                .map_err(|e| format!("failed to load versions for {right_crate}: {e}"))?
                .into_iter()
                .take(version_limit as usize)
                .map(|row| row.version)
                .collect::<Vec<_>>()
        };

        if left_versions.is_empty() {
            return Err(format!("no candidate versions available for '{}'", left_crate));
        }
        if right_versions.is_empty() {
            return Err(format!("no candidate versions available for '{}'", right_crate));
        }

        let mut compatible_pairs = Vec::<CrateCompatibilityMatrixEntry>::new();
        let mut incompatible_pairs = Vec::<CrateCompatibilityMatrixEntry>::new();
        let mut tested = 0usize;

        for left_version in &left_versions {
            for right_version in &right_versions {
                if tested >= max_pairs as usize {
                    break;
                }

                let resolve = self
                    .handle_dependency_resolve(DependencyResolveRequest {
                        dependencies: Some(vec![
                            DependencyResolveInputDependency {
                                name: left_crate.clone(),
                                version_req: Some(format!("={left_version}")),
                            },
                            DependencyResolveInputDependency {
                                name: right_crate.clone(),
                                version_req: Some(format!("={right_version}")),
                            },
                        ]),
                        cargo_toml: None,
                        additions: None,
                        check_features: Some(check_features),
                        limit: Some(2),
                    })
                    .await?
                    .0;

                let entry = CrateCompatibilityMatrixEntry {
                    left_version: left_version.clone(),
                    right_version: right_version.clone(),
                    resolvable: resolve.resolvable,
                    conflict_messages: resolve
                        .conflicts
                        .into_iter()
                        .map(|conflict| conflict.message)
                        .collect(),
                };

                if entry.resolvable {
                    compatible_pairs.push(entry);
                } else {
                    incompatible_pairs.push(entry);
                }

                tested += 1;
            }

            if tested >= max_pairs as usize {
                break;
            }
        }

        let confidence_assessment = if tested == 0 {
            ConfidenceAssessment {
                level: ConfidenceLevel::Low,
                reason: "no version pairs were evaluated".to_string(),
            }
        } else if tested < (left_versions.len() * right_versions.len()) {
            ConfidenceAssessment {
                level: ConfidenceLevel::Medium,
                reason: format!("evaluated {tested} version pairs (truncated by max_pairs limit)"),
            }
        } else {
            ConfidenceAssessment {
                level: ConfidenceLevel::High,
                reason: format!(
                    "evaluated all {tested} candidate version pairs using indexed resolver data"
                ),
            }
        };

        Ok(Json(CrateCompatibilityMatrixResponse {
            left_crate,
            right_crate,
            check_features,
            pairs_tested: tested,
            compatible_pairs,
            incompatible_pairs,
            confidence: confidence_assessment
                .level
                .as_str()
                .to_string(),
            confidence_assessment,
            next_best_calls: vec![
                "crate_compatibility".to_string(),
                "dependency_resolve".to_string(),
                "crate_graph".to_string(),
            ],
            provenance: "dependency_resolve matrix expansion over indexed versions".to_string(),
        }))
    }
}
