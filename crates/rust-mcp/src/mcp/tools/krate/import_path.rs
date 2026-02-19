use rmcp::Json;
pub use rust_mcp_types::types::krate::{
    CrateImportPathMatch, CrateImportPathRequest, CrateImportPathResponse,
};
use serde::{Deserialize, Serialize};

use crate::db::models::ImportPathRow;
use crate::db::tools;
use crate::mcp::models::{ConfidenceAssessment, ConfidenceLevel};
use crate::mcp::server::McpServer;
use crate::mcp::utils::{
    CursorToken, apply_pagination_limit, build_crate_freshness_sources, decode_cursor,
    import_path_limit, normalize_optional, normalize_required, resolve_pagination, sync_page,
};

#[derive(Debug, Serialize, Deserialize)]
struct CrateImportPathCursorToken {
    v: u8,
    offset: u32,
    limit: u32,
    crate_name: String,
    symbol_name: String,
    version: Option<String>,
    kind: Option<String>,
}

impl CursorToken for CrateImportPathCursorToken {
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

fn normalized_import_path(crate_name: &str, row: &ImportPathRow) -> String {
    row.canonical_path
        .clone()
        .filter(|path| !path.trim().is_empty())
        .unwrap_or_else(|| format!("{crate_name}::{}", row.symbol_name))
}

impl McpServer {
    pub(crate) async fn handle_crate_import_path(
        &self,
        request: CrateImportPathRequest,
    ) -> Result<Json<CrateImportPathResponse>, String> {
        let crate_name = normalize_required(request.crate_name, "crate_name")?;
        let symbol_name = normalize_required(request.symbol_name, "symbol_name")?;
        let requested_version = normalize_optional(request.version);
        let kind = normalize_optional(request.kind);
        let cursor = normalize_optional(request.cursor);
        let page = sync_page(request.page);
        let requested_limit = import_path_limit(request.limit);

        let decoded = cursor
            .as_deref()
            .map(decode_cursor::<CrateImportPathCursorToken>)
            .transpose()?;

        if let Some(ref token) = decoded
            && (token.crate_name != crate_name
                || !token
                    .symbol_name
                    .eq_ignore_ascii_case(&symbol_name)
                || token.version != requested_version
                || token.kind != kind)
        {
            return Err("cursor does not match current crate.import_path filters".to_string());
        }

        let pag =
            resolve_pagination(decoded.as_ref(), request.limit.is_some(), requested_limit, page)?;

        let ctx = self
            .fetch_crate_context(&crate_name)
            .await?;
        let resolution = self
            .resolve_version_or_latest(&ctx, requested_version.as_deref())
            .await?;

        let symbol_rows = tools::list_import_path_matches(
            &self.state.db,
            resolution.selected_version.id,
            &symbol_name,
            kind.as_deref(),
            i64::from(pag.limit.saturating_add(1)),
            i64::from(pag.offset),
        )
        .await
        .map_err(|e| format!("crate.import_path query failed: {e}"))?;

        let matches = symbol_rows
            .iter()
            .map(|row| CrateImportPathMatch {
                symbol_name: row.symbol_name.clone(),
                kind: row.kind.clone(),
                import_path: normalized_import_path(&ctx.crate_row.name, row),
                definition_path: row.definition_path.clone(),
                source_path: row.source_path.clone(),
                start_line: row.start_line.max(1) as u32,
                end_line: row.end_line.max(1) as u32,
                index_source: row.index_source.clone(),
                exact_name_match: row
                    .symbol_name
                    .eq_ignore_ascii_case(&symbol_name),
            })
            .collect::<Vec<_>>();

        let best_import_path = matches
            .first()
            .map(|entry| entry.import_path.clone());

        let confidence_assessment = if matches.is_empty() {
            ConfidenceAssessment {
                level: ConfidenceLevel::Low,
                reason: "no matching public symbols were found in indexed local cache data"
                    .to_string(),
            }
        } else if matches
            .first()
            .is_some_and(|entry| entry.index_source == "rustdoc_json" && entry.exact_name_match)
        {
            ConfidenceAssessment {
                level: ConfidenceLevel::High,
                reason: "best import path resolved from rustdoc canonical symbol data".to_string(),
            }
        } else {
            ConfidenceAssessment {
                level: ConfidenceLevel::Medium,
                reason: "import path resolved from indexed symbols without rustdoc canonical \
                         priority"
                    .to_string(),
            }
        };

        let crate_name_clone = crate_name.clone();
        let symbol_name_clone = symbol_name.clone();
        let version_clone = requested_version.clone();
        let kind_clone = kind.clone();
        let paginated = apply_pagination_limit(matches, pag.limit, pag.offset, |next_offset| {
            CrateImportPathCursorToken {
                v: 1,
                offset: next_offset,
                limit: pag.limit,
                crate_name: crate_name_clone,
                symbol_name: symbol_name_clone,
                version: version_clone,
                kind: kind_clone,
            }
        })?;

        let freshness_check_result = ctx
            .freshness_outcome
            .freshness_check_result
            .clone();

        Ok(Json(CrateImportPathResponse {
            crate_name: ctx.crate_row.name,
            selected_version: resolution
                .selected_version
                .version,
            latest_version: ctx.latest_version.version,
            symbol_name,
            kind,
            cursor,
            next_cursor: paginated.next_cursor,
            page: pag.effective_page,
            limit: pag.limit,
            has_more: paginated.has_more,
            truncated: paginated.has_more,
            count: paginated.items.len(),
            best_import_path,
            matches: paginated.items,
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
                "crate.re_exports".to_string(),
                "crate.type_info".to_string(),
                "source.search".to_string(),
            ],
            provenance: "local_postgres_index(symbols, source_files)".to_string(),
        }))
    }
}
