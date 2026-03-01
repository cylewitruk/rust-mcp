use std::collections::BTreeMap;

use rmcp::Json;
pub use rust_mcp_types::types::krate::{
    CrateApiDiffChange, CrateApiDiffChangeType, CrateApiDiffRequest, CrateApiDiffResponse,
};

use crate::db::models::ApiDiffSymbolRow;
use crate::db::tools;
use crate::mcp::models::{ConfidenceAssessment, ConfidenceLevel};
use crate::mcp::server::McpServer;
use crate::mcp::utils::{api_diff_limit, build_crate_freshness_sources, normalize_required};

#[derive(Debug, Clone, PartialEq, Eq)]
struct SymbolFingerprint {
    signature: Option<String>,
    visibility: Option<String>,
}

#[derive(Debug)]
struct ApiDiffSummary {
    added_count: usize,
    removed_count: usize,
    changed_count: usize,
    breaking_changes_detected: bool,
    changes: Vec<CrateApiDiffChange>,
    confidence_assessment: ConfidenceAssessment,
}

fn normalize_signature(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn normalize_visibility(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_ascii_lowercase())
        .filter(|v| !v.is_empty())
}

fn row_key(row: &ApiDiffSymbolRow) -> (String, String) {
    (row.name.clone(), row.kind.clone())
}

fn symbol_row_priority(row: &ApiDiffSymbolRow) -> u8 {
    let mut score = 0u8;
    if row.index_source == "rustdoc_json" {
        score = score.saturating_add(4);
    }
    if normalize_signature(row.signature.clone()).is_some() {
        score = score.saturating_add(2);
    }
    if normalize_visibility(row.visibility.clone()).is_some() {
        score = score.saturating_add(1);
    }
    score
}

fn prioritize_symbol_rows(rows: Vec<ApiDiffSymbolRow>) -> Vec<ApiDiffSymbolRow> {
    let mut best_by_key = BTreeMap::<(String, String), (u8, ApiDiffSymbolRow)>::new();

    for row in rows {
        let key = row_key(&row);
        let priority = symbol_row_priority(&row);
        match best_by_key.get(&key) {
            Some((existing_priority, _)) if *existing_priority >= priority => {}
            _ => {
                best_by_key.insert(key, (priority, row));
            }
        }
    }

    best_by_key
        .into_values()
        .map(|(_, row)| row)
        .collect()
}

fn map_symbols(rows: Vec<ApiDiffSymbolRow>) -> BTreeMap<(String, String), SymbolFingerprint> {
    let mut out = BTreeMap::<(String, String), SymbolFingerprint>::new();
    for row in prioritize_symbol_rows(rows) {
        out.entry(row_key(&row))
            .or_insert(SymbolFingerprint {
                signature: normalize_signature(row.signature),
                visibility: normalize_visibility(row.visibility),
            });
    }
    out
}

fn change_sort_rank(change_type: CrateApiDiffChangeType) -> u8 {
    match change_type {
        CrateApiDiffChangeType::Removed => 0,
        CrateApiDiffChangeType::VisibilityChanged => 1,
        CrateApiDiffChangeType::SignatureChanged => 2,
        CrateApiDiffChangeType::Added => 3,
    }
}

