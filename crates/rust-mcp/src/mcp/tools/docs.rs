use std::collections::{HashSet, VecDeque};

use rmcp::{Json, schemars};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Postgres, QueryBuilder};

use crate::integration::docs_rs::{DocsRsClient, discover_docs_paths, extract_title, strip_html};
use crate::mcp::models::{ConfidenceAssessment, ConfidenceLevel};
use crate::mcp::server::McpServer;
use crate::mcp::utils::{
    docs_search_limit, normalize_optional, normalize_required, sync_page, sync_per_page,
};

fn default_confidence_assessment() -> ConfidenceAssessment {
    ConfidenceAssessment {
        level: ConfidenceLevel::Low,
        reason: "confidence assessment unavailable in cached legacy response".to_string(),
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DocsSearchRequest {
    pub query: String,
    pub crate_name: Option<String>,
    pub version: Option<String>,
    pub path_prefix: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct DocsSearchResponse {
    pub query: String,
    pub crate_name: Option<String>,
    pub version: Option<String>,
    pub path_prefix: Option<String>,
    pub limit: u32,
    pub count: usize,
    pub confidence: String,
    #[serde(default = "default_confidence_assessment")]
    pub confidence_assessment: ConfidenceAssessment,
    pub next_best_calls: Vec<String>,
    pub provenance: String,
    pub hits: Vec<DocsSearchHit>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct DocsSearchHit {
    pub crate_name: String,
    pub version: String,
    pub path: String,
    pub title: Option<String>,
    pub source_url: Option<String>,
    pub indexed_at: String,
    pub snippet: String,
}

#[derive(Debug, FromRow)]
pub(crate) struct DocsSearchRow {
    pub(crate) crate_name: String,
    pub(crate) version: String,
    pub(crate) path: String,
    pub(crate) title: Option<String>,
    pub(crate) source_url: Option<String>,
    pub(crate) indexed_at: String,
    pub(crate) content: String,
}

#[derive(Debug, FromRow)]
pub(crate) struct DocsSyncCandidateRow {
    pub(crate) crate_name: String,
    pub(crate) version: String,
    pub(crate) crate_version_id: i64,
}

#[derive(Debug, Default)]
pub(crate) struct DocsRefreshOutcome {
    pub(crate) versions_processed: usize,
    pub(crate) pages_written: usize,
    pub(crate) touched_versions: Vec<String>,
    pub(crate) errors: Vec<String>,
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
    pub(crate) async fn sync_docs_pages(
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

        let candidates = sqlx::query_as::<_, DocsSyncCandidateRow>(
            "SELECT
                c.name AS crate_name,
                cv.version,
                cv.id AS crate_version_id
             FROM crate_versions cv
             JOIN crates c ON c.id = cv.crate_id
             WHERE ($1::TEXT IS NULL OR c.name = $1)
             ORDER BY cv.published_at DESC NULLS LAST, cv.id DESC
             LIMIT $2 OFFSET $3",
        )
        .bind(crate_filter.as_deref())
        .bind(i64::from(per_page))
        .bind(i64::from(offset))
        .fetch_all(&self.state.db)
        .await
        .map_err(|e| format!("docs sync failed to load crate versions: {e}"))?;

        let mut outcome = DocsRefreshOutcome::default();
        let docs_rs = DocsRsClient::new(&self.state);

        for candidate in candidates {
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

                let rows_affected = sqlx::query(
                    "INSERT INTO docs_pages (
                        crate_version_id,
                        path,
                        title,
                        content,
                        source_url,
                        indexed_at
                     ) VALUES (
                        $1, $2, $3, $4, $5, NOW()
                     )
                     ON CONFLICT (crate_version_id, path) DO UPDATE
                     SET title = EXCLUDED.title,
                         content = EXCLUDED.content,
                         source_url = EXCLUDED.source_url,
                         indexed_at = NOW()
                     WHERE docs_pages.title IS DISTINCT FROM EXCLUDED.title
                        OR docs_pages.content IS DISTINCT FROM EXCLUDED.content
                        OR docs_pages.source_url IS DISTINCT FROM EXCLUDED.source_url",
                )
                .bind(candidate.crate_version_id)
                .bind(&path)
                .bind(title)
                .bind(content)
                .bind(source_url)
                .execute(&self.state.db)
                .await
                .map_err(|e| {
                    format!(
                        "failed to upsert docs page {} for {}@{}: {e}",
                        path, candidate.crate_name, candidate.version
                    )
                })?
                .rows_affected();

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

    pub(crate) async fn handle_docs_search(
        &self,
        request: DocsSearchRequest,
    ) -> Result<Json<DocsSearchResponse>, String> {
        let query = normalize_required(request.query, "query")?;
        let crate_name = normalize_optional(request.crate_name);
        let version = normalize_optional(request.version);
        let path_prefix = normalize_optional(request.path_prefix);
        let limit = docs_search_limit(request.limit);

        let cache_key = serde_json::to_string(&serde_json::json!({
            "query": query,
            "crate_name": crate_name,
            "version": version,
            "path_prefix": path_prefix,
            "limit": limit,
        }))
        .map_err(|e| format!("failed to build docs.search cache key: {e}"))?;
        if let Some(cached) = self
            .query_cache_get("docs.search", &cache_key)
            .await?
        {
            let cached_response = serde_json::from_value::<DocsSearchResponse>(cached)
                .map_err(|e| format!("failed to decode docs.search cache entry: {e}"))?;
            return Ok(Json(cached_response));
        }

        let mut qb = QueryBuilder::<Postgres>::new(
            "SELECT
                c.name AS crate_name,
                cv.version,
                dp.path,
                dp.title,
                dp.source_url,
                dp.indexed_at::TEXT AS indexed_at,
                dp.content
             FROM docs_pages dp
             JOIN crate_versions cv ON cv.id = dp.crate_version_id
             JOIN crates c ON c.id = cv.crate_id ",
        );

        let mut has_where = false;
        if let Some(ref crate_filter) = crate_name {
            qb.push(if has_where { "AND " } else { "WHERE " });
            has_where = true;
            qb.push("c.name = ");
            qb.push_bind(crate_filter);
            qb.push(' ');
        }
        if let Some(ref version_filter) = version {
            qb.push(if has_where { "AND " } else { "WHERE " });
            has_where = true;
            qb.push("cv.version = ");
            qb.push_bind(version_filter);
            qb.push(' ');
        }
        if let Some(ref path_filter) = path_prefix {
            qb.push(if has_where { "AND " } else { "WHERE " });
            has_where = true;
            qb.push("dp.path ILIKE ");
            qb.push_bind(format!("{path_filter}%"));
            qb.push(' ');
        }

        qb.push(if has_where { "AND " } else { "WHERE " });
        qb.push("(dp.content ILIKE ");
        qb.push_bind(format!("%{query}%"));
        qb.push(" OR COALESCE(dp.title, '') ILIKE ");
        qb.push_bind(format!("%{query}%"));
        qb.push(" OR dp.path ILIKE ");
        qb.push_bind(format!("%{query}%"));
        qb.push(") ");

        qb.push("ORDER BY dp.indexed_at DESC, c.name ASC, dp.path ASC LIMIT ");
        qb.push_bind(i64::from(limit));

        let rows = qb
            .build_query_as::<DocsSearchRow>()
            .fetch_all(&self.state.db)
            .await
            .map_err(|e| format!("docs.search query failed: {e}"))?;

        let hits = rows
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
            limit,
            count: hits.len(),
            confidence: confidence_assessment
                .level
                .as_str()
                .to_string(),
            confidence_assessment,
            next_best_calls: if hits.is_empty() {
                vec!["index.refresh".to_string(), "crate.intel".to_string()]
            } else {
                vec!["source.read".to_string(), "crate.intel".to_string()]
            },
            provenance: "local_postgres_index".to_string(),
            hits,
        };

        self.query_cache_put(
            "docs.search",
            &cache_key,
            &serde_json::to_value(&response)
                .map_err(|e| format!("failed to encode docs.search cache value: {e}"))?,
            300,
        )
        .await?;

        Ok(Json(response))
    }
}
