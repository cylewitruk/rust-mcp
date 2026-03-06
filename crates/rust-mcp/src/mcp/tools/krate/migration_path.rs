use rmcp::Json;
pub use rust_mcp_types::types::krate::{
    CrateMigrationAction, CrateMigrationPathRequest, CrateMigrationPathResponse,
};

use crate::mcp::models::{ConfidenceAssessment, ConfidenceLevel};
use crate::mcp::progress::ToolCallContext;
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

fn split_symbol_tokens(name: &str) -> Vec<String> {
    let mut tokens = Vec::<String>::new();
    let mut current = String::new();
    let mut previous_was_lower = false;

    for ch in name.chars() {
        if !ch.is_ascii_alphanumeric() {
            if !current.is_empty() {
                tokens.push(current.to_ascii_lowercase());
                current.clear();
            }
            previous_was_lower = false;
            continue;
        }

        if ch.is_ascii_uppercase() && previous_was_lower && !current.is_empty() {
            tokens.push(current.to_ascii_lowercase());
            current.clear();
        }

        previous_was_lower = ch.is_ascii_lowercase();
        current.push(ch);
    }

    if !current.is_empty() {
        tokens.push(current.to_ascii_lowercase());
    }

    tokens
}

fn token_overlap_score(left: &str, right: &str) -> i32 {
    let left_tokens = split_symbol_tokens(left);
    let right_tokens = split_symbol_tokens(right);
    if left_tokens.is_empty() || right_tokens.is_empty() {
        return 0;
    }

    let overlap = left_tokens
        .iter()
        .filter(|token| right_tokens.contains(token))
        .count();
    if overlap == 0 {
        return 0;
    }

    let total = left_tokens.len() + right_tokens.len() - overlap;
    ((overlap as f32 / total as f32) * 4.0).round() as i32
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

fn signature_shape(signature: &str) -> Option<(usize, bool)> {
    let open_idx = signature.find('(')?;
    let close_idx = signature[open_idx + 1..].find(')')? + open_idx + 1;
    let args_text = signature[open_idx + 1..close_idx].trim();
    let arg_count = if args_text.is_empty() { 0 } else { args_text.split(',').count() };

    let has_return = signature[close_idx + 1..].contains("->");
    Some((arg_count, has_return))
}

fn normalized_signature_key(signature: &str) -> String {
    let condensed = signature
        .split_whitespace()
        .collect::<String>();

    if let Some(fn_idx) = condensed.find("fn")
        && let Some(open_idx) = condensed[fn_idx + 2..].find('(')
    {
        let open_idx = fn_idx + 2 + open_idx;
        let mut normalized = String::from("fn");
        normalized.push_str(&condensed[open_idx..]);
        return normalized;
    }

    condensed
}

fn signature_compatibility_score(
    removed_signature: Option<&str>,
    added_signature: Option<&str>,
) -> i32 {
    match (removed_signature, added_signature) {
        (Some(left), Some(right)) => {
            if normalized_signature_key(left) == normalized_signature_key(right) {
                return 5;
            }

            match (signature_shape(left), signature_shape(right)) {
                (Some((left_args, left_ret)), Some((right_args, right_ret))) => {
                    let mut score = 0;
                    if left_args == right_args {
                        score += 2;
                    }
                    if left_ret == right_ret {
                        score += 1;
                    }
                    score
                }
                _ => 0,
            }
        }
        _ => 0,
    }
}

fn replacement_candidate<'a>(
    removed_change: &CrateApiDiffChange,
    changes: &'a [CrateApiDiffChange],
) -> Option<&'a CrateApiDiffChange> {
    let mut best_match: Option<(&CrateApiDiffChange, i32)> = None;

    for candidate in changes {
        if candidate.change_type != CrateApiDiffChangeType::Added
            || candidate.kind != removed_change.kind
        {
            continue;
        }

        let mut score = 0;
        if maybe_renamed(&removed_change.name, &candidate.name) {
            score += 5;
        }
        score += token_overlap_score(&removed_change.name, &candidate.name);
        score += signature_compatibility_score(
            removed_change
                .from_signature
                .as_deref(),
            candidate
                .to_signature
                .as_deref(),
        );

        if score < 5 {
            continue;
        }

        match best_match {
            Some((_, best_score)) if best_score >= score => {}
            _ => best_match = Some((candidate, score)),
        }
    }

    best_match.map(|(candidate, _)| candidate)
}

