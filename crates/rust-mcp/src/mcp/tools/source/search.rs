use rmcp::Json;
pub use rust_mcp_types::types::source::{
    SourceSearchHit, SourceSearchMode, SourceSearchRequest, SourceSearchResponse,
};
use serde::{Deserialize, Serialize};

use crate::db::tools;
use crate::mcp::models::{
    ConfidenceAssessment, ConfidenceLevel, SourceReadRequest, SourceReadResponse,
};
use crate::mcp::server::McpServer;
use crate::mcp::utils::{
    CursorToken, decode_cursor, encode_cursor, normalize_optional, normalize_required,
    path_glob_to_like, resolve_pagination, source_read_end_line, source_search_limit, sync_page,
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

fn extract_text_snippet(content: &str, query: &str) -> (Option<u32>, String) {
    if query.is_empty() {
        let first_line = content
            .lines()
            .next()
            .unwrap_or_default();
        return (
            Some(1),
            first_line
                .chars()
                .take(220)
                .collect(),
        );
    }

    let q = query.to_ascii_lowercase();
    for (index, line) in content.lines().enumerate() {
        if line
            .to_ascii_lowercase()
            .contains(&q)
        {
            return (
                Some((index + 1) as u32),
                line.trim()
                    .chars()
                    .take(220)
                    .collect::<String>(),
            );
        }
    }

    let fallback = content
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .chars()
        .take(220)
        .collect();
    (None, fallback)
}

impl McpServer {
    pub(crate) async fn handle_source_search(
        &self,
        request: SourceSearchRequest,
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
        .map_err(|e| format!("failed to build source.search cache key: {e}"))?;
        if let Some(cached) = self
            .query_cache_get("source.search", &cache_key)
            .await?
        {
            let cached_response = serde_json::from_value::<SourceSearchResponse>(cached)
                .map_err(|e| format!("failed to decode source.search cache entry: {e}"))?;
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
            return Err("cursor does not match current source.search filters".to_string());
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
                self.ensure_source_indexed(cn, ver.id)
                    .await?;
            }
        }

        let path_like = path_glob
            .as_deref()
            .map(path_glob_to_like);
        let rows = tools::search_source_files(
            &self.state.db,
            &tools::SourceFileSearchParams {
                query: &query,
                crate_name: crate_name.as_deref(),
                version: version.as_deref(),
                path_like: path_like.as_deref(),
                mode,
                limit: i64::from(limit.saturating_add(1)),
                offset: i64::from(offset),
            },
        )
        .await
        .map_err(|e| format!("source.search query failed: {e}"))?;

        let mut hits = rows
            .into_iter()
            .map(|row| {
                let (line, snippet) = extract_text_snippet(&row.content, &query);
                SourceSearchHit {
                    crate_name: row.crate_name,
                    version: row.version,
                    path: row.path,
                    indexed_at: row.indexed_at,
                    match_line: line,
                    snippet,
                }
            })
            .collect::<Vec<_>>();

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
                reason: "matches are lexical and may require source.read confirmation".to_string(),
            }
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
            next_best_calls: vec!["source.read".to_string(), "crate.intel".to_string()],
            provenance: "local_postgres_index".to_string(),
            hits,
        };

        self.query_cache_put(
            "source.search",
            &cache_key,
            &serde_json::to_value(&response)
                .map_err(|e| format!("failed to encode source.search cache value: {e}"))?,
            300,
        )
        .await?;

        Ok(Json(response))
    }

    pub(crate) async fn handle_source_read(
        &self,
        request: SourceReadRequest,
    ) -> Result<Json<SourceReadResponse>, String> {
        let crate_name = normalize_required(request.crate_name, "crate_name")?;
        let path = normalize_required(request.path, "path")?;

        let row = if let Some(version) = normalize_optional(request.version) {
            let crate_row = tools::fetch_crate_core_by_name(&self.state.db, &crate_name)
                .await
                .map_err(|e| {
                    format!(
                        "source.read crate lookup failed for {crate_name}@{version}:{path}: {e}"
                    )
                })?
                .ok_or_else(|| format!("crate '{crate_name}' is not indexed locally"))?;

            let crate_version =
                tools::fetch_crate_version_by_name(&self.state.db, crate_row.id, &version)
                    .await
                    .map_err(|e| {
                        format!(
                            "source.read version lookup failed for {crate_name}@{version}:{path}: \
                             {e}"
                        )
                    })?
                    .ok_or_else(|| {
                        format!(
                            "version '{version}' for crate '{crate_name}' is not indexed locally"
                        )
                    })?;

            self.ensure_source_indexed(&crate_name, crate_version.id)
                .await?;

            tools::fetch_source_read_for_crate_version_path(
                &self.state.db,
                &crate_name,
                crate_version.id,
                &path,
            )
            .await
            .map_err(|e| {
                format!("source.read lookup failed for {crate_name}@{version}:{path}: {e}")
            })?
            .ok_or_else(|| format!("source file not found for {crate_name}@{version}:{path}"))?
        } else {
            // Best-effort on-demand source indexing for the latest version.
            if let Ok(Some(cr)) = tools::fetch_crate_core_by_name(&self.state.db, &crate_name).await
                && let Ok(Some(lv)) = tools::fetch_latest_crate_version(&self.state.db, cr.id).await
            {
                self.ensure_source_indexed(&crate_name, lv.id)
                    .await?;
            }

            tools::fetch_source_read_latest_for_crate_path(&self.state.db, &crate_name, &path)
                .await
                .map_err(|e| format!("source.read lookup failed for {crate_name}:{path}: {e}"))?
                .ok_or_else(|| format!("source file not found for {crate_name}:{path}"))?
        };

        let lines = row
            .content
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
            next_best_calls: vec!["source.search".to_string(), "symbol.search".to_string()],
            provenance: "local_postgres_index".to_string(),
        }))
    }
}
