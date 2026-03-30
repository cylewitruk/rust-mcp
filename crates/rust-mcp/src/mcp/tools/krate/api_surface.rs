use std::collections::BTreeSet;

use rmcp::Json;
pub use rust_mcp_types::types::krate::{CrateApiRequest, CrateApiResponse, CrateApiSymbol};
use serde::{Deserialize, Serialize};

use crate::db::tools;
use crate::mcp::models::{ConfidenceAssessment, ConfidenceLevel};
use crate::mcp::progress::ToolCallContext;
use crate::mcp::server::McpServer;
use crate::mcp::utils::{
    CursorToken, build_crate_freshness_sources, crate_api_limit, decode_cursor, doc_summary,
    encode_cursor, normalize_optional, normalize_required, path_glob_to_like, resolve_pagination,
    sync_page,
};

#[derive(Debug, Serialize, Deserialize)]
struct CrateApiCursorToken {
    v: u8,
    offset: u32,
    limit: u32,
    crate_name: String,
    version: Option<String>,
    path_glob: Option<String>,
    kinds: Vec<String>,
}

impl CursorToken for CrateApiCursorToken {
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

fn normalize_kind_filters(input: Option<Vec<String>>) -> Vec<String> {
    let mut out = input
        .unwrap_or_default()
        .into_iter()
        .map(|value| {
            let normalized = value
                .trim()
                .to_ascii_lowercase();
            // "method" is an alias for "function" (rustdoc doesn't distinguish them).
            if normalized == "method" { "function".to_string() } else { normalized }
        })
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    out.sort();
    out.dedup();
    out
}

fn allowed_kind_filters(values: Vec<String>) -> Vec<String> {
    let allowed =
        ["function", "struct", "enum", "trait", "type_alias", "const", "static", "module"]
            .into_iter()
            .map(ToString::to_string)
            .collect::<BTreeSet<_>>();

    values
        .into_iter()
        .filter(|value| allowed.contains(value))
        .collect()
}

impl McpServer {
    /// Handles the `crate_api` tool call.
    pub async fn handle_crate_api(
        &self,
        request: CrateApiRequest,
        tcx: ToolCallContext,
    ) -> Result<Json<CrateApiResponse>, String> {
        let crate_name = normalize_required(request.crate_name, "crate_name")?;
        let requested_version = normalize_optional(request.version);
        let path_glob = normalize_optional(request.path_glob);
        let kinds = allowed_kind_filters(normalize_kind_filters(request.kinds));
        let include_docs = request
            .include_docs
            .unwrap_or(false);
        let cursor = normalize_optional(request.cursor);
        let page = sync_page(request.page);
        let requested_limit = crate_api_limit(request.limit);

        let decoded = cursor
            .as_deref()
            .map(decode_cursor::<CrateApiCursorToken>)
            .transpose()?;

        if let Some(ref token) = decoded
            && (token.crate_name != crate_name
                || token.version != requested_version
                || token.path_glob != path_glob
                || token.kinds != kinds)
        {
            return Err("cursor does not match current crate_api filters".to_string());
        }

        let pag =
            resolve_pagination(decoded.as_ref(), request.limit.is_some(), requested_limit, page)?;

        let ctx = self
            .fetch_crate_context(&crate_name, &tcx)
            .await?;
        let resolution = self
            .resolve_version_or_latest(&ctx, requested_version.as_deref(), &tcx)
            .await?;

        self.ensure_rustdoc_indexed(&crate_name, resolution.selected_version.id, &tcx)
            .await?;

        let has_rustdoc_symbols = tools::count_rustdoc_public_symbols_for_version(
            &self.state.db,
            resolution.selected_version.id,
        )
        .await
        .map_err(|e| format!("crate_api rustdoc source check failed: {e}"))?
            > 0;
        let preferred_source = if has_rustdoc_symbols { Some("rustdoc_json") } else { None };

        let path_like = path_glob
            .as_deref()
            .map(path_glob_to_like);
        let kind_filter = if kinds.is_empty() { None } else { Some(kinds.as_slice()) };
        let mut rows = tools::list_public_api_symbols_for_version(
            &self.state.db,
            &tools::ApiSurfaceListParams {
                crate_version_id: resolution.selected_version.id,
                kinds: kind_filter,
                preferred_source,
                path_like: path_like.as_deref(),
                limit: i64::from(pag.limit.saturating_add(1)),
                offset: i64::from(pag.offset),
            },
        )
        .await
        .map_err(|e| format!("crate_api query failed: {e}"))?;

        let has_more = rows.len() > pag.limit as usize;
        if has_more {
            rows.truncate(pag.limit as usize);
        }
        let next_cursor = if has_more {
            Some(encode_cursor(&CrateApiCursorToken {
                v: 1,
                offset: pag
                    .offset
                    .saturating_add(pag.limit),
                limit: pag.limit,
                crate_name: crate_name.clone(),
                version: requested_version.clone(),
                path_glob: path_glob.clone(),
                kinds: kinds.clone(),
            })?)
        } else {
            None
        };

        let symbols = rows
            .into_iter()
            .map(|row| CrateApiSymbol {
                name: row.name,
                kind: row.kind,
                signature: row.signature,
                visibility: row.visibility,
                docs: if include_docs {
                    row.docs
                        .as_deref()
                        .map(|d| doc_summary(d).to_string())
                } else {
                    None
                },
                source_path: row.source_path,
                start_line: row.start_line,
                end_line: row.end_line,
                index_source: row.index_source,
            })
            .collect::<Vec<_>>();

        let confidence_assessment = if symbols.is_empty() {
            ConfidenceAssessment {
                level: ConfidenceLevel::Low,
                reason: "no public symbols matched the selected version/filter constraints"
                    .to_string(),
            }
        } else if symbols
            .iter()
            .any(|symbol| symbol.signature.is_none())
        {
            ConfidenceAssessment {
                level: ConfidenceLevel::Medium,
                reason: "some public symbols are missing extracted signatures".to_string(),
            }
        } else {
            ConfidenceAssessment {
                level: ConfidenceLevel::High,
                reason: if has_rustdoc_symbols {
                    "public symbol surface resolved from rustdoc_json indexed symbols".to_string()
                } else {
                    "public symbol surface resolved from indexed symbols".to_string()
                },
            }
        };

        let freshness_check_result = ctx
            .freshness_outcome
            .freshness_check_result
            .clone();

        let suggested_next_tools = if symbols.is_empty() {
            vec!["index_crates".to_string(), "index_refresh".to_string(), "crate_intel".to_string()]
        } else {
            vec![
                "crate_type_info".to_string(),
                "crate_trait_impls".to_string(),
                "crate_api_diff".to_string(),
            ]
        };

        Ok(Json(CrateApiResponse {
            crate_name: ctx.crate_row.name,
            selected_version: resolution
                .selected_version
                .version,
            latest_version: ctx.latest_version.version,
            path_glob,
            kinds,
            cursor,
            next_cursor,
            page: pag.effective_page,
            limit: pag.limit,
            has_more,
            truncated: has_more,
            count: symbols.len(),
            symbols,
            freshness_check_performed: ctx
                .freshness_outcome
                .freshness_check_performed,
            freshness_check_result: freshness_check_result.clone(),
            refresh_enqueued: resolution.refresh_enqueued,
            refresh_job_id: resolution.refresh_job_id,
            freshness: build_crate_freshness_sources(
                ctx.crate_row
                    .updated_at
                    .clone(),
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
    use super::{allowed_kind_filters, normalize_kind_filters};

    #[test]
    fn normalize_kind_filters_dedupes_and_lowers() {
        let values = normalize_kind_filters(Some(vec![
            " Function ".to_string(),
            "trait".to_string(),
            "function".to_string(),
        ]));
        assert_eq!(values, vec!["function".to_string(), "trait".to_string()]);
    }

    #[test]
    fn allowed_kind_filters_removes_unknown_values() {
        let values = allowed_kind_filters(vec![
            "function".to_string(),
            "macro".to_string(),
            "module".to_string(),
        ]);
        assert_eq!(values, vec!["function".to_string(), "module".to_string()]);
    }

    #[test]
    fn normalize_maps_method_to_function() {
        let values = normalize_kind_filters(Some(vec!["method".to_string(), "struct".to_string()]));
        assert_eq!(values, vec!["function".to_string(), "struct".to_string()]);
    }

    #[test]
    fn normalize_deduplicates_method_and_function() {
        let values =
            normalize_kind_filters(Some(vec!["method".to_string(), "function".to_string()]));
        assert_eq!(values, vec!["function".to_string()]);
    }
}
