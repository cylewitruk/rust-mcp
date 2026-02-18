use rmcp::Json;
pub use rust_mcp_types::types::symbol::{
    SymbolSearchHit, SymbolSearchRequest, SymbolSearchResponse,
};
use serde::{Deserialize, Serialize};

use crate::db::tools;
use crate::mcp::models::{ConfidenceAssessment, ConfidenceLevel};
use crate::mcp::server::McpServer;
use crate::mcp::utils::{
    CursorToken, decode_cursor, encode_cursor, normalize_optional, normalize_required,
    resolve_pagination, symbol_search_limit, sync_page,
};

#[derive(Debug, Serialize)]
struct SymbolSearchCacheKey<'a> {
    query: &'a str,
    crate_name: Option<&'a str>,
    version: Option<&'a str>,
    kind: Option<&'a str>,
    include_all_versions: bool,
    collapse_by_canonical: bool,
    cursor: Option<&'a str>,
    page: u32,
    limit: u32,
}

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

impl CursorToken for SymbolCursorToken {
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
    pub(crate) async fn handle_symbol_search(
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

        let cache_key = serde_json::to_string(&SymbolSearchCacheKey {
            query: &query,
            crate_name: crate_name.as_deref(),
            version: version.as_deref(),
            kind: kind.as_deref(),
            include_all_versions,
            collapse_by_canonical,
            cursor: cursor.as_deref(),
            page,
            limit: requested_limit,
        })
        .map_err(|e| format!("failed to build symbol.search cache key: {e}"))?;
        if let Some(cached) = self
            .query_cache_get("symbol.search", &cache_key)
            .await?
        {
            let cached_response = serde_json::from_value::<SymbolSearchResponse>(cached)
                .map_err(|e| format!("failed to decode symbol.search cache entry: {e}"))?;
            return Ok(Json(cached_response));
        }

        let decoded_cursor = cursor
            .as_deref()
            .map(decode_cursor::<SymbolCursorToken>)
            .transpose()?;
        if let Some(ref decoded) = decoded_cursor
            && (decoded.query != query
                || decoded.crate_name != crate_name
                || decoded.version != version
                || decoded.kind != kind
                || decoded.include_all_versions != include_all_versions
                || decoded.collapse_by_canonical != collapse_by_canonical)
        {
            return Err("cursor does not match current symbol.search filters".to_string());
        }
        let pagination = resolve_pagination(
            decoded_cursor.as_ref(),
            request.limit.is_some(),
            requested_limit,
            page,
        )?;
        let (offset, limit, effective_page) =
            (pagination.offset, pagination.limit, pagination.effective_page);

        let filters = tools::SymbolSearchFilters {
            query: &query,
            crate_name: crate_name.as_deref(),
            version: version.as_deref(),
            kind: kind.as_deref(),
            include_all_versions,
            collapse_by_canonical,
        };

        let total_count = tools::count_symbol_search_hits(&self.state.db, &filters)
            .await
            .map_err(|e| format!("symbol.search count query failed: {e}"))?
            .max(0) as usize;

        let rows = tools::search_symbol_hits(
            &self.state.db,
            &filters,
            i64::from(limit),
            i64::from(offset),
        )
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
            Some(encode_cursor(&SymbolCursorToken {
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
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    use sqlx::PgPool;
    use sqlx::postgres::PgPoolOptions;

    use super::*;
    use crate::config::Config;
    use crate::db::indexing;
    use crate::db::models::{IndexedExtractionBatch, IndexedSymbolInsert};
    use crate::integration::crates_io::{
        CratesIoCrateDetailResponse, CratesIoCrateRecord, CratesIoVersionRecord,
    };
    use crate::state::AppState;

    async fn seed_symbol_collapse_fixture(pool: &PgPool, crate_name: &str) -> i64 {
        let detail = CratesIoCrateDetailResponse {
            krate: CratesIoCrateRecord {
                name: crate_name.to_string(),
                description: Some("symbol collapse test".to_string()),
                repository: None,
                documentation: None,
                homepage: None,
                max_version: Some("2.0.0".to_string()),
            },
            versions: vec![
                CratesIoVersionRecord {
                    num: "1.0.0".to_string(),
                    created_at: Some("2024-01-01T00:00:00Z".to_string()),
                    updated_at: None,
                    yanked: false,
                    downloads: Some(10),
                    checksum: Some("checksum-v1".to_string()),
                    rust_version: None,
                    license: None,
                    features: BTreeMap::new(),
                },
                CratesIoVersionRecord {
                    num: "2.0.0".to_string(),
                    created_at: Some("2025-01-01T00:00:00Z".to_string()),
                    updated_at: None,
                    yanked: false,
                    downloads: Some(20),
                    checksum: Some("checksum-v2".to_string()),
                    rust_version: None,
                    license: None,
                    features: BTreeMap::new(),
                },
            ],
            keywords: vec![],
            categories: vec![],
        };

        let categories = Vec::<String>::new();
        let keywords = Vec::<String>::new();
        indexing::persist_crate_sync(pool, &detail, None, None, None, &categories, &keywords)
            .await
            .expect("persist crate fixture");

        let crate_row = tools::fetch_crate_core_by_name(pool, crate_name)
            .await
            .expect("load crate fixture")
            .expect("crate fixture exists");

        for (version, sha) in [("1.0.0", "sha-v1"), ("2.0.0", "sha-v2")] {
            let version_row = tools::fetch_crate_version_by_name(pool, crate_row.id, version)
                .await
                .expect("load fixture version")
                .expect("fixture version exists");

            indexing::upsert_source_file_unconditional(
                pool,
                version_row.id,
                "src/lib.rs",
                sha,
                10,
                Some("Rust"),
                "pub trait Serializer {}",
            )
            .await
            .expect("upsert fixture source");

            let source_file_id =
                indexing::fetch_source_file_id_required(pool, version_row.id, "src/lib.rs")
                    .await
                    .expect("load fixture source id");

            let extraction = IndexedExtractionBatch {
                symbols: vec![IndexedSymbolInsert {
                    name: "Serializer".to_string(),
                    kind: "trait".to_string(),
                    visibility: Some("public".to_string()),
                    signature: Some("trait Serializer".to_string()),
                    start_line: 1,
                    end_line: 1,
                    ..Default::default()
                }],
                ..Default::default()
            };

            indexing::replace_source_file_index_rows(
                pool,
                version_row.id,
                source_file_id,
                "local_cache",
                &extraction,
            )
            .await
            .expect("index fixture symbol");
        }

        crate_row.id
    }

    #[tokio::test]
    async fn symbol_search_collapse_by_canonical_deduplicates_versions() {
        let config = Config::load_from_env();

        let pool = match PgPoolOptions::new()
            .max_connections(2)
            .connect(&config.database_url)
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
        let crate_id = seed_symbol_collapse_fixture(&pool, &crate_name).await;

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
