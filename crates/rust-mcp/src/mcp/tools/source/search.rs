use rmcp::Json;
pub use rust_mcp_types::types::source::{
    SourceSearchHit, SourceSearchMode, SourceSearchRequest, SourceSearchResponse,
};
use sqlx::{FromRow, Postgres, QueryBuilder};

use crate::mcp::models::{
    ConfidenceAssessment, ConfidenceLevel, SourceReadRequest, SourceReadResponse, SourceReadRow,
};
use crate::mcp::server::McpServer;
use crate::mcp::utils::{
    normalize_optional, normalize_required, path_glob_to_like, source_read_end_line,
    source_search_limit,
};

#[derive(Debug, FromRow)]
pub(crate) struct SourceSearchRow {
    pub(crate) crate_name: String,
    pub(crate) version: String,
    pub(crate) path: String,
    pub(crate) content: String,
    pub(crate) indexed_at: String,
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
        let limit = source_search_limit(request.limit);

        let mut qb = QueryBuilder::<Postgres>::new(
            "SELECT
                c.name AS crate_name,
                cv.version,
                sf.path,
                sf.content,
                sf.indexed_at::TEXT AS indexed_at
             FROM source_files sf
             JOIN crate_versions cv ON cv.id = sf.crate_version_id
             JOIN crates c ON c.id = cv.crate_id ",
        );

        let mut has_where = false;
        if let Some(ref crate_name_filter) = crate_name {
            qb.push(if has_where { "AND " } else { "WHERE " });
            has_where = true;
            qb.push("c.name = ");
            qb.push_bind(crate_name_filter);
            qb.push(' ');
        }

        if let Some(ref version_filter) = version {
            qb.push(if has_where { "AND " } else { "WHERE " });
            has_where = true;
            qb.push("cv.version = ");
            qb.push_bind(version_filter);
            qb.push(' ');
        }

        if let Some(ref path_filter) = path_glob {
            qb.push(if has_where { "AND " } else { "WHERE " });
            has_where = true;
            qb.push("sf.path ILIKE ");
            qb.push_bind(path_glob_to_like(path_filter));
            qb.push(" ESCAPE '\\\\' ");
        }

        qb.push(if has_where { "AND " } else { "WHERE " });
        match mode {
            SourceSearchMode::Text => {
                qb.push("sf.content ILIKE ");
                qb.push_bind(format!("%{query}%"));
                qb.push(' ');
            }
            SourceSearchMode::Regex => {
                qb.push("sf.content ~* ");
                qb.push_bind(&query);
                qb.push(' ');
            }
        }

        qb.push("ORDER BY sf.indexed_at DESC, c.name ASC, sf.path ASC LIMIT ");
        qb.push_bind(i64::from(limit));

        let rows = qb
            .build_query_as::<SourceSearchRow>()
            .fetch_all(&self.state.db)
            .await
            .map_err(|e| format!("source.search query failed: {e}"))?;

        let hits = rows
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

        Ok(Json(SourceSearchResponse {
            query,
            crate_name,
            version,
            path_glob,
            mode,
            limit,
            count: hits.len(),
            confidence: confidence_assessment
                .level
                .as_str()
                .to_string(),
            confidence_assessment,
            next_best_calls: vec!["source.read".to_string(), "crate.intel".to_string()],
            provenance: "local_postgres_index".to_string(),
            hits,
        }))
    }

    pub(crate) async fn handle_source_read(
        &self,
        request: SourceReadRequest,
    ) -> Result<Json<SourceReadResponse>, String> {
        let crate_name = normalize_required(request.crate_name, "crate_name")?;
        let path = normalize_required(request.path, "path")?;

        let row = if let Some(version) = normalize_optional(request.version) {
            sqlx::query_as::<_, SourceReadRow>(
                "SELECT
                    c.name AS crate_name,
                    cv.version,
                    sf.path,
                    sf.content
                 FROM source_files sf
                 JOIN crate_versions cv ON cv.id = sf.crate_version_id
                 JOIN crates c ON c.id = cv.crate_id
                 WHERE c.name = $1 AND cv.version = $2 AND sf.path = $3
                 LIMIT 1",
            )
            .bind(&crate_name)
            .bind(&version)
            .bind(&path)
            .fetch_optional(&self.state.db)
            .await
            .map_err(|e| {
                format!("source.read lookup failed for {crate_name}@{version}:{path}: {e}")
            })?
            .ok_or_else(|| format!("source file not found for {crate_name}@{version}:{path}"))?
        } else {
            sqlx::query_as::<_, SourceReadRow>(
                "SELECT
                    c.name AS crate_name,
                    cv.version,
                    sf.path,
                    sf.content
                 FROM source_files sf
                 JOIN crate_versions cv ON cv.id = sf.crate_version_id
                 JOIN crates c ON c.id = cv.crate_id
                 WHERE c.name = $1 AND sf.path = $2
                 ORDER BY cv.published_at DESC NULLS LAST, cv.id DESC
                 LIMIT 1",
            )
            .bind(&crate_name)
            .bind(&path)
            .fetch_optional(&self.state.db)
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
