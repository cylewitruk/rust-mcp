use std::collections::{BTreeSet, HashMap};

use rmcp::Json;
use serde_json::Value;

use super::models::{
    ConfidenceAssessment, ConfidenceLevel, CrateCoreRow, CrateErrorTypeEntry,
    CrateErrorTypesRequest, CrateErrorTypesResponse, CrateVersionSelectionRow, ErrorTypeImplRow,
    ErrorTypeReturnRow, ErrorTypeTypeRow, ResponseFreshnessSource,
};
use super::server::McpServer;
use super::utils::{error_types_limit, normalize_optional, normalize_required};

fn collect_field_types(value: &Value) -> Vec<String> {
    match value {
        Value::Array(entries) => entries
            .iter()
            .filter_map(|entry| {
                entry
                    .get("type")
                    .and_then(Value::as_str)
            })
            .map(ToString::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

fn collect_variant_names(value: &Value) -> Vec<String> {
    match value {
        Value::Array(entries) => entries
            .iter()
            .filter_map(|entry| {
                entry
                    .get("name")
                    .and_then(Value::as_str)
            })
            .map(ToString::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

fn extract_generic_argument(display: &str) -> Option<String> {
    let start = display.find('<')?;
    let end = display.rfind('>')?;
    if end <= start + 1 {
        return None;
    }

    let inner = display[start + 1..end].trim();
    if inner.is_empty() { None } else { Some(inner.to_string()) }
}

fn extract_display_patterns(content: &str, type_name: &str) -> Vec<String> {
    let mut patterns = BTreeSet::<String>::new();
    let lines = content
        .lines()
        .collect::<Vec<_>>();

    for (index, line) in lines.iter().enumerate() {
        let normalized = line.trim();
        if !(normalized.contains("impl")
            && normalized.contains("Display")
            && normalized.contains(type_name))
        {
            continue;
        }

        let window_end = (index + 24).min(lines.len());
        for candidate in &lines[index..window_end] {
            let candidate = candidate.trim();
            if !candidate.contains("write!(") {
                continue;
            }
            if let Some(start) = candidate.find('"')
                && let Some(end_offset) = candidate[start + 1..].find('"')
            {
                let end = start + 1 + end_offset;
                let extracted = candidate[start + 1..end].trim();
                if !extracted.is_empty() {
                    patterns.insert(extracted.to_string());
                }
            }
        }
    }

    patterns.into_iter().collect()
}

fn matches_error_heuristic(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with("error") || lower.ends_with("err")
}

impl McpServer {
    pub(super) async fn handle_crate_error_types(
        &self,
        request: CrateErrorTypesRequest,
    ) -> Result<Json<CrateErrorTypesResponse>, String> {
        let crate_name = normalize_required(request.crate_name, "crate_name")?;
        let requested_version = normalize_optional(request.version);
        let type_name_filter = normalize_optional(request.type_name);
        let limit = error_types_limit(request.limit);

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

        let type_rows = sqlx::query_as::<_, ErrorTypeTypeRow>(
            "SELECT
                ct.type_name,
                ct.kind,
                ct.fields,
                ct.variants,
                sf.path AS source_path,
                ct.start_line
             FROM crate_types ct
             JOIN source_files sf ON sf.id = ct.source_file_id
             WHERE ct.crate_version_id = $1
             ORDER BY ct.type_name ASC",
        )
        .bind(selected_version.id)
        .fetch_all(&self.state.db)
        .await
        .map_err(|e| format!("crate.error_types type query failed: {e}"))?;

        let impl_rows = sqlx::query_as::<_, ErrorTypeImplRow>(
            "SELECT
                ci.type_name,
                ci.trait_name,
                ci.trait_name_display,
                sf.path AS source_path
             FROM crate_impls ci
             JOIN source_files sf ON sf.id = ci.source_file_id
             WHERE ci.crate_version_id = $1",
        )
        .bind(selected_version.id)
        .fetch_all(&self.state.db)
        .await
        .map_err(|e| format!("crate.error_types impl query failed: {e}"))?;

        let mut candidate_names = BTreeSet::<String>::new();
        for row in &type_rows {
            if matches_error_heuristic(&row.type_name) {
                candidate_names.insert(row.type_name.clone());
            }
        }

        for row in &impl_rows {
            let trait_name = row
                .trait_name
                .as_deref()
                .unwrap_or_default()
                .to_ascii_lowercase();
            let trait_display = row
                .trait_name_display
                .as_deref()
                .unwrap_or_default()
                .to_ascii_lowercase();
            if trait_name == "error" || trait_display.contains("std::error::error") {
                candidate_names.insert(row.type_name.clone());
            }
        }

        if let Some(filter) = type_name_filter.as_deref() {
            candidate_names.retain(|name| name.eq_ignore_ascii_case(filter));
        }

        let type_by_name = type_rows
            .into_iter()
            .map(|row| {
                (
                    row.type_name
                        .to_ascii_lowercase(),
                    row,
                )
            })
            .collect::<HashMap<_, _>>();

        let mut error_types = Vec::<CrateErrorTypeEntry>::new();
        for candidate in candidate_names
            .into_iter()
            .take(limit as usize)
        {
            let Some(type_row) = type_by_name.get(&candidate.to_ascii_lowercase()) else {
                continue;
            };

            let candidate_impls = impl_rows
                .iter()
                .filter(|row| {
                    row.type_name
                        .eq_ignore_ascii_case(&candidate)
                })
                .collect::<Vec<_>>();

            let mut from_conversions = BTreeSet::<String>::new();
            for row in &candidate_impls {
                let trait_name = row
                    .trait_name
                    .as_deref()
                    .unwrap_or_default();
                if !matches!(trait_name, "From" | "TryFrom") {
                    continue;
                }
                if let Some(display) = row
                    .trait_name_display
                    .as_deref()
                    && let Some(source_type) = extract_generic_argument(display)
                {
                    from_conversions.insert(source_type);
                }
            }

            let return_rows = sqlx::query_as::<_, ErrorTypeReturnRow>(
                "SELECT
                    s.name,
                    s.signature
                 FROM symbols s
                 WHERE s.crate_version_id = $1
                   AND s.signature IS NOT NULL
                   AND s.signature ILIKE '%Result<%'
                   AND s.signature ILIKE $2
                 ORDER BY s.name ASC
                 LIMIT 50",
            )
            .bind(selected_version.id)
            .bind(format!("%{}%", candidate))
            .fetch_all(&self.state.db)
            .await
            .map_err(|e| format!("crate.error_types return signature query failed: {e}"))?;

            let returned_by = return_rows
                .into_iter()
                .map(|row| {
                    let signature = row
                        .signature
                        .unwrap_or_default();
                    if signature.is_empty() {
                        row.name
                    } else {
                        format!("{}: {}", row.name, signature)
                    }
                })
                .collect::<Vec<_>>();

            let mut display_patterns = BTreeSet::<String>::new();
            for row in &candidate_impls {
                let trait_name = row
                    .trait_name
                    .as_deref()
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                if trait_name != "display" {
                    continue;
                }

                let content = sqlx::query_scalar::<_, String>(
                    "SELECT content
                     FROM source_files
                     WHERE crate_version_id = $1 AND path = $2
                     LIMIT 1",
                )
                .bind(selected_version.id)
                .bind(&row.source_path)
                .fetch_optional(&self.state.db)
                .await
                .map_err(|e| format!("crate.error_types display source lookup failed: {e}"))?
                .unwrap_or_default();

                for pattern in extract_display_patterns(&content, &candidate) {
                    display_patterns.insert(pattern);
                }
            }

            if display_patterns.is_empty()
                && candidate_impls
                    .iter()
                    .any(|row| {
                        row.trait_name
                            .as_deref()
                            .map(|name| name.eq_ignore_ascii_case("display"))
                            .unwrap_or(false)
                    })
            {
                display_patterns.insert("Display impl present".to_string());
            }

            error_types.push(CrateErrorTypeEntry {
                type_name: candidate,
                kind: type_row.kind.clone(),
                variants: collect_variant_names(&type_row.variants),
                fields: collect_field_types(&type_row.fields),
                display_patterns: display_patterns
                    .into_iter()
                    .collect(),
                from_conversions: from_conversions
                    .into_iter()
                    .collect(),
                returned_by,
                source_path: type_row.source_path.clone(),
                source_line: type_row.start_line,
            });
        }

        let confidence_assessment = if error_types.is_empty() {
            ConfidenceAssessment {
                level: ConfidenceLevel::Low,
                reason: "no error-like types were identified in indexed type metadata".to_string(),
            }
        } else if error_types
            .iter()
            .all(|entry| {
                entry
                    .from_conversions
                    .is_empty()
                    && entry.returned_by.is_empty()
            })
        {
            ConfidenceAssessment {
                level: ConfidenceLevel::Medium,
                reason: "error types were found but conversion/return chains are sparse"
                    .to_string(),
            }
        } else {
            ConfidenceAssessment {
                level: ConfidenceLevel::High,
                reason: "error types, conversions, and return signatures resolved from local index"
                    .to_string(),
            }
        };

        let freshness_check_result = freshness_outcome
            .freshness_check_result
            .clone();

        Ok(Json(CrateErrorTypesResponse {
            crate_name: crate_row.name,
            selected_version: selected_version.version,
            latest_version: latest_version.version,
            type_name: type_name_filter,
            limit,
            count: error_types.len(),
            error_types,
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
                "crate.type_info".to_string(),
                "crate.trait_impls".to_string(),
                "source.search".to_string(),
            ],
            provenance: "local_postgres_index(crate_types, crate_impls, symbols, source_files)"
                .to_string(),
        }))
    }
}
