use rmcp::Json;
pub use rust_mcp_types::types::krate::{
    CrateSearchHit, CrateSearchRequest, CrateSearchResponse, CrateSearchSort,
};
use serde::{Deserialize, Serialize};

use crate::db::tools;
use crate::mcp::models::{
    ConfidenceAssessment, ConfidenceLevel, CrateSearchRow, ResponseFreshnessSource,
};
use crate::mcp::progress::ToolCallContext;
use crate::mcp::server::McpServer;
use crate::mcp::utils::{
    CursorToken, decode_cursor, encode_cursor, match_reasons, normalize_optional,
    resolve_pagination, search_limit, sync_page,
};

#[derive(Debug, Serialize)]
struct CrateSearchCacheKey<'a> {
    query: Option<&'a str>,
    category: Option<&'a str>,
    keyword: Option<&'a str>,
    sort: CrateSearchSort,
    cursor: Option<&'a str>,
    page: u32,
    limit: u32,
}

#[derive(Debug, Serialize, Deserialize)]
struct CrateCursorToken {
    v: u8,
    offset: u32,
    limit: u32,
    query: Option<String>,
    category: Option<String>,
    keyword: Option<String>,
    sort: CrateSearchSort,
}

impl CursorToken for CrateCursorToken {
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
    async fn run_crate_search(
        &self,
        query: Option<&str>,
        category: Option<&str>,
        keyword: Option<&str>,
        sort: CrateSearchSort,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<CrateSearchRow>, sqlx::Error> {
        tools::search_crates(
            &self.state.db,
            &tools::CrateSearchParams {
                query,
                category,
                keyword,
                sort,
                limit: i64::from(limit),
                offset: i64::from(offset),
            },
        )
        .await
    }

    /// Handles the `crate_search` tool call.
    pub async fn handle_crate_search(
        &self,
        request: CrateSearchRequest,
        tcx: ToolCallContext,
    ) -> Result<Json<CrateSearchResponse>, String> {
        let query = normalize_optional(request.query);
        let category = normalize_optional(request.category);
        let keyword = normalize_optional(request.keyword);
        let cursor = normalize_optional(request.cursor);
        let page = sync_page(request.page);
        let requested_limit = search_limit(request.limit);

        let sort = request
            .sort
            .unwrap_or_else(|| {
                if query.is_some() {
                    CrateSearchSort::Relevance
                } else {
                    CrateSearchSort::Downloads
                }
            });

        let cache_key = serde_json::to_string(&CrateSearchCacheKey {
            query: query.as_deref(),
            category: category.as_deref(),
            keyword: keyword.as_deref(),
            sort,
            cursor: cursor.as_deref(),
            page,
            limit: requested_limit,
        })
        .map_err(|e| format!("failed to build crate_search cache key: {e}"))?;
        if let Some(cached) = self
            .query_cache_get("crate_search", &cache_key)
            .await?
        {
            let cached_response = serde_json::from_value::<CrateSearchResponse>(cached)
                .map_err(|e| format!("failed to decode crate_search cache entry: {e}"))?;
            return Ok(Json(cached_response));
        }

        let decoded_cursor = cursor
            .as_deref()
            .map(decode_cursor::<CrateCursorToken>)
            .transpose()?;
        if let Some(ref decoded) = decoded_cursor
            && (decoded.query != query
                || decoded.category != category
                || decoded.keyword != keyword
                || decoded.sort != sort)
        {
            return Err("cursor does not match current crate_search filters".to_string());
        }
        let pagination = resolve_pagination(
            decoded_cursor.as_ref(),
            request.limit.is_some(),
            requested_limit,
            page,
        )?;
        let (offset, limit, effective_page) =
            (pagination.offset, pagination.limit, pagination.effective_page);

        let mut rows = self
            .run_crate_search(
                query.as_deref(),
                category.as_deref(),
                keyword.as_deref(),
                sort,
                limit.saturating_add(1),
                offset,
            )
            .await
            .map_err(|e| format!("crate_search query failed: {e}"))?;

        let has_more = rows.len() > limit as usize;
        if has_more {
            rows.truncate(limit as usize);
        }
        let next_cursor = if has_more {
            Some(encode_cursor(&CrateCursorToken {
                v: 1,
                offset: offset.saturating_add(limit),
                limit,
                query: query.clone(),
                category: category.clone(),
                keyword: keyword.clone(),
                sort,
            })?)
        } else {
            None
        };

        let mut freshness_checks_performed = 0_usize;
        let mut refresh_jobs_enqueued = 0_usize;

        for row in rows.iter().take(5) {
            let Some(latest_version) = row.latest_version.as_deref() else {
                continue;
            };

            let freshness = self
                .ensure_freshness_for_interaction(row.crate_id, &row.name, latest_version, &tcx)
                .await?;

            if freshness.freshness_check_performed {
                freshness_checks_performed += 1;
            }
            if freshness.refresh_enqueued {
                refresh_jobs_enqueued += 1;
            }
        }

        let hits = rows
            .into_iter()
            .map(|row| CrateSearchHit {
                match_reasons: match_reasons(
                    &row,
                    query.as_deref(),
                    category.as_deref(),
                    keyword.as_deref(),
                ),
                name: row.name,
                description: row.description,
                repository_url: row.repository_url,
                docs_url: row.docs_url,
                homepage_url: row.homepage_url,
                categories: row.categories,
                keywords: row.keywords,
                total_downloads: row.total_downloads,
                latest_published_at: row.latest_published_at,
                dependent_crates: row.dependent_count,
                rank_score: row.relevance_score,
            })
            .collect::<Vec<_>>();

        let confidence_assessment = if hits.is_empty() {
            ConfidenceAssessment {
                level: ConfidenceLevel::Low,
                reason: "no crates matched the provided filters".to_string(),
            }
        } else {
            ConfidenceAssessment {
                level: ConfidenceLevel::High,
                reason: "ranked crate hits resolved from local index".to_string(),
            }
        };

        let response = CrateSearchResponse {
            query,
            category,
            keyword,
            sort,
            cursor,
            next_cursor,
            page: effective_page,
            limit,
            has_more,
            truncated: has_more,
            count: hits.len(),
            freshness_checks_performed,
            refresh_jobs_enqueued,
            freshness: vec![
                ResponseFreshnessSource {
                    source: "local_postgres_index".to_string(),
                    status: "fresh".to_string(),
                    checked_at: None,
                },
                ResponseFreshnessSource {
                    source: "crates.io".to_string(),
                    status: if freshness_checks_performed > 0 {
                        "probed".to_string()
                    } else {
                        "not_checked".to_string()
                    },
                    checked_at: None,
                },
            ],
            confidence: confidence_assessment
                .level
                .as_str()
                .to_string(),
            confidence_assessment,
            suggested_next_tools: if hits.is_empty() {
                vec!["index_sync_crates".to_string()]
            } else {
                vec![
                    "crate_intel".to_string(),
                    "crate_versions".to_string(),
                    "crate_graph".to_string(),
                ]
            },
            provenance: "local_postgres_index".to_string(),
            hits,
        };

        self.query_cache_put(
            "crate_search",
            &cache_key,
            &serde_json::to_value(&response)
                .map_err(|e| format!("failed to encode crate_search cache value: {e}"))?,
            300,
        )
        .await?;

        Ok(Json(response))
    }
}
