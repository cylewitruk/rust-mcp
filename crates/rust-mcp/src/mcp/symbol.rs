use base64::Engine as _;
use rmcp::Json;
use serde::{Deserialize, Serialize};
use sqlx::{Postgres, QueryBuilder};

use super::models::{
    ConfidenceAssessment, ConfidenceLevel, SymbolSearchHit, SymbolSearchRequest,
    SymbolSearchResponse, SymbolSearchRow,
};
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
    collapse_by_canonical: bool,
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
        let collapse_by_canonical = request
            .collapse_by_canonical
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
            "collapse_by_canonical": collapse_by_canonical,
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
                || decoded.collapse_by_canonical != collapse_by_canonical
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

        let mut count_qb = QueryBuilder::<Postgres>::new(if collapse_by_canonical {
            "SELECT COUNT(DISTINCT (c.name, sf.path, s.name, s.kind))::BIGINT
             FROM symbols s
             JOIN crate_versions cv ON cv.id = s.crate_version_id
             JOIN crates c ON c.id = cv.crate_id
             JOIN source_files sf ON sf.id = s.source_file_id "
        } else {
            "SELECT COUNT(*)::BIGINT
             FROM symbols s
             JOIN crate_versions cv ON cv.id = s.crate_version_id
             JOIN crates c ON c.id = cv.crate_id
             JOIN source_files sf ON sf.id = s.source_file_id "
        });

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

        let mut qb = QueryBuilder::<Postgres>::new(if collapse_by_canonical {
            "SELECT DISTINCT ON (c.name, sf.path, s.name, s.kind)
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
             JOIN source_files sf ON sf.id = s.source_file_id "
        } else {
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
             JOIN source_files sf ON sf.id = s.source_file_id "
        });

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

        if collapse_by_canonical {
            qb.push(
                "ORDER BY
                c.name ASC,
                sf.path ASC,
                s.name ASC,
                s.kind ASC,
                cv.published_at DESC NULLS LAST,
                cv.id DESC,
                s.start_line ASC,
                s.end_line ASC,
                s.id ASC
             LIMIT ",
            );
        } else {
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
        }
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
                collapse_by_canonical,
            })?)
        } else {
            None
        };

        let confidence_assessment = if hits.is_empty() {
            ConfidenceAssessment {
                level: ConfidenceLevel::Low,
                reason: "no indexed symbols matched the query/filter set".to_string(),
            }
        } else {
            ConfidenceAssessment {
                level: ConfidenceLevel::Medium,
                reason: "symbol hits resolved by indexed name matching; verify with source.read"
                    .to_string(),
            }
        };

        let response = SymbolSearchResponse {
            query,
            crate_name,
            version,
            kind,
            include_all_versions,
            collapse_by_canonical,
            cursor,
            next_cursor,
            page: effective_page,
            limit,
            total_count,
            has_more,
            count: hits.len(),
            confidence: confidence_assessment
                .level
                .as_str()
                .to_string(),
            confidence_assessment,
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

#[cfg(test)]
mod tests {
    use std::env;
    use std::net::SocketAddr;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    use sqlx::postgres::PgPoolOptions;

    use super::*;
    use crate::config::{Config, LogFormat, TransportMode};
    use crate::state::AppState;

    fn test_config(database_url: String) -> Config {
        Config {
            mcp_transport: TransportMode::Http,
            http_bind: "127.0.0.1:43173"
                .parse::<SocketAddr>()
                .expect("valid socket address"),
            mcp_sse_keep_alive_secs: 15,
            mcp_sse_retry_ms: 3000,
            database_url,
            crates_io_base_url: "https://crates.io".to_string(),
            crates_io_user_agent: "rust-mcp/test".to_string(),
            crates_io_timeout_secs: 5,
            crates_io_min_interval_ms: 1,
            docs_rs_base_url: "https://docs.rs".to_string(),
            docs_rs_min_interval_ms: 1,
            osv_min_interval_ms: 1,
            database_min_connections: 1,
            database_max_connections: 2,
            max_concurrent_requests: 16,
            auto_migrate: false,
            cargo_registry_dir: PathBuf::from("/tmp"),
            data_dir: PathBuf::from("/tmp"),
            rustsec_db_dir: None,
            rustdoc_json_dir: None,
            prometheus_bind: "127.0.0.1:9090"
                .parse::<SocketAddr>()
                .expect("valid socket address"),
            rust_log: "warn".to_string(),
            log_format: LogFormat::Pretty,
        }
    }

    #[tokio::test]
    async fn symbol_search_collapse_by_canonical_deduplicates_versions() {
        let Some(database_url) = env::var("DATABASE_URL").ok() else {
            return;
        };

        let pool = match PgPoolOptions::new()
            .max_connections(2)
            .connect(&database_url)
            .await
        {
            Ok(pool) => pool,
            Err(_) => return,
        };

        if sqlx::migrate!("../../migrations")
            .run(&pool)
            .await
            .is_err()
        {
            return;
        }

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time after unix epoch")
            .as_nanos();
        let crate_name = format!("symbol_collapse_test_{stamp}");

        let crate_id: i64 = sqlx::query_scalar(
            "INSERT INTO crates (name, description, updated_at)
             VALUES ($1, $2, NOW())
             RETURNING id",
        )
        .bind(&crate_name)
        .bind("symbol collapse test")
        .fetch_one(&pool)
        .await
        .expect("insert test crate");

        let version1_id: i64 = sqlx::query_scalar(
            "INSERT INTO crate_versions (crate_id, version, published_at, yanked)
             VALUES ($1, '1.0.0', NOW() - INTERVAL '10 day', false)
             RETURNING id",
        )
        .bind(crate_id)
        .fetch_one(&pool)
        .await
        .expect("insert test version 1");

        let version2_id: i64 = sqlx::query_scalar(
            "INSERT INTO crate_versions (crate_id, version, published_at, yanked)
             VALUES ($1, '2.0.0', NOW() - INTERVAL '1 day', false)
             RETURNING id",
        )
        .bind(crate_id)
        .fetch_one(&pool)
        .await
        .expect("insert test version 2");

        let source_v1_id: i64 = sqlx::query_scalar(
            "INSERT INTO source_files (
                 crate_version_id, path, sha256, file_size, language, content
             ) VALUES (
                 $1, 'src/lib.rs', 'sha-v1', 10, 'Rust', 'pub trait Serializer {}'
             )
             RETURNING id",
        )
        .bind(version1_id)
        .fetch_one(&pool)
        .await
        .expect("insert source for v1");

        let source_v2_id: i64 = sqlx::query_scalar(
            "INSERT INTO source_files (
                 crate_version_id, path, sha256, file_size, language, content
             ) VALUES (
                 $1, 'src/lib.rs', 'sha-v2', 10, 'Rust', 'pub trait Serializer {}'
             )
             RETURNING id",
        )
        .bind(version2_id)
        .fetch_one(&pool)
        .await
        .expect("insert source for v2");

        for (version_id, source_id) in [(version1_id, source_v1_id), (version2_id, source_v2_id)] {
            sqlx::query(
                "INSERT INTO symbols (
                     crate_version_id, source_file_id, name, kind, signature, visibility,
                     start_line, end_line, index_source
                 ) VALUES (
                     $1, $2, 'Serializer', 'trait', 'trait Serializer', 'public', 1, 1,
                     'local_cache'
                 )",
            )
            .bind(version_id)
            .bind(source_id)
            .execute(&pool)
            .await
            .expect("insert symbol row");
        }

        let config = test_config(database_url);
        let state = AppState {
            outbound_rate_limiters: Arc::new(crate::state::OutboundRateLimiters::new(&config)),
            config,
            db: pool.clone(),
            http: reqwest::Client::new(),
        };
        let server = McpServer::new(state);

        let non_collapsed = server
            .handle_symbol_search(SymbolSearchRequest {
                query: "Serializer".to_string(),
                crate_name: Some(crate_name.clone()),
                version: None,
                kind: Some("trait".to_string()),
                include_all_versions: Some(true),
                collapse_by_canonical: Some(false),
                cursor: None,
                page: Some(1),
                limit: Some(50),
            })
            .await
            .expect("non-collapsed symbol search succeeds")
            .0;

        assert_eq!(non_collapsed.total_count, 2);
        assert_eq!(non_collapsed.count, 2);

        let collapsed = server
            .handle_symbol_search(SymbolSearchRequest {
                query: "Serializer".to_string(),
                crate_name: Some(crate_name.clone()),
                version: None,
                kind: Some("trait".to_string()),
                include_all_versions: Some(true),
                collapse_by_canonical: Some(true),
                cursor: None,
                page: Some(1),
                limit: Some(50),
            })
            .await
            .expect("collapsed symbol search succeeds")
            .0;

        assert_eq!(collapsed.total_count, 1);
        assert_eq!(collapsed.count, 1);
        assert_eq!(collapsed.hits[0].version, "2.0.0");

        sqlx::query("DELETE FROM crates WHERE id = $1")
            .bind(crate_id)
            .execute(&pool)
            .await
            .expect("cleanup test crate");
    }
}
