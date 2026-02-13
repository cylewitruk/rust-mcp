use base64::Engine as _;
use rmcp::Json;
use serde::{Deserialize, Serialize};
use sqlx::{Postgres, QueryBuilder};

use super::models::{SymbolSearchHit, SymbolSearchRequest, SymbolSearchResponse, SymbolSearchRow};
use super::server::McpServer;
use super::utils::{normalize_optional, normalize_required, symbol_search_limit, sync_page};

#[derive(Debug, Serialize, Deserialize)]
struct SymbolCursorToken {
    v: u8,
    offset: u32,
    limit: u32,
    query: String,
    crate_name: Option<String>,
    version: Option<String>,
    kind: Option<String>,
    include_all_versions: bool,
}

fn decode_symbol_cursor(token: &str) -> Result<SymbolCursorToken, String> {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(token)
        .map_err(|_| "cursor is invalid".to_string())?;
    let decoded = serde_json::from_slice::<SymbolCursorToken>(&bytes)
        .map_err(|_| "cursor is invalid".to_string())?;

    if decoded.v != 1 {
        return Err("cursor version is not supported".to_string());
    }
    if decoded.limit == 0 {
        return Err("cursor is invalid".to_string());
    }

    Ok(decoded)
}

fn encode_symbol_cursor(token: &SymbolCursorToken) -> Result<String, String> {
    let bytes =
        serde_json::to_vec(token).map_err(|e| format!("cursor serialization failed: {e}"))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

impl McpServer {
    pub(super) async fn handle_symbol_search(
        &self,
        request: SymbolSearchRequest,
    ) -> Result<Json<SymbolSearchResponse>, String> {
        let query = normalize_required(request.query, "query")?;
        let crate_name = normalize_optional(request.crate_name);
        let version = normalize_optional(request.version);
        let kind = normalize_optional(request.kind);
        let include_all_versions = request
            .include_all_versions
            .unwrap_or(false);
        let cursor = normalize_optional(request.cursor);
        let page = sync_page(request.page);
        let requested_limit = symbol_search_limit(request.limit);

        let cache_key = serde_json::to_string(&serde_json::json!({
            "query": query,
            "crate_name": crate_name,
            "version": version,
            "kind": kind,
            "include_all_versions": include_all_versions,
            "cursor": cursor,
            "page": page,
            "limit": requested_limit,
        }))
        .map_err(|e| format!("failed to build symbol.search cache key: {e}"))?;
        if let Some(cached) = self
            .query_cache_get("symbol.search", &cache_key)
            .await?
        {
            let cached_response = serde_json::from_value::<SymbolSearchResponse>(cached)
                .map_err(|e| format!("failed to decode symbol.search cache entry: {e}"))?;
            return Ok(Json(cached_response));
        }

        let (offset, limit, effective_page) = if let Some(ref token) = cursor {
            let decoded = decode_symbol_cursor(token)?;
            if decoded.query != query
                || decoded.crate_name != crate_name
                || decoded.version != version
                || decoded.kind != kind
                || decoded.include_all_versions != include_all_versions
            {
                return Err("cursor does not match current symbol.search filters".to_string());
            }

            if request.limit.is_some() && requested_limit != decoded.limit {
                return Err("limit must match the cursor page size".to_string());
            }

            (decoded.offset, decoded.limit, (decoded.offset / decoded.limit).saturating_add(1))
        } else {
            ((page - 1) * requested_limit, requested_limit, page)
        };

        let mut count_qb = QueryBuilder::<Postgres>::new(
            "SELECT COUNT(*)::BIGINT
             FROM symbols s
             JOIN crate_versions cv ON cv.id = s.crate_version_id
             JOIN crates c ON c.id = cv.crate_id
             JOIN source_files sf ON sf.id = s.source_file_id ",
        );

        let mut has_where = false;
        if let Some(ref crate_filter) = crate_name {
            count_qb.push(if has_where { "AND " } else { "WHERE " });
            has_where = true;
            count_qb.push("c.name = ");
            count_qb.push_bind(crate_filter);
            count_qb.push(' ');
        }

        if let Some(ref version_filter) = version {
            count_qb.push(if has_where { "AND " } else { "WHERE " });
            has_where = true;
            count_qb.push("cv.version = ");
            count_qb.push_bind(version_filter);
            count_qb.push(' ');
        }

        if let Some(ref kind_filter) = kind {
            count_qb.push(if has_where { "AND " } else { "WHERE " });
            has_where = true;
            count_qb.push("s.kind = ");
            count_qb.push_bind(kind_filter);
            count_qb.push(' ');
        }

        count_qb.push(if has_where { "AND " } else { "WHERE " });
        has_where = true;
        count_qb.push("s.name ILIKE ");
        count_qb.push_bind(format!("%{query}%"));
        count_qb.push(' ');

        if version.is_none() && !include_all_versions {
            count_qb.push(if has_where { "AND " } else { "WHERE " });
            count_qb.push(
                "cv.id = (
                    SELECT cv2.id
                    FROM crate_versions cv2
                    WHERE cv2.crate_id = c.id
                    ORDER BY cv2.published_at DESC NULLS LAST, cv2.id DESC
                    LIMIT 1
                 ) ",
            );
        }

        let total_count = count_qb
            .build_query_scalar::<i64>()
            .fetch_one(&self.state.db)
            .await
            .map_err(|e| format!("symbol.search count query failed: {e}"))?
            .max(0) as usize;

        let mut qb = QueryBuilder::<Postgres>::new(
            "SELECT
                s.id AS _symbol_id,
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
        has_where = true;
        qb.push("s.name ILIKE ");
        qb.push_bind(format!("%{query}%"));
        qb.push(' ');

        if version.is_none() && !include_all_versions {
            qb.push(if has_where { "AND " } else { "WHERE " });
            qb.push(
                "cv.id = (
                    SELECT cv2.id
                    FROM crate_versions cv2
                    WHERE cv2.crate_id = c.id
                    ORDER BY cv2.published_at DESC NULLS LAST, cv2.id DESC
                    LIMIT 1
                 ) ",
            );
        }

        qb.push(
            "ORDER BY
                c.name ASC,
                cv.published_at DESC NULLS LAST,
                cv.id DESC,
                s.name ASC,
                s.kind ASC,
                sf.path ASC,
                s.start_line ASC,
                s.end_line ASC,
                s.id ASC
             LIMIT ",
        );
        qb.push_bind(i64::from(limit));
        qb.push(" OFFSET ");
        qb.push_bind(i64::from(offset));

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

        let has_more = ((offset as usize) + hits.len()) < total_count;
        let next_cursor = if has_more {
            Some(encode_symbol_cursor(&SymbolCursorToken {
                v: 1,
                offset: offset.saturating_add(limit),
                limit,
                query: query.clone(),
                crate_name: crate_name.clone(),
                version: version.clone(),
                kind: kind.clone(),
                include_all_versions,
            })?)
        } else {
            None
        };

        let response = SymbolSearchResponse {
            query,
            crate_name,
            version,
            kind,
            include_all_versions,
            cursor,
            next_cursor,
            page: effective_page,
            limit,
            total_count,
            has_more,
            count: hits.len(),
            confidence: if hits.is_empty() { "low".to_string() } else { "medium".to_string() },
            next_best_calls: if hits.is_empty() {
                vec!["index.refresh".to_string(), "source.search".to_string()]
            } else {
                vec!["source.read".to_string(), "crate.intel".to_string()]
            },
            provenance: "local_postgres_index".to_string(),
            hits,
        };

        self.query_cache_put(
            "symbol.search",
            &cache_key,
            &serde_json::to_value(&response)
                .map_err(|e| format!("failed to encode symbol.search cache value: {e}"))?,
            300,
        )
        .await?;

        Ok(Json(response))
    }
}
