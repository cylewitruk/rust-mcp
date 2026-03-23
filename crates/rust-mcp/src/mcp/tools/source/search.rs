use std::collections::HashSet;

use rmcp::Json;
pub use rust_mcp_types::types::source::{
    SourceSearchHit, SourceSearchMode, SourceSearchRequest, SourceSearchResponse,
};
use serde::{Deserialize, Serialize};

use crate::db::tools;
use crate::mcp::models::{
    ConfidenceAssessment, ConfidenceLevel, SourceReadRequest, SourceReadResponse,
};
use crate::mcp::progress::ToolCallContext;
use crate::mcp::ripgrep::{self, RipgrepMode};
use crate::mcp::server::McpServer;
use crate::mcp::utils::{
    CursorToken, decode_cursor, encode_cursor, normalize_optional, normalize_required,
    read_source_file_from_disk_or_cache, resolve_pagination, resolve_source_dir,
    source_read_end_line, source_search_limit, sync_page,
};

#[derive(Debug, Serialize)]
struct SourceSearchCacheKey<'a> {
    query: &'a str,
    crate_name: Option<&'a str>,
    version: Option<&'a str>,
    path_glob: Option<&'a str>,
    mode: SourceSearchMode,
    cursor: Option<&'a str>,
    page: u32,
    limit: u32,
}

#[derive(Debug, Serialize, Deserialize)]
struct SourceCursorToken {
    v: u8,
    offset: u32,
    limit: u32,
    query: String,
    crate_name: Option<String>,
    version: Option<String>,
    path_glob: Option<String>,
    mode: SourceSearchMode,
}