fn build_diff_summary(
    from_rows: Vec<ApiDiffSymbolRow>,
    to_rows: Vec<ApiDiffSymbolRow>,
) -> ApiDiffSummary {
    let from_map = map_symbols(from_rows);
    let to_map = map_symbols(to_rows);

    let mut changes = Vec::<CrateApiDiffChange>::new();
    let mut added_count = 0usize;
    let mut removed_count = 0usize;
    let mut changed_count = 0usize;
    let mut breaking_changes_detected = false;
    let mut changed_with_missing_signatures = 0usize;

    for ((name, kind), from_symbol) in &from_map {
        match to_map.get(&(name.clone(), kind.clone())) {
            None => {
                removed_count += 1;
                breaking_changes_detected = true;
                changes.push(CrateApiDiffChange {
                    name: name.clone(),
                    kind: kind.clone(),
                    change_type: CrateApiDiffChangeType::Removed,
                    from_signature: from_symbol.signature.clone(),
                    to_signature: None,
                    from_visibility: from_symbol.visibility.clone(),
                    to_visibility: None,
                    breaking_change: true,
                });
            }
            Some(to_symbol) => {
                let visibility_changed = from_symbol.visibility != to_symbol.visibility;
                let signature_changed = from_symbol.signature != to_symbol.signature;
                if !(visibility_changed || signature_changed) {
                    continue;
                }

                changed_count += 1;
                let downgraded_visibility = matches!(
                    (
                        from_symbol
                            .visibility
                            .as_deref(),
                        to_symbol
                            .visibility
                            .as_deref()
                    ),
                    (Some("public"), Some("private"))
                );
                let missing_signature_context = from_symbol
                    .signature
                    .is_none()
                    || to_symbol.signature.is_none();

                if missing_signature_context {
                    changed_with_missing_signatures += 1;
                }

                let breaking_change = downgraded_visibility || signature_changed;
                if breaking_change {
                    breaking_changes_detected = true;
                }

                changes.push(CrateApiDiffChange {
                    name: name.clone(),
                    kind: kind.clone(),
                    change_type: if visibility_changed {
                        CrateApiDiffChangeType::VisibilityChanged
                    } else {
                        CrateApiDiffChangeType::SignatureChanged
                    },
                    from_signature: from_symbol.signature.clone(),
                    to_signature: to_symbol.signature.clone(),
                    from_visibility: from_symbol.visibility.clone(),
                    to_visibility: to_symbol.visibility.clone(),
                    breaking_change,
                });
            }
        }
    }

    for ((name, kind), to_symbol) in &to_map {
        if from_map.contains_key(&(name.clone(), kind.clone())) {
            continue;
        }

        added_count += 1;
        changes.push(CrateApiDiffChange {
            name: name.clone(),
            kind: kind.clone(),
            change_type: CrateApiDiffChangeType::Added,
            from_signature: None,
            to_signature: to_symbol.signature.clone(),
            from_visibility: None,
            to_visibility: to_symbol.visibility.clone(),
            breaking_change: false,
        });
    }

    changes.sort_by(|left, right| {
        let left_rank = change_sort_rank(left.change_type);
        let right_rank = change_sort_rank(right.change_type);
        left_rank
            .cmp(&right_rank)
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.kind.cmp(&right.kind))
    });

    let confidence_assessment = if from_map.is_empty() || to_map.is_empty() {
        ConfidenceAssessment {
            level: ConfidenceLevel::Low,
            reason: "one or both versions have no indexed public symbols".to_string(),
        }
    } else if changed_with_missing_signatures > 0 {
        ConfidenceAssessment {
            level: ConfidenceLevel::Medium,
            reason: format!(
                "{changed_with_missing_signatures} changed symbols lack complete signatures"
            ),
        }
    } else {
        ConfidenceAssessment {
            level: ConfidenceLevel::High,
            reason: "public symbol snapshots available for both versions".to_string(),
        }
    };

    ApiDiffSummary {
        added_count,
        removed_count,
        changed_count,
        breaking_changes_detected,
        changes,
        confidence_assessment,
    }
}

