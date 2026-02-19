use rmcp::Json;
pub use rust_mcp_types::types::krate::{
    CrateUsagePattern, CrateUsagePatternsRequest, CrateUsagePatternsResponse,
};
use serde::{Deserialize, Serialize};

use crate::db::tools;
use crate::mcp::models::{ConfidenceAssessment, ConfidenceLevel};
use crate::mcp::server::McpServer;
use crate::mcp::utils::{
    CursorToken, build_crate_freshness_sources, decode_cursor, encode_cursor, normalize_optional,
    normalize_required, resolve_pagination, sync_page, usage_patterns_limit,
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

fn extract_usage_snippet(content: &str, symbol_name: &str) -> (u32, u32, String) {
    let lines = content
        .lines()
        .collect::<Vec<_>>();
    if lines.is_empty() {
        return (1, 1, String::new());
    }

    let needle = symbol_name.to_ascii_lowercase();
    let line_index = lines
        .iter()
        .position(|line| {
            line.to_ascii_lowercase()
                .contains(&needle)
        })
        .unwrap_or(0);

    let line_number = (line_index + 1) as u32;
    let start = line_index.saturating_sub(1);
    let end = (line_index + 1).min(lines.len() - 1);

    let snippet = lines[start..=end]
        .join("\n")
        .chars()
        .take(300)
        .collect::<String>();

    (line_number, line_number, snippet)
}

impl McpServer {
    pub(crate) async fn handle_crate_usage_patterns(
        &self,
        request: CrateUsagePatternsRequest,
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
            return Err("cursor does not match current crate.usage_patterns filters".to_string());
        }
        let pag =
            resolve_pagination(decoded.as_ref(), request.limit.is_some(), requested_limit, page)?;

        let ctx = self
            .fetch_crate_context(&crate_name)
            .await?;
        let resolution = self
            .resolve_version_or_latest(&ctx, requested_version.as_deref())
            .await?;

        let symbol_filter = format!("%{}%", symbol_name);

        let rows = tools::list_crate_usage_sources(
            &self.state.db,
            ctx.crate_row.id,
            &symbol_filter,
            i64::from(pag.limit.saturating_add(1)),
            i64::from(pag.offset),
        )
        .await
        .map_err(|e| format!("crate.usage_patterns query failed: {e}"))?;

        let mut patterns = rows
            .into_iter()
            .map(|row| {
                let (line_start, line_end, snippet) =
                    extract_usage_snippet(&row.content, &symbol_name);
                CrateUsagePattern {
                    dependent_crate: row.dependent_crate,
                    dependent_version: row.dependent_version,
                    dependent_downloads: row.dependent_downloads,
                    path: row.path,
                    line_start,
                    line_end,
                    snippet,
                }
            })
            .collect::<Vec<_>>();

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

        let confidence_assessment = if patterns.is_empty() {
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
            next_best_calls: vec![
                "source.read".to_string(),
                "crate.type_info".to_string(),
                "crate.api".to_string(),
            ],
            provenance: "local_postgres_index(dependency_edges, crate_versions, source_files)"
                .to_string(),
        }))
    }
}
