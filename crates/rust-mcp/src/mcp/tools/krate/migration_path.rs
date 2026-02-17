use rmcp::Json;
pub use rust_mcp_types::types::krate::{
    CrateMigrationAction, CrateMigrationPathRequest, CrateMigrationPathResponse,
};

use crate::mcp::models::{ConfidenceAssessment, ConfidenceLevel};
use crate::mcp::server::McpServer;
use crate::mcp::tools::krate::api_diff::{
    CrateApiDiffChange, CrateApiDiffChangeType, CrateApiDiffRequest,
};
use crate::mcp::utils::{api_diff_limit, normalize_required};

fn normalized_symbol_key(name: &str) -> String {
    name.chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(|ch| ch.to_lowercase())
        .collect::<String>()
}

fn maybe_renamed(from_name: &str, to_name: &str) -> bool {
    let from_key = normalized_symbol_key(from_name);
    let to_key = normalized_symbol_key(to_name);
    if from_key.is_empty() || to_key.is_empty() {
        return false;
    }

    if from_key == to_key {
        return true;
    }

    let from_contains_to = from_key.contains(&to_key);
    let to_contains_from = to_key.contains(&from_key);
    let length_delta = from_key
        .len()
        .abs_diff(to_key.len());
    (from_contains_to || to_contains_from) && length_delta <= 6
}

fn rename_candidate<'a>(
    removed_change: &CrateApiDiffChange,
    changes: &'a [CrateApiDiffChange],
) -> Option<&'a CrateApiDiffChange> {
    changes
        .iter()
        .find(|candidate| {
            candidate.change_type == CrateApiDiffChangeType::Added
                && candidate.kind == removed_change.kind
                && maybe_renamed(&removed_change.name, &candidate.name)
        })
}

fn action_for_change(change: &CrateApiDiffChange) -> (String, String) {
    match change.change_type {
        CrateApiDiffChangeType::Removed => (
            "replace removed API usage".to_string(),
            "symbol was removed between selected versions".to_string(),
        ),
        CrateApiDiffChangeType::SignatureChanged => (
            "update call or type signature".to_string(),
            "symbol signature changed and callsites likely need edits".to_string(),
        ),
        CrateApiDiffChangeType::VisibilityChanged => (
            "adjust import path or usage scope".to_string(),
            "symbol visibility changed and may no longer be publicly reachable".to_string(),
        ),
        CrateApiDiffChangeType::Added => (
            "consider adopting new API".to_string(),
            "symbol was added and may provide replacement or improved usage".to_string(),
        ),
    }
}

impl McpServer {
    pub(crate) async fn handle_crate_migration_path(
        &self,
        request: CrateMigrationPathRequest,
    ) -> Result<Json<CrateMigrationPathResponse>, String> {
        let crate_name = normalize_required(request.crate_name, "crate_name")?;
        let from_version = normalize_required(request.from_version, "from_version")?;
        let to_version = normalize_required(request.to_version, "to_version")?;
        let limit = api_diff_limit(request.limit);

        let diff_response = self
            .handle_crate_api_diff(CrateApiDiffRequest {
                crate_name: crate_name.clone(),
                from_version: from_version.clone(),
                to_version: to_version.clone(),
                limit: Some(limit),
            })
            .await?
            .0;

        let all_changes = diff_response.changes.clone();
        let migration_actions = all_changes
            .iter()
            .filter(|change| {
                change.breaking_change || change.change_type == CrateApiDiffChangeType::Removed
            })
            .map(|change| {
                let (action, mut rationale) = action_for_change(change);
                if change.change_type == CrateApiDiffChangeType::Removed
                    && let Some(candidate) = rename_candidate(change, &all_changes)
                {
                    rationale = format!(
                        "symbol was removed between selected versions; possible replacement is \
                         '{}'",
                        candidate.name
                    );
                }

                CrateMigrationAction {
                    action,
                    rationale,
                    affected_symbol: change.name.clone(),
                    kind: change.kind.clone(),
                    from_signature: change.from_signature.clone(),
                    to_signature: change.to_signature.clone(),
                }
            })
            .collect::<Vec<_>>();

        let confidence_assessment = if diff_response.truncated {
            ConfidenceAssessment {
                level: ConfidenceLevel::Medium,
                reason: "migration actions are partial because api diff results were truncated"
                    .to_string(),
            }
        } else if migration_actions.is_empty() {
            ConfidenceAssessment {
                level: ConfidenceLevel::High,
                reason: "no breaking API differences were detected in indexed symbols".to_string(),
            }
        } else {
            ConfidenceAssessment {
                level: ConfidenceLevel::High,
                reason: "migration actions are derived from indexed API diff breaking changes"
                    .to_string(),
            }
        };

        Ok(Json(CrateMigrationPathResponse {
            crate_name,
            from_version,
            to_version,
            breaking_changes_detected: diff_response.breaking_changes_detected,
            added_count: diff_response.added_count,
            removed_count: diff_response.removed_count,
            changed_count: diff_response.changed_count,
            migration_actions,
            confidence: confidence_assessment
                .level
                .as_str()
                .to_string(),
            confidence_assessment,
            next_best_calls: vec![
                "crate.api_diff".to_string(),
                "source.search".to_string(),
                "source.read".to_string(),
            ],
            provenance: "crate.api_diff wrapper over indexed symbols".to_string(),
        }))
    }
}