impl CursorToken for SourceCursorToken {
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
    /// Handles the `source_search` tool call.
    pub async fn handle_source_search(
        &self,
        request: SourceSearchRequest,
        tcx: ToolCallContext,
    ) -> Result<Json<SourceSearchResponse>, String> {
        let query = normalize_required(request.query, "query")?;
        let crate_name = normalize_optional(request.crate_name);
        let version = normalize_optional(request.version);
        let path_glob = normalize_optional(request.path_glob);
        let mode = request
            .mode
            .unwrap_or(SourceSearchMode::Text);
        let cursor = normalize_optional(request.cursor);
        let page = sync_page(request.page);
        let requested_limit = source_search_limit(request.limit);

        let cache_key = serde_json::to_string(&SourceSearchCacheKey {
            query: &query,
            crate_name: crate_name.as_deref(),
            version: version.as_deref(),
            path_glob: path_glob.as_deref(),
            mode,
            cursor: cursor.as_deref(),
            page,
            limit: requested_limit,
        })
        .map_err(|e| format!("failed to build source_search cache key: {e}"))?;
        if let Some(cached) = self
            .query_cache_get("source_search", &cache_key)
            .await?
        {
            let cached_response = serde_json::from_value::<SourceSearchResponse>(cached)
                .map_err(|e| format!("failed to decode source_search cache entry: {e}"))?;
            return Ok(Json(cached_response));
        }

        let decoded_cursor = cursor
            .as_deref()
            .map(decode_cursor::<SourceCursorToken>)
            .transpose()?;
        if let Some(ref decoded) = decoded_cursor
            && (decoded.query != query
                || decoded.crate_name != crate_name
                || decoded.version != version
                || decoded.path_glob != path_glob
                || decoded.mode != mode)
        {
            return Err("cursor does not match current source_search filters".to_string());
        }
        let pagination = resolve_pagination(
            decoded_cursor.as_ref(),
            request.limit.is_some(),
            requested_limit,
            page,
        )?;
        let (offset, limit, effective_page) =
            (pagination.offset, pagination.limit, pagination.effective_page);

        // On-demand source indexing when searching within a specific crate.
        if let Some(ref cn) = crate_name
            && let Ok(Some(cr)) = tools::fetch_crate_core_by_name(&self.state.db, cn).await
        {
            let ver = match version.as_deref() {
                Some(v) => tools::fetch_crate_version_by_name(&self.state.db, cr.id, v)
                    .await
                    .ok()
                    .flatten(),
                None => tools::fetch_latest_crate_version(&self.state.db, cr.id)
                    .await
                    .ok()
                    .flatten(),
            };
            if let Some(ver) = ver {
                self.ensure_source_indexed(cn, &ver.version, ver.id, &tcx)
                    .await?;
            }
        }

        let rg_mode = match mode {
            SourceSearchMode::Text => RipgrepMode::FixedString,
            SourceSearchMode::Regex => RipgrepMode::Regex,
        };

        let crate_versions = tools::list_searchable_crate_versions(
            &self.state.db,
            crate_name.as_deref(),
            version.as_deref(),
            200,
        )
        .await
        .map_err(|e| format!("source_search crate version query failed: {e}"))?;

        let target_count = (offset as usize) + (limit as usize) + 1;
        let mut file_hits = Vec::<SourceSearchHit>::new();

        for cv in &crate_versions {
            if file_hits.len() >= target_count {
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
                &cv.crate_name,
                &cv.version,
            ) else {
                continue;
            };

            let matches = ripgrep::search(&version_dir, &query, rg_mode, path_glob.as_deref(), 500)
                .map_err(|e| format!("source_search ripgrep failed: {e}"))?;

            let mut seen_paths = HashSet::new();
            for m in matches {
                if file_hits.len() >= target_count {
                    break;
                }
                if !seen_paths.insert(m.path.clone()) {
                    continue;
                }
                file_hits.push(SourceSearchHit {
                    crate_name: cv.crate_name.clone(),
                    version: cv.version.clone(),
                    path: m.path,
                    indexed_at: String::new(),
                    match_line: Some(m.line_number),
                    snippet: m
                        .line_content
                        .trim()
                        .chars()
                        .take(220)
                        .collect(),
                });
            }
        }

        let mut hits: Vec<_> = file_hits
            .into_iter()
            .skip(offset as usize)
            .collect();
        let has_more = hits.len() > limit as usize;
        if has_more {
            hits.truncate(limit as usize);
        }
        let next_cursor = if has_more {
            Some(encode_cursor(&SourceCursorToken {
                v: 1,
                offset: offset.saturating_add(limit),
                limit,
                query: query.clone(),
                crate_name: crate_name.clone(),
                version: version.clone(),
                path_glob: path_glob.clone(),
                mode,
            })?)
        } else {
            None
        };

        let confidence_assessment = if hits.is_empty() {
            ConfidenceAssessment {
                level: ConfidenceLevel::Low,
                reason: "no indexed source content matched the query".to_string(),
            }
        } else {
            ConfidenceAssessment {
                level: ConfidenceLevel::Medium,
                reason: "matches are lexical and may require source_read confirmation".to_string(),
            }
        };

        let suggested_next_tools_for_search = if hits.is_empty() {
            vec!["index_crates".to_string(), "symbol_search".to_string(), "docs_search".to_string()]
        } else {
            vec!["source_read".to_string(), "source_context".to_string()]
        };

        let response = SourceSearchResponse {
            query,
            crate_name,
            version,
            path_glob,
            mode,
            cursor,
            next_cursor,
            page: effective_page,
            limit,
            has_more,
            truncated: has_more,
            count: hits.len(),
            confidence: confidence_assessment
                .level
                .as_str()
                .to_string(),
            confidence_assessment,
            suggested_next_tools: suggested_next_tools_for_search,
            provenance: "local_postgres_index".to_string(),
            hits,
        };

        self.query_cache_put(
            "source_search",
            &cache_key,
            &serde_json::to_value(&response)
                .map_err(|e| format!("failed to encode source_search cache value: {e}"))?,
            300,
        )
        .await?;

        Ok(Json(response))
    }

