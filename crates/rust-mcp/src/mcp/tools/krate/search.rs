use rmcp::Json;
pub use rust_mcp_types::types::krate::{
    CrateSearchHit, CrateSearchRequest, CrateSearchResponse, CrateSearchSort,
};
use serde::{Deserialize, Serialize};
use sqlx::{Postgres, QueryBuilder};

use crate::mcp::models::{
    ConfidenceAssessment, ConfidenceLevel, CrateSearchRow, ResponseFreshnessSource,
};
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
        let mut qb = QueryBuilder::<Postgres>::new(
            "SELECT c.id AS crate_id, c.name, c.description, c.repository_url, c.docs_url, \
             c.homepage_url, c.categories, c.keywords, COALESCE(MAX(cv.total_downloads), \
             0)::BIGINT AS total_downloads, (ARRAY_REMOVE(ARRAY_AGG(cv.version ORDER BY \
             cv.published_at DESC NULLS LAST, cv.id DESC), NULL))[1] AS latest_version, \
             MAX(cv.published_at)::TEXT AS latest_published_at, COALESCE(COUNT(DISTINCT \
             dep.from_version_id), 0)::BIGINT AS dependent_count, ",
        );

        if let Some(q) = query {
            let name_like = format!("%{q}%");
            qb.push("(similarity(c.name, ");
            qb.push_bind(q);
            qb.push(") * 2.0 + similarity(COALESCE(c.description, ''), ");
            qb.push_bind(q);
            qb.push(") + CASE WHEN c.name ILIKE ");
            qb.push_bind(name_like);
            qb.push(
                " THEN 1.5 ELSE 0 END + CASE WHEN to_tsvector('english', COALESCE(c.description, \
                 '')) @@ websearch_to_tsquery('english', ",
            );
            qb.push_bind(q);
            qb.push(") THEN 1.0 ELSE 0 END)::DOUBLE PRECISION AS relevance_score ");
        } else {
            qb.push("0.0::DOUBLE PRECISION AS relevance_score ");
        }

        qb.push(
            "FROM crates c LEFT JOIN crate_versions cv ON cv.crate_id = c.id LEFT JOIN \
             dependency_edges dep ON dep.to_crate_id = c.id ",
        );

        let mut has_where = false;

        if let Some(q) = query {
            if !has_where {
                qb.push("WHERE ");
                has_where = true;
            }
            qb.push("(");
            qb.push("c.name ILIKE ");
            qb.push_bind(format!("%{q}%"));
            qb.push(" OR COALESCE(c.description, '') ILIKE ");
            qb.push_bind(format!("%{q}%"));
            qb.push(" OR COALESCE(c.keywords, '{}'::TEXT[]) @> ARRAY[");
            qb.push_bind(q);
            qb.push("]::TEXT[]");
            qb.push(" OR COALESCE(c.categories, '{}'::TEXT[]) @> ARRAY[");
            qb.push_bind(q);
            qb.push("]::TEXT[]");
            qb.push(
                " OR to_tsvector('english', COALESCE(c.description, '')) @@ \
                 websearch_to_tsquery('english', ",
            );
            qb.push_bind(q);
            qb.push(")");
            qb.push(")");
        }

        if let Some(c) = category {
            if !has_where {
                qb.push("WHERE ");
                has_where = true;
            } else {
                qb.push("AND ");
            }
            qb.push("COALESCE(c.categories, '{}'::TEXT[]) @> ARRAY[");
            qb.push_bind(c);
            qb.push("]::TEXT[] ");
        }

        if let Some(k) = keyword {
            if !has_where {
                qb.push("WHERE ");
            } else {
                qb.push("AND ");
            }
            qb.push("COALESCE(c.keywords, '{}'::TEXT[]) @> ARRAY[");
            qb.push_bind(k);
            qb.push("]::TEXT[] ");
        }

        qb.push(
            "GROUP BY c.id, c.name, c.description, c.repository_url, c.docs_url, c.homepage_url, \
             c.categories, c.keywords ",
        );

        match sort {
            CrateSearchSort::Relevance => {
                if query.is_some() {
                    qb.push("ORDER BY relevance_score DESC, total_downloads DESC, c.name ASC ");
                } else {
                    qb.push("ORDER BY total_downloads DESC, c.name ASC ");
                }
            }
            CrateSearchSort::Downloads => {
                qb.push("ORDER BY total_downloads DESC, c.name ASC ");
            }
            CrateSearchSort::Recent => {
                qb.push(
                    "ORDER BY MAX(cv.published_at) DESC NULLS LAST, total_downloads DESC, c.name \
                     ASC ",
                );
            }
        }

        qb.push("LIMIT ");
        qb.push_bind(i64::from(limit));
        qb.push(" OFFSET ");
        qb.push_bind(i64::from(offset));

        qb.build_query_as::<CrateSearchRow>()
            .fetch_all(&self.state.db)
            .await
    }

    pub(crate) async fn handle_crate_search(
        &self,
        request: CrateSearchRequest,
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
        .map_err(|e| format!("failed to build crate.search cache key: {e}"))?;
        if let Some(cached) = self
            .query_cache_get("crate.search", &cache_key)
            .await?
        {
            let cached_response = serde_json::from_value::<CrateSearchResponse>(cached)
                .map_err(|e| format!("failed to decode crate.search cache entry: {e}"))?;
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
            return Err("cursor does not match current crate.search filters".to_string());
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
            .map_err(|e| format!("crate.search query failed: {e}"))?;

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
                .ensure_freshness_for_interaction(row.crate_id, &row.name, latest_version)
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
            next_best_calls: if hits.is_empty() {
                vec!["index.sync_crates".to_string()]
            } else {
                vec![
                    "crate.intel".to_string(),
                    "crate.versions".to_string(),
                    "crate.graph".to_string(),
                ]
            },
            provenance: "local_postgres_index".to_string(),
            hits,
        };

        self.query_cache_put(
            "crate.search",
            &cache_key,
            &serde_json::to_value(&response)
                .map_err(|e| format!("failed to encode crate.search cache value: {e}"))?,
            300,
        )
        .await?;

        Ok(Json(response))
    }
}
