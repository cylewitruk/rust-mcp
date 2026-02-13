use rmcp::Json;
use sqlx::{Postgres, QueryBuilder};

use super::models::{
    DocsSearchHit, DocsSearchRequest, DocsSearchResponse, DocsSearchRow, DocsSyncCandidateRow,
};
use super::server::McpServer;
use super::utils::{
    docs_search_limit, normalize_optional, normalize_required, sync_page, sync_per_page,
};

#[derive(Debug, Default)]
pub(super) struct DocsRefreshOutcome {
    pub(super) versions_processed: usize,
    pub(super) pages_written: usize,
    pub(super) touched_versions: Vec<String>,
    pub(super) errors: Vec<String>,
}

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

fn extract_title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let start_tag = "<title>";
    let end_tag = "</title>";

    let start = lower.find(start_tag)? + start_tag.len();
    let end = lower[start..].find(end_tag)? + start;
    let title = html[start..end].trim();
    if title.is_empty() {
        None
    } else {
        Some(
            title
                .chars()
                .take(300)
                .collect(),
        )
    }
}

fn strip_html(html: &str) -> String {
    let mut output = String::with_capacity(html.len().min(200_000));
    let mut in_tag = false;

    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => output.push(ch),
            _ => {}
        }
    }

    let normalized = output
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    normalized
        .chars()
        .take(1_000_000)
        .collect()
}

impl McpServer {
    fn docs_rs_url(&self, path: &str) -> String {
        let base = self
            .state
            .config
            .docs_rs_base_url
            .trim_end_matches('/');
        let suffix = path.trim_start_matches('/');
        format!("{base}/{suffix}")
    }

    async fn fetch_docs_page_html(&self, path: &str) -> Result<String, String> {
        let url = self.docs_rs_url(path);
        let response = self
            .state
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("docs fetch failed {url}: {e}"))?;

        let status = response.status();
        if !status.is_success() {
            return Err(format!("docs fetch failed {url}: status {status}"));
        }

        response
            .text()
            .await
            .map_err(|e| format!("docs body read failed {url}: {e}"))
    }

    pub(super) async fn sync_docs_pages(
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

        for candidate in candidates {
            let paths = vec![
                format!("{}/{}/{}/", candidate.crate_name, candidate.version, candidate.crate_name),
                format!(
                    "{}/{}/{}/index.html",
                    candidate.crate_name, candidate.version, candidate.crate_name
                ),
            ];

            let mut written_any = false;
            for path in paths {
                let html = match self
                    .fetch_docs_page_html(&path)
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
                let source_url = self.docs_rs_url(&path);

                sqlx::query(
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
                         indexed_at = NOW()",
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
                })?;

                written_any = true;
                outcome.pages_written += 1;
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

    pub(super) async fn handle_docs_search(
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

        let response = DocsSearchResponse {
            query,
            crate_name,
            version,
            path_prefix,
            limit,
            count: hits.len(),
            confidence: if hits.is_empty() { "low".to_string() } else { "medium".to_string() },
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