    /// Handles the `source_read` tool call.
    pub async fn handle_source_read(
        &self,
        request: SourceReadRequest,
        tcx: ToolCallContext,
    ) -> Result<Json<SourceReadResponse>, String> {
        let crate_name = normalize_required(request.crate_name, "crate_name")?;
        let path = normalize_required(request.path, "path")?;

        let row = if let Some(version) = normalize_optional(request.version) {
            let crate_row = tools::fetch_crate_core_by_name(&self.state.db, &crate_name)
                .await
                .map_err(|e| {
                    format!(
                        "source_read crate lookup failed for {crate_name}@{version}:{path}: {e}"
                    )
                })?
                .ok_or_else(|| format!("crate '{crate_name}' is not indexed locally"))?;

            let crate_version =
                tools::fetch_crate_version_by_name(&self.state.db, crate_row.id, &version)
                    .await
                    .map_err(|e| {
                        format!(
                            "source_read version lookup failed for {crate_name}@{version}:{path}: \
                             {e}"
                        )
                    })?
                    .ok_or_else(|| {
                        format!(
                            "version '{version}' for crate '{crate_name}' is not indexed locally"
                        )
                    })?;

            self.ensure_source_indexed(&crate_name, &version, crate_version.id, &tcx)
                .await?;

            tools::fetch_source_read_for_crate_version_path(
                &self.state.db,
                &crate_name,
                crate_version.id,
                &path,
            )
            .await
            .map_err(|e| {
                format!("source_read lookup failed for {crate_name}@{version}:{path}: {e}")
            })?
            .ok_or_else(|| {
                format!(
                    "source file not found for {crate_name}@{version}:{path}. Try crate_api or \
                     symbol_search to explore this crate's API surface."
                )
            })?
        } else {
            // Best-effort on-demand source indexing for the latest version.
            if let Ok(Some(cr)) = tools::fetch_crate_core_by_name(&self.state.db, &crate_name).await
                && let Ok(Some(lv)) = tools::fetch_latest_crate_version(&self.state.db, cr.id).await
            {
                self.ensure_source_indexed(&crate_name, &lv.version, lv.id, &tcx)
                    .await?;
            }

            tools::fetch_source_read_latest_for_crate_path(&self.state.db, &crate_name, &path)
                .await
                .map_err(|e| format!("source_read lookup failed for {crate_name}:{path}: {e}"))?
                .ok_or_else(|| {
                    format!(
                        "source file not found for {crate_name}:{path}. Try crate_api or \
                         symbol_search to explore this crate's API surface."
                    )
                })?
        };

        let content = read_source_file_from_disk_or_cache(
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
            &row.crate_name,
            &row.version,
            &row.path,
        )
        .ok_or_else(|| {
            format!(
                "source content not available on disk for {}@{}:{} (not found in cargo registry). \
                 Try crate_type_info, crate_api, or symbol_search for rustdoc-based API data.",
                row.crate_name, row.version, row.path
            )
        })?;

        let lines = content
            .lines()
            .collect::<Vec<_>>();
        let total_lines = lines.len().max(1) as u32;
        let start_line = request
            .start_line
            .unwrap_or(1)
            .clamp(1, total_lines);
        let end_line = source_read_end_line(request.end_line)
            .max(start_line)
            .min(total_lines);

        let start_idx = (start_line - 1) as usize;
        let end_idx = end_line as usize;
        let selected = lines
            .get(start_idx..end_idx)
            .map(|slice| slice.join("\n"))
            .unwrap_or_default();

        let confidence_assessment = ConfidenceAssessment {
            level: ConfidenceLevel::High,
            reason: "returned exact indexed file slice for selected crate/path".to_string(),
        };

        Ok(Json(SourceReadResponse {
            crate_name: row.crate_name,
            version: row.version,
            path: row.path,
            start_line,
            end_line,
            total_lines,
            content: selected,
            confidence: confidence_assessment
                .level
                .as_str()
                .to_string(),
            confidence_assessment,
            suggested_next_tools: vec![
                "source_context".to_string(),
                "crate_type_info".to_string(),
                "source_search".to_string(),
            ],
            provenance: "local_postgres_index".to_string(),
        }))
    }
}
