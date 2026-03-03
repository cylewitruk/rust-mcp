use std::collections::{HashSet, VecDeque};

use rmcp::Json;
pub use rust_mcp_types::types::docs::{DocsSearchHit, DocsSearchRequest, DocsSearchResponse};
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::db::{indexing, tools};
use crate::integration::docs_rs::{DocsRsClient, discover_docs_paths, extract_title, strip_html};
use crate::mcp::models::{ConfidenceAssessment, ConfidenceLevel};
use crate::mcp::server::McpServer;
use crate::mcp::utils::{
    CursorToken, decode_cursor, docs_search_limit, encode_cursor, normalize_optional,
    normalize_required, resolve_pagination, sync_page, sync_per_page,
};

#[derive(Debug, Serialize)]
struct DocsSearchCacheKey<'a> {
    query: &'a str,
    crate_name: Option<&'a str>,
    version: Option<&'a str>,
    path_prefix: Option<&'a str>,
    cursor: Option<&'a str>,
    page: u32,
    limit: u32,
}

#[derive(Debug, Serialize, Deserialize)]
struct DocsCursorToken {
    v: u8,
    offset: u32,
    limit: u32,
    query: String,
    crate_name: Option<String>,
    version: Option<String>,
    path_prefix: Option<String>,
}

impl CursorToken for DocsCursorToken {
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

#[derive(Debug, Default)]
/// Outcome of a docs page refresh operation.
pub struct DocsRefreshOutcome {
    /// Number of crate versions whose documentation was processed.
    pub versions_processed: usize,
    /// Number of documentation pages written to the database.
    pub pages_written: usize,
    /// Version strings that were touched during the refresh.
    pub touched_versions: Vec<String>,
    /// Errors encountered during the refresh.
    pub errors: Vec<String>,
}

const DOCS_DISCOVERY_MAX_PAGES: usize = 64;

fn docs_snippet(content: &str, query: &str) -> String {
    if content.is_empty() {
        return String::new();
    }

    let q = query.to_ascii_lowercase();
    let lower = content.to_ascii_lowercase();
    if let Some(idx) = lower.find(&q) {
        let start = idx.saturating_sub(80);
        let end = (idx + query.len() + 140).min(content.len());
        return content[start..end]
            .trim()
            .to_string();
    }

    content
        .chars()
        .take(220)
        .collect()
}

impl McpServer {
    /// Fetches and indexes documentation pages from docs.rs.
    pub async fn sync_docs_pages(
        &self,
        crate_name: Option<String>,
        page: Option<u32>,
        per_page: Option<u32>,
    ) -> Result<DocsRefreshOutcome, String> {
        let crate_filter = match crate_name {
            Some(name) => Some(normalize_required(name, "crate_name")?),
            None => None,
        };
        let page = sync_page(page);
        let per_page = sync_per_page(per_page);
        let offset = (page.saturating_sub(1)) * per_page;

        let candidates = indexing::fetch_rustdoc_sync_candidates(
            &self.state.db,
            crate_filter.as_deref(),
            i64::from(per_page),
            i64::from(offset),
            false,
            false,
            None,
        )
        .await
        .map_err(|e| format!("docs sync failed to load crate versions: {e}"))?;

        let mut outcome = DocsRefreshOutcome::default();
        let docs_rs = DocsRsClient::new(&self.state);

        for candidate in candidates {
            info!(
                crate_name = %candidate.crate_name,
                version = %candidate.version,
                "syncing docs.rs pages"
            );
            let crate_prefix = format!("{}/{}/", candidate.crate_name, candidate.version);
            let module_root = format!("{}{}/", crate_prefix, candidate.crate_name);
            let module_root_index = format!("{}index.html", module_root);
            let mut queue = VecDeque::from([module_root, module_root_index]);
            let mut pending = queue
                .iter()
                .cloned()
                .collect::<HashSet<_>>();
            let mut seen = HashSet::new();
            let mut written_any = false;

            while let Some(path) = queue.pop_front() {
                pending.remove(&path);
                if !seen.insert(path.clone()) {
                    continue;
                }

                let html = match docs_rs
                    .fetch_page_html(&path)
                    .await
                {
                    Ok(html) => html,
                    Err(error) => {
                        outcome.errors.push(error);
                        continue;
                    }
                };

                let title = extract_title(&html);
                let content = strip_html(&html);
                let source_url = docs_rs.url(&path);

                let rows_affected = tools::upsert_docs_page_if_changed(
                    &self.state.db,
                    candidate.crate_version_id,
                    &path,
                    title.as_deref(),
                    &content,
                    Some(source_url.as_str()),
                )
                .await
                .map_err(|e| {
                    format!(
                        "failed to upsert docs page {} for {}@{}: {e}",
                        path, candidate.crate_name, candidate.version
                    )
                })?;

                if rows_affected > 0 {
                    written_any = true;
                    outcome.pages_written += rows_affected as usize;
                }

                if seen.len() + pending.len() >= DOCS_DISCOVERY_MAX_PAGES {
                    continue;
                }

                for discovered in
                    discover_docs_paths(docs_rs.base_url(), &path, &html, &crate_prefix)
                {
                    if seen.contains(&discovered) || pending.contains(&discovered) {
                        continue;
                    }
                    if seen.len() + pending.len() >= DOCS_DISCOVERY_MAX_PAGES {
                        break;
                    }
                    pending.insert(discovered.clone());
                    queue.push_back(discovered);
                }
            }

            outcome.versions_processed += 1;
            if written_any {
                outcome
                    .touched_versions
                    .push(format!("{}@{}", candidate.crate_name, candidate.version));
            }
        }

        Ok(outcome)
    }

