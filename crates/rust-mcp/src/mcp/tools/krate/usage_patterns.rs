use rmcp::Json;
pub use rust_mcp_types::types::krate::{
    CrateUsagePattern, CrateUsagePatternsRequest, CrateUsagePatternsResponse,
};
use serde::{Deserialize, Serialize};

use crate::db::tools;
use crate::mcp::models::{ConfidenceAssessment, ConfidenceLevel};
use crate::mcp::progress::ToolCallContext;
use crate::mcp::ripgrep::{self, RipgrepMode};
use crate::mcp::server::McpServer;
use crate::mcp::utils::{
    CursorToken, build_crate_freshness_sources, decode_cursor, encode_cursor, normalize_optional,
    normalize_required, resolve_pagination, resolve_source_dir, sync_page, usage_patterns_limit,
};

#[derive(Debug, Serialize, Deserialize)]
struct CrateUsagePatternsCursorToken {
    v: u8,
    offset: u32,
    limit: u32,
    crate_name: String,
    symbol_name: String,
    version: Option<String>,
}

impl CursorToken for CrateUsagePatternsCursorToken {
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

impl McpServer {
    /// Handles the `crate_usage_patterns` tool call.
    pub async fn handle_crate_usage_patterns(
        &self,
        request: CrateUsagePatternsRequest,
        tcx: ToolCallContext,
    ) -> Result<Json<CrateUsagePatternsResponse>, String> {
        let crate_name = normalize_required(request.crate_name, "crate_name")?;
        let symbol_name = normalize_required(request.symbol_name, "symbol_name")?;
        let requested_version = normalize_optional(request.version);
        let cursor = normalize_optional(request.cursor);
        let page = sync_page(request.page);
        let requested_limit = usage_patterns_limit(request.limit);

        let decoded = cursor
            .as_deref()
            .map(decode_cursor::<CrateUsagePatternsCursorToken>)
            .transpose()?;
        if let Some(ref token) = decoded
            && (token.crate_name != crate_name
                || token.symbol_name != symbol_name
                || token.version != requested_version)
        {
            return Err("cursor does not match current crate_usage_patterns filters".to_string());
        }
        let pag =
            resolve_pagination(decoded.as_ref(), request.limit.is_some(), requested_limit, page)?;

        let ctx = self
            .fetch_crate_context(&crate_name, &tcx)
            .await?;
        let resolution = self
            .resolve_version_or_latest(&ctx, requested_version.as_deref(), &tcx)
            .await?;

        let dependents =
            tools::list_dependent_crate_versions(&self.state.db, ctx.crate_row.id, 200, 0)
                .await
                .map_err(|e| format!("crate_usage_patterns dependency query failed: {e}"))?;

        let target_count = (pag.offset as usize) + (pag.limit as usize) + 1;
        let mut all_patterns = Vec::<CrateUsagePattern>::new();
        let mut scanned_dependents: usize = 0;

        for dep in &dependents {
            if all_patterns.len() >= target_count {
                break;
            }
            let Some(version_dir) = resolve_source_dir(
                &self
                    .state
                    .config
                    .cargo_registry_dir,
                Some(
                    &self
                        .state
                        .config
                        .crate_source_cache_dir,
                ),
                &dep.dependent_crate,
                &dep.dependent_version,
            ) else {
                continue;
            };
            scanned_dependents += 1;

            let matches = ripgrep::search(
                &version_dir,
                &symbol_name,
                RipgrepMode::FixedString,
                Some("*.rs"),
                20,
            )
            .unwrap_or_default();

            for m in matches {
                if all_patterns.len() >= target_count {
                    break;
                }
                all_patterns.push(CrateUsagePattern {
                    dependent_crate: dep.dependent_crate.clone(),
                    dependent_version: dep.dependent_version.clone(),
                    dependent_downloads: dep.dependent_downloads,
                    path: m.path,
                    line_start: m.line_number,
                    line_end: m.line_number,
                    snippet: m
                        .line_content
                        .trim()
                        .chars()
                        .take(300)
                        .collect(),
                });
            }
        }

        let mut patterns: Vec<_> = all_patterns
            .into_iter()
            .skip(pag.offset as usize)
            .collect();
        let has_more = patterns.len() > pag.limit as usize;
        if has_more {
            patterns.truncate(pag.limit as usize);
        }
        let next_cursor = if has_more {
            Some(encode_cursor(&CrateUsagePatternsCursorToken {
                v: 1,
                offset: pag
                    .offset
                    .saturating_add(pag.limit),
                limit: pag.limit,
                crate_name: crate_name.clone(),
                symbol_name: symbol_name.clone(),
                version: requested_version.clone(),
            })?)
        } else {
            None
        };

        let confidence_assessment = if patterns.is_empty() && scanned_dependents == 0 {
            ConfidenceAssessment {
                level: ConfidenceLevel::Low,
                reason: "no dependent crate source files are locally cached; try crate_type_info \
                         or symbol_search for rustdoc-based intelligence"
                    .to_string(),
            }
        } else if patterns.is_empty() {
            ConfidenceAssessment {
                level: ConfidenceLevel::Low,
                reason: "no dependent source snippets matched the requested symbol in local index"
                    .to_string(),
            }
        } else {
            ConfidenceAssessment {
                level: ConfidenceLevel::High,
                reason: "usage snippets were resolved from indexed dependent crate source files"
                    .to_string(),
            }
        };

        let suggested_next_tools = if patterns.is_empty() {
            vec![
                "index_crates".to_string(),
                "crate_type_info".to_string(),
                "symbol_search".to_string(),
                "crate_api".to_string(),
            ]
        } else {
            vec!["source_read".to_string(), "crate_type_info".to_string(), "crate_api".to_string()]
        };

        let freshness_check_result = ctx
            .freshness_outcome
            .freshness_check_result
            .clone();

        Ok(Json(CrateUsagePatternsResponse {
            crate_name: ctx.crate_row.name,
            selected_version: resolution
                .selected_version
                .version,
            latest_version: ctx.latest_version.version,
            symbol_name,
            cursor,
            next_cursor,
            page: pag.effective_page,
            limit: pag.limit,
            has_more,
            truncated: has_more,
            count: patterns.len(),
            scanned_dependents,
            patterns,
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
            provenance: "local_postgres_index(dependency_edges) + cargo_registry(ripgrep)"
                .to_string(),
        }))
    }
}