impl McpServer {
    /// Handles the `crate.api_diff` tool call.
    pub async fn handle_crate_api_diff(
        &self,
        request: CrateApiDiffRequest,
    ) -> Result<Json<CrateApiDiffResponse>, String> {
        let crate_name = normalize_required(request.crate_name, "crate_name")?;
        let from_version = normalize_required(request.from_version, "from_version")?;
        let to_version = normalize_required(request.to_version, "to_version")?;
        let limit = api_diff_limit(request.limit) as usize;

        let ctx = self
            .fetch_crate_context(&crate_name)
            .await?;
        let freshness_check_result = ctx
            .freshness_outcome
            .freshness_check_result
            .clone();

        let from_version_row =
            tools::fetch_crate_version_by_name(&self.state.db, ctx.crate_row.id, &from_version)
                .await
                .map_err(|e| {
                    format!(
                        "source version lookup failed for {}@{}: {e}",
                        ctx.crate_row.name, from_version
                    )
                })?
                .ok_or_else(|| {
                    format!(
                        "version '{}' for crate '{}' is not indexed locally",
                        from_version, ctx.crate_row.name
                    )
                })?;

        let to_version_row =
            tools::fetch_crate_version_by_name(&self.state.db, ctx.crate_row.id, &to_version)
                .await
                .map_err(|e| {
                    format!(
                        "target version lookup failed for {}@{}: {e}",
                        ctx.crate_row.name, to_version
                    )
                })?
                .ok_or_else(|| {
                    format!(
                        "version '{}' for crate '{}' is not indexed locally",
                        to_version, ctx.crate_row.name
                    )
                })?;

        self.ensure_rustdoc_indexed(&crate_name, from_version_row.id)
            .await?;
        self.ensure_rustdoc_indexed(&crate_name, to_version_row.id)
            .await?;

        let from_symbols =
            tools::list_api_diff_symbols_for_version(&self.state.db, from_version_row.id)
                .await
                .map_err(|e| {
                    format!(
                        "public symbol lookup failed for {}@{}: {e}",
                        ctx.crate_row.name, from_version_row.version
                    )
                })?;

        let to_symbols =
            tools::list_api_diff_symbols_for_version(&self.state.db, to_version_row.id)
                .await
                .map_err(|e| {
                    format!(
                        "public symbol lookup failed for {}@{}: {e}",
                        ctx.crate_row.name, to_version_row.version
                    )
                })?;

        let mut summary = build_diff_summary(from_symbols, to_symbols);
        let truncated = summary.changes.len() > limit;
        if truncated {
            summary
                .changes
                .truncate(limit);
        }

        Ok(Json(CrateApiDiffResponse {
            crate_name: ctx.crate_row.name,
            from_version: from_version_row.version,
            to_version: to_version_row.version,
            added_count: summary.added_count,
            removed_count: summary.removed_count,
            changed_count: summary.changed_count,
            breaking_changes_detected: summary.breaking_changes_detected,
            changes: summary.changes,
            truncated,
            freshness_check_performed: ctx
                .freshness_outcome
                .freshness_check_performed,
            freshness_check_result: freshness_check_result.clone(),
            refresh_enqueued: ctx
                .freshness_outcome
                .refresh_enqueued,
            refresh_job_id: ctx
                .freshness_outcome
                .refresh_job_id,
            freshness: build_crate_freshness_sources(
                ctx.crate_row.updated_at,
                &freshness_check_result,
            ),
            confidence: summary
                .confidence_assessment
                .level
                .as_str()
                .to_string(),
            confidence_assessment: summary.confidence_assessment,
            next_best_calls: vec![
                "symbol.search".to_string(),
                "source.read".to_string(),
                "crate.features".to_string(),
            ],
            provenance: "local_postgres_index".to_string(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ApiDiffSymbolRow, CrateApiDiffChangeType, build_diff_summary, prioritize_symbol_rows,
    };

    fn symbol(
        name: &str,
        kind: &str,
        signature: Option<&str>,
        visibility: Option<&str>,
    ) -> ApiDiffSymbolRow {
        ApiDiffSymbolRow {
            name: name.to_string(),
            kind: kind.to_string(),
            signature: signature.map(ToString::to_string),
            visibility: visibility.map(ToString::to_string),
            index_source: "local_cache".to_string(),
        }
    }

    fn symbol_with_source(
        name: &str,
        kind: &str,
        signature: Option<&str>,
        visibility: Option<&str>,
        index_source: &str,
    ) -> ApiDiffSymbolRow {
        ApiDiffSymbolRow {
            name: name.to_string(),
            kind: kind.to_string(),
            signature: signature.map(ToString::to_string),
            visibility: visibility.map(ToString::to_string),
            index_source: index_source.to_string(),
        }
    }

    #[test]
    fn builds_added_removed_and_changed_counts() {
        let from = vec![
            symbol("old_fn", "function", Some("pub fn old_fn()"), Some("public")),
            symbol("changed_fn", "function", Some("pub fn changed_fn()"), Some("public")),
            symbol("gone", "struct", Some("pub struct gone;"), Some("public")),
        ];
        let to = vec![
            symbol("old_fn", "function", Some("pub fn old_fn()"), Some("public")),
            symbol("changed_fn", "function", Some("pub fn changed_fn(arg: u32)"), Some("public")),
            symbol("new_item", "enum", Some("pub enum new_item { A }"), Some("public")),
        ];

        let summary = build_diff_summary(from, to);

        assert_eq!(summary.added_count, 1);
        assert_eq!(summary.removed_count, 1);
        assert_eq!(summary.changed_count, 1);
        assert!(summary.breaking_changes_detected);

        assert!(
            summary
                .changes
                .iter()
                .any(|change| change.change_type == CrateApiDiffChangeType::Removed)
        );
        assert!(
            summary
                .changes
                .iter()
                .any(|change| change.change_type == CrateApiDiffChangeType::Added)
        );
        assert!(
            summary
                .changes
                .iter()
                .any(|change| change.change_type == CrateApiDiffChangeType::SignatureChanged)
        );
    }

    #[test]
    fn visibility_downgrade_is_breaking() {
        let from = vec![symbol("api_type", "struct", Some("pub struct ApiType;"), Some("public"))];
        let to = vec![symbol("api_type", "struct", Some("struct ApiType;"), Some("private"))];

        let summary = build_diff_summary(from, to);

        assert_eq!(summary.changed_count, 1);
        assert!(summary.breaking_changes_detected);
        assert_eq!(summary.changes[0].change_type, CrateApiDiffChangeType::VisibilityChanged);
        assert!(summary.changes[0].breaking_change);
    }

    #[test]
    fn prioritize_symbol_rows_prefers_rustdoc_and_complete_signatures() {
        let prioritized = prioritize_symbol_rows(vec![
            symbol_with_source("foo", "function", None, Some("public"), "local_cache"),
            symbol_with_source(
                "foo",
                "function",
                Some("pub fn foo()"),
                Some("public"),
                "rustdoc_json",
            ),
            symbol_with_source(
                "foo",
                "function",
                Some("pub fn foo_local()"),
                Some("public"),
                "local_cache",
            ),
        ]);

        assert_eq!(prioritized.len(), 1);
        assert_eq!(prioritized[0].index_source, "rustdoc_json");
        assert_eq!(
            prioritized[0]
                .signature
                .as_deref(),
            Some("pub fn foo()")
        );
    }
}