    /// Handles the `docs.search` tool call.
    pub async fn handle_docs_search(
        &self,
        request: DocsSearchRequest,
    ) -> Result<Json<DocsSearchResponse>, String> {
        let query = normalize_required(request.query, "query")?;
        let crate_name = normalize_optional(request.crate_name);
        let version = normalize_optional(request.version);
        let path_prefix = normalize_optional(request.path_prefix);
        let cursor = normalize_optional(request.cursor);
        let page = sync_page(request.page);
        let requested_limit = docs_search_limit(request.limit);

        let decoded_cursor = cursor
            .as_deref()
            .map(decode_cursor::<DocsCursorToken>)
            .transpose()?;
        if let Some(ref decoded) = decoded_cursor
            && (decoded.query != query
                || decoded.crate_name != crate_name
                || decoded.version != version
                || decoded.path_prefix != path_prefix)
        {
            return Err("cursor does not match current docs.search filters".to_string());
        }
        let pagination = resolve_pagination(
            decoded_cursor.as_ref(),
            request.limit.is_some(),
            requested_limit,
            page,
        )?;
        let (offset, limit, effective_page) =
            (pagination.offset, pagination.limit, pagination.effective_page);

        let cache_key = serde_json::to_string(&DocsSearchCacheKey {
            query: &query,
            crate_name: crate_name.as_deref(),
            version: version.as_deref(),
            path_prefix: path_prefix.as_deref(),
            cursor: cursor.as_deref(),
            page,
            limit,
        })
        .map_err(|e| format!("failed to build docs.search cache key: {e}"))?;
        if let Some(cached) = self
            .query_cache_get("docs_search", &cache_key)
            .await?
        {
            let cached_response = serde_json::from_value::<DocsSearchResponse>(cached)
                .map_err(|e| format!("failed to decode docs.search cache entry: {e}"))?;
            return Ok(Json(cached_response));
        }

        let rows = tools::search_docs_pages(
            &self.state.db,
            &tools::DocsSearchParams {
                crate_name: crate_name.as_deref(),
                version: version.as_deref(),
                path_prefix: path_prefix.as_deref(),
                query: &query,
                limit: i64::from(limit.saturating_add(1)),
                offset: i64::from(offset),
            },
        )
        .await
        .map_err(|e| format!("docs_search query failed: {e}"))?;

        let mut hits = rows
            .into_iter()
            .map(|row| DocsSearchHit {
                crate_name: row.crate_name,
                version: row.version,
                path: row.path,
                title: row.title,
                source_url: row.source_url,
                indexed_at: row.indexed_at,
                snippet: docs_snippet(&row.content, &query),
            })
            .collect::<Vec<_>>();

        let has_more = hits.len() > limit as usize;
        if has_more {
            hits.truncate(limit as usize);
        }
        let next_cursor = if has_more {
            Some(encode_cursor(&DocsCursorToken {
                v: 1,
                offset: offset.saturating_add(limit),
                limit,
                query: query.clone(),
                crate_name: crate_name.clone(),
                version: version.clone(),
                path_prefix: path_prefix.clone(),
            })?)
        } else {
            None
        };

        let confidence_assessment = if hits.is_empty() {
            ConfidenceAssessment {
                level: ConfidenceLevel::Low,
                reason: "no indexed docs pages matched the query/filter set".to_string(),
            }
        } else {
            ConfidenceAssessment {
                level: ConfidenceLevel::Medium,
                reason: "docs hits are text-ranked snippets from indexed docs.rs pages".to_string(),
            }
        };

        let response = DocsSearchResponse {
            query,
            crate_name,
            version,
            path_prefix,
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
            suggested_next_tools: if hits.is_empty() {
                vec!["index_refresh".to_string(), "crate_intel".to_string()]
            } else {
                vec!["source_read".to_string(), "crate_intel".to_string()]
            },
            provenance: "local_postgres_index".to_string(),
            hits,
        };

        self.query_cache_put(
            "docs_search",
            &cache_key,
            &serde_json::to_value(&response)
                .map_err(|e| format!("failed to encode docs.search cache value: {e}"))?,
            300,
        )
        .await?;

        Ok(Json(response))
    }
}