fn action_rationale(change: &CrateApiDiffChange, all_changes: &[CrateApiDiffChange]) -> String {
    let (_, mut rationale) = action_for_change(change);

    match change.change_type {
        CrateApiDiffChangeType::Removed => {
            if let Some(candidate) = replacement_candidate(change, all_changes) {
                rationale = format!(
                    "symbol was removed between selected versions; likely replacement is '{}'",
                    candidate.name
                );
            }
        }
        CrateApiDiffChangeType::SignatureChanged => {
            if let (Some(from_sig), Some(to_sig)) = (
                change
                    .from_signature
                    .as_deref(),
                change.to_signature.as_deref(),
            ) {
                rationale = format!(
                    "symbol signature changed; update callsites from `{from_sig}` to `{to_sig}`"
                );
            }
        }
        CrateApiDiffChangeType::VisibilityChanged => {
            if let (Some(from_vis), Some(to_vis)) = (
                change
                    .from_visibility
                    .as_deref(),
                change
                    .to_visibility
                    .as_deref(),
            ) {
                rationale = format!(
                    "symbol visibility changed from `{from_vis}` to `{to_vis}` and may require \
                     import/API usage adjustments"
                );
            }
        }
        CrateApiDiffChangeType::Added => {}
    }

    rationale
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
    /// Handles the `crate_migration_path` tool call.
    pub async fn handle_crate_migration_path(
        &self,
        request: CrateMigrationPathRequest,
        tcx: ToolCallContext,
    ) -> Result<Json<CrateMigrationPathResponse>, String> {
        let crate_name = normalize_required(request.crate_name, "crate_name")?;
        let from_version = normalize_required(request.from_version, "from_version")?;
        let to_version = normalize_required(request.to_version, "to_version")?;
        let limit = api_diff_limit(request.limit);

        let diff_response = self
            .handle_crate_api_diff(
                CrateApiDiffRequest {
                    crate_name: crate_name.clone(),
                    from_version: from_version.clone(),
                    to_version: to_version.clone(),
                    limit: Some(limit),
                },
                tcx,
            )
            .await?
            .0;

        let all_changes = diff_response.changes.clone();
        let migration_actions = all_changes
            .iter()
            .filter(|change| {
                change.breaking_change || change.change_type == CrateApiDiffChangeType::Removed
            })
            .map(|change| {
                let (action, _) = action_for_change(change);
                let rationale = action_rationale(change, &all_changes);

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
            suggested_next_tools: vec![
                "crate_api_diff".to_string(),
                "source_search".to_string(),
                "source_read".to_string(),
            ],
            provenance: "crate_api_diff wrapper over indexed symbols".to_string(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CrateApiDiffChange, CrateApiDiffChangeType, action_rationale, replacement_candidate,
    };

    fn change(
        name: &str,
        kind: &str,
        change_type: CrateApiDiffChangeType,
        from_signature: Option<&str>,
        to_signature: Option<&str>,
    ) -> CrateApiDiffChange {
        CrateApiDiffChange {
            name: name.to_string(),
            kind: kind.to_string(),
            change_type,
            from_signature: from_signature.map(ToString::to_string),
            to_signature: to_signature.map(ToString::to_string),
            from_visibility: Some("public".to_string()),
            to_visibility: Some("public".to_string()),
            breaking_change: matches!(
                change_type,
                CrateApiDiffChangeType::Removed
                    | CrateApiDiffChangeType::SignatureChanged
                    | CrateApiDiffChangeType::VisibilityChanged
            ),
        }
    }

    #[test]
    fn replacement_candidate_prefers_signature_compatible_added_symbol() {
        let removed = change(
            "legacy_decode",
            "function",
            CrateApiDiffChangeType::Removed,
            Some("pub fn legacy_decode(input: &str) -> Result<u32, Error>"),
            None,
        );
        let added_unrelated = change(
            "new_decoder",
            "function",
            CrateApiDiffChangeType::Added,
            None,
            Some("pub fn new_decoder(bytes: &[u8])"),
        );
        let added_compatible = change(
            "parse_value",
            "function",
            CrateApiDiffChangeType::Added,
            None,
            Some("pub fn parse_value(input: &str) -> Result<u32, Error>"),
        );

        let all = vec![removed.clone(), added_unrelated, added_compatible];
        let candidate = replacement_candidate(&removed, &all).expect("candidate should exist");
        assert_eq!(candidate.name, "parse_value");
    }

    #[test]
    fn replacement_candidate_rejects_kind_mismatch() {
        let removed = change(
            "legacy_decode",
            "function",
            CrateApiDiffChangeType::Removed,
            Some("pub fn legacy_decode(input: &str) -> Result<u32, Error>"),
            None,
        );
        let added_struct = change(
            "parse_value",
            "struct",
            CrateApiDiffChangeType::Added,
            None,
            Some("pub struct ParseValue;"),
        );

        let all = vec![removed.clone(), added_struct];
        assert!(replacement_candidate(&removed, &all).is_none());
    }

    #[test]
    fn action_rationale_includes_signature_guidance_for_signature_changes() {
        let signature_change = change(
            "parse",
            "function",
            CrateApiDiffChangeType::SignatureChanged,
            Some("pub fn parse(s: &str) -> Value"),
            Some("pub fn parse(s: &str, strict: bool) -> Value"),
        );

        let rationale =
            action_rationale(&signature_change, std::slice::from_ref(&signature_change));
        assert!(rationale.contains("from `pub fn parse(s: &str) -> Value`"));
        assert!(rationale.contains("to `pub fn parse(s: &str, strict: bool) -> Value`"));
    }
}
