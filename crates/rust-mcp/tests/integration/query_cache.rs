//! Integration tests for query cache invalidation on re-index.

use std::path::PathBuf;

use rust_mcp::db::tools::{
    delete_query_cache_for_crate, fetch_query_cache_value, upsert_query_cache_value,
};
use rust_mcp::state::AppState;
use rust_mcp_testing::postgres::PostgresTestContainer;
use serde_json::json;

use super::common;

/// Helper: create a migrated `PgPool` from a test container.
async fn migrated_pool(pg: &PostgresTestContainer) -> sqlx::PgPool {
    let state = AppState::connect(common::test_config(
        pg.connection_string()
            .to_string(),
        PathBuf::from("/tmp"),
    ))
    .await
    .expect("connect");
    state
        .run_migrations()
        .await
        .expect("migrate");
    state.db
}

/// Builds a cache key matching the serialization format used by tool handlers.
fn cache_key_with_crate(crate_name: &str, query: &str) -> String {
    json!({"query": query, "crate_name": crate_name}).to_string()
}

/// Builds a cache key WITHOUT a `crate_name` field, like `crate_search`.
fn cache_key_without_crate(query: &str) -> String {
    json!({"query": query, "sort": "relevance"}).to_string()
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

/// Invalidation removes all cache entries matching the target crate name.
#[tokio::test]
async fn invalidation_removes_matching_entries() {
    let pg = PostgresTestContainer::start()
        .await
        .expect("start pg");
    let pool = migrated_pool(&pg).await;

    let key1 = cache_key_with_crate("tokio", "async runtime");
    let key2 = cache_key_with_crate("tokio", "spawn task");

    upsert_query_cache_value(&pool, &key1, "symbol_search", &json!({"r": 1}), 3600)
        .await
        .expect("insert key1");
    upsert_query_cache_value(&pool, &key2, "source_search", &json!({"r": 2}), 3600)
        .await
        .expect("insert key2");

    let deleted = delete_query_cache_for_crate(&pool, "tokio")
        .await
        .expect("invalidation");
    assert_eq!(deleted, 2, "both tokio entries should be deleted");

    // Verify they're actually gone.
    let v1 = fetch_query_cache_value(&pool, &key1, "symbol_search")
        .await
        .expect("fetch key1");
    let v2 = fetch_query_cache_value(&pool, &key2, "source_search")
        .await
        .expect("fetch key2");
    assert!(v1.is_none(), "key1 should be gone after invalidation");
    assert!(v2.is_none(), "key2 should be gone after invalidation");
}

/// Invalidation does NOT affect entries for differently-named crates, even with
/// a shared prefix (e.g. "tokio" must not delete "tokio-util" entries).
#[tokio::test]
async fn invalidation_does_not_affect_similarly_named_crates() {
    let pg = PostgresTestContainer::start()
        .await
        .expect("start pg");
    let pool = migrated_pool(&pg).await;

    let tokio_key = cache_key_with_crate("tokio", "spawn");
    let tokio_util_key = cache_key_with_crate("tokio-util", "codec");
    let tokio_stream_key = cache_key_with_crate("tokio-stream", "StreamExt");

    for (key, source) in [
        (&tokio_key, "symbol_search"),
        (&tokio_util_key, "symbol_search"),
        (&tokio_stream_key, "docs_search"),
    ] {
        upsert_query_cache_value(&pool, key, source, &json!({"ok": true}), 3600)
            .await
            .expect("insert");
    }

    let deleted = delete_query_cache_for_crate(&pool, "tokio")
        .await
        .expect("invalidation");
    assert_eq!(deleted, 1, "only the exact 'tokio' entry should be deleted");

    // tokio-util and tokio-stream entries survive.
    assert!(
        fetch_query_cache_value(&pool, &tokio_util_key, "symbol_search")
            .await
            .expect("fetch tokio-util")
            .is_some(),
        "tokio-util entry must survive"
    );
    assert!(
        fetch_query_cache_value(&pool, &tokio_stream_key, "docs_search")
            .await
            .expect("fetch tokio-stream")
            .is_some(),
        "tokio-stream entry must survive"
    );
}

/// Invalidation does NOT affect cache entries that lack a `crate_name` field
/// (e.g. `crate_search` keys).
#[tokio::test]
async fn invalidation_does_not_affect_global_cache_entries() {
    let pg = PostgresTestContainer::start()
        .await
        .expect("start pg");
    let pool = migrated_pool(&pg).await;

    let crate_specific = cache_key_with_crate("serde", "derive");
    let global = cache_key_without_crate("serde");

    upsert_query_cache_value(&pool, &crate_specific, "symbol_search", &json!({"r": 1}), 3600)
        .await
        .expect("insert crate-specific");
    upsert_query_cache_value(&pool, &global, "crate_search", &json!({"r": 2}), 3600)
        .await
        .expect("insert global");

    let deleted = delete_query_cache_for_crate(&pool, "serde")
        .await
        .expect("invalidation");
    assert_eq!(deleted, 1, "only the crate-specific entry should be deleted");

    // Global entry survives.
    assert!(
        fetch_query_cache_value(&pool, &global, "crate_search")
            .await
            .expect("fetch global")
            .is_some(),
        "global crate_search entry must survive invalidation"
    );
}

/// Invalidation returns zero when no matching entries exist.
#[tokio::test]
async fn invalidation_returns_zero_for_unknown_crate() {
    let pg = PostgresTestContainer::start()
        .await
        .expect("start pg");
    let pool = migrated_pool(&pg).await;

    let deleted = delete_query_cache_for_crate(&pool, "nonexistent-crate")
        .await
        .expect("invalidation");
    assert_eq!(deleted, 0);
}

/// Invalidation is idempotent — calling it twice returns zero on the second
/// call.
#[tokio::test]
async fn invalidation_is_idempotent() {
    let pg = PostgresTestContainer::start()
        .await
        .expect("start pg");
    let pool = migrated_pool(&pg).await;

    let key = cache_key_with_crate("hyper", "client");
    upsert_query_cache_value(&pool, &key, "source_search", &json!({"r": 1}), 3600)
        .await
        .expect("insert");

    let first = delete_query_cache_for_crate(&pool, "hyper")
        .await
        .expect("first invalidation");
    assert_eq!(first, 1);

    let second = delete_query_cache_for_crate(&pool, "hyper")
        .await
        .expect("second invalidation");
    assert_eq!(second, 0, "second call should find nothing to delete");
}
