use rmcp::Json;
use sqlx::{Postgres, QueryBuilder};

use super::models::{SymbolSearchHit, SymbolSearchRequest, SymbolSearchResponse, SymbolSearchRow};
use super::server::McpServer;
use super::utils::{normalize_optional, normalize_required, symbol_search_limit};

impl McpServer {
    pub(super) async fn handle_symbol_search(
        &self,
        request: SymbolSearchRequest,
    ) -> Result<Json<SymbolSearchResponse>, String> {
        let query = normalize_required(request.query, "query")?;
        let crate_name = normalize_optional(request.crate_name);
        let version = normalize_optional(request.version);
        let kind = normalize_optional(request.kind);
        let limit = symbol_search_limit(request.limit);

        let mut qb = QueryBuilder::<Postgres>::new(
            "SELECT
                c.name AS crate_name,
                cv.version,
                sf.path AS source_path,
                s.name,
                s.kind,
                s.signature,
                s.visibility,
                s.start_line,
                s.end_line,
                s.index_source,
                s.indexed_at::TEXT AS indexed_at
             FROM symbols s
             JOIN crate_versions cv ON cv.id = s.crate_version_id
             JOIN crates c ON c.id = cv.crate_id
             JOIN source_files sf ON sf.id = s.source_file_id ",
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

        if let Some(ref kind_filter) = kind {
            qb.push(if has_where { "AND " } else { "WHERE " });
            has_where = true;
            qb.push("s.kind = ");
            qb.push_bind(kind_filter);
            qb.push(' ');
        }

        qb.push(if has_where { "AND " } else { "WHERE " });
        qb.push("s.name ILIKE ");
        qb.push_bind(format!("%{query}%"));
        qb.push(' ');

        qb.push("ORDER BY s.indexed_at DESC, c.name ASC, s.name ASC LIMIT ");
        qb.push_bind(i64::from(limit));

        let rows = qb
            .build_query_as::<SymbolSearchRow>()
            .fetch_all(&self.state.db)
            .await
            .map_err(|e| format!("symbol.search query failed: {e}"))?;

        let hits = rows
            .into_iter()
            .map(|row| SymbolSearchHit {
                crate_name: row.crate_name,
                version: row.version,
                source_path: row.source_path,
                name: row.name,
                kind: row.kind,
                signature: row.signature,
                visibility: row.visibility,
                start_line: row.start_line,
                end_line: row.end_line,
                index_source: row.index_source,
                indexed_at: row.indexed_at,
            })
            .collect::<Vec<_>>();

        Ok(Json(SymbolSearchResponse {
            query,
            crate_name,
            version,
            kind,
            limit,
            count: hits.len(),
            confidence: if hits.is_empty() { "low".to_string() } else { "medium".to_string() },
            next_best_calls: if hits.is_empty() {
                vec!["index.refresh".to_string(), "source.search".to_string()]
            } else {
                vec!["source.read".to_string(), "crate.intel".to_string()]
            },
            provenance: "local_postgres_index".to_string(),
            hits,
        }))
    }
}
