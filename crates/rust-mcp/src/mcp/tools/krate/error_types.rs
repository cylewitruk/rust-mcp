use std::collections::{BTreeSet, HashMap};

use rmcp::Json;
pub use rust_mcp_types::types::krate::{
    CrateErrorTypeEntry, CrateErrorTypesRequest, CrateErrorTypesResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::FromRow;

use crate::mcp::models::{ConfidenceAssessment, ConfidenceLevel};
use crate::mcp::server::McpServer;
use crate::mcp::utils::{
    CursorToken, build_crate_freshness_sources, decode_cursor, encode_cursor, error_types_limit,
    normalize_optional, normalize_required, resolve_pagination, sync_page,
};

#[derive(Debug, Serialize, Deserialize)]
struct CrateErrorTypesCursorToken {
    v: u8,
    offset: u32,
    limit: u32,
    crate_name: String,
    version: Option<String>,
    type_name: Option<String>,
}

impl CursorToken for CrateErrorTypesCursorToken {
    fn version(&self) -> u8 {
        self.v
    }
    fn limit(&self) -> u32 {
        self.limit
    }
    fn offset(&self) -> u32 {
        self.offset
    }
}

#[derive(Debug, Clone, FromRow)]
pub(crate) struct ErrorTypeTypeRow {
    pub(crate) type_name: String,
    pub(crate) kind: String,
    pub(crate) fields: Value,
    pub(crate) variants: Value,
    pub(crate) source_path: String,
    pub(crate) start_line: i32,
}

#[derive(Debug, Clone, FromRow)]
pub(crate) struct ErrorTypeImplRow {
    pub(crate) type_name: String,
    pub(crate) trait_name: Option<String>,
    pub(crate) trait_name_display: Option<String>,
    pub(crate) source_path: String,
}

#[derive(Debug, Clone, FromRow)]
pub(crate) struct ErrorTypeReturnRow {
    pub(crate) name: String,
    pub(crate) signature: Option<String>,
}

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
    pub(crate) async fn handle_crate_error_types(
        &self,
        request: CrateErrorTypesRequest,
    ) -> Result<Json<CrateErrorTypesResponse>, String> {
        let crate_name = normalize_required(request.crate_name, "crate_name")?;
        let requested_version = normalize_optional(request.version);
        let type_name_filter = normalize_optional(request.type_name);
        let cursor = normalize_optional(request.cursor);
        let page = sync_page(request.page);
        let requested_limit = error_types_limit(request.limit);

        let decoded = cursor
            .as_deref()
            .map(decode_cursor::<CrateErrorTypesCursorToken>)
            .transpose()?;

        if let Some(ref token) = decoded
            && (token.crate_name != crate_name
                || token.version != requested_version
                || token.type_name != type_name_filter)
        {
            return Err("cursor does not match current crate.error_types filters".to_string());
        }

        let pag =
            resolve_pagination(decoded.as_ref(), request.limit.is_some(), requested_limit, page)?;

        let ctx = self
            .fetch_crate_context(&crate_name)
            .await?;
        let resolution = self
            .resolve_version_or_latest(&ctx, requested_version.as_deref())
            .await?;

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
        .bind(resolution.selected_version.id)
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
        .bind(resolution.selected_version.id)
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
        for (index, candidate) in candidate_names
            .into_iter()
            .enumerate()
        {
            if index < pag.offset as usize {
                continue;
            }
            if error_types.len() > pag.limit as usize {
                break;
            }

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
            .bind(resolution.selected_version.id)
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
                .bind(resolution.selected_version.id)
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

        let has_more = error_types.len() > pag.limit as usize;
        if has_more {
            error_types.truncate(pag.limit as usize);
        }
        let next_cursor = if has_more {
            Some(encode_cursor(&CrateErrorTypesCursorToken {
                v: 1,
                offset: pag
                    .offset
                    .saturating_add(pag.limit),
                limit: pag.limit,
                crate_name: crate_name.clone(),
                version: requested_version.clone(),
                type_name: type_name_filter.clone(),
            })?)
        } else {
            None
        };

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

        let freshness_check_result = ctx
            .freshness_outcome
            .freshness_check_result
            .clone();

        Ok(Json(CrateErrorTypesResponse {
            crate_name: ctx.crate_row.name,
            selected_version: resolution
                .selected_version
                .version,
            latest_version: ctx.latest_version.version,
            type_name: type_name_filter,
            cursor,
            next_cursor,
            page: pag.effective_page,
            limit: pag.limit,
            has_more,
            truncated: has_more,
            count: error_types.len(),
            error_types,
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
