//! Integration tests for [`DbSessionManager`] session persistence.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use rmcp::transport::streamable_http_server::session::SessionManager;
use rmcp::transport::streamable_http_server::session::local::SessionConfig;
use rust_mcp::mcp::server::McpServer;
use rust_mcp::mcp::session::{DbSessionManager, ServiceFactory};
use rust_mcp::state::AppState;
use rust_mcp_testing::local_mcp::LocalMcpHttpHarness;
use rust_mcp_testing::postgres::PostgresTestContainer;
use serde_json::json;

/// Helper: create a migrated PgPool and AppState from a test container.
async fn migrated_state(pg: &PostgresTestContainer) -> (sqlx::PgPool, AppState) {
    let state = AppState::connect(super::common::test_config(
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
    (state.db.clone(), state)
}

/// Helper: create a service factory for tests.
fn test_service_factory(state: &AppState) -> ServiceFactory {
    let s = state.clone();
    Arc::new(move || Ok(McpServer::new(s.clone())))
}

/// Helper: spin up a full MCP HTTP harness backed by the given database URL.
async fn mcp_harness(database_url: &str) -> LocalMcpHttpHarness {
    let config = super::common::test_config(database_url.to_string(), PathBuf::from("/tmp"));
    let state = AppState::connect(config.clone())
        .await
        .expect("connect");
    let router =
        rust_mcp::http::router(state.clone(), config, super::common::test_prometheus_handle());
    let mcp = LocalMcpHttpHarness::spawn(router)
        .await
        .expect("spawn harness");
    mcp.wait_until_ready(Duration::from_secs(20))
        .await
        .expect("harness ready");
    mcp
}

#[tokio::test]
async fn session_persists_across_manager_instances() {
    let pg = PostgresTestContainer::start()
        .await
        .expect("start postgres");
    let (pool, app_state) = migrated_state(&pg).await;
    let factory = test_service_factory(&app_state);

    // --- First manager: create a session ---
    let manager1 = DbSessionManager::new(pool.clone(), SessionConfig::default(), factory.clone());
    let (session_id, _transport) = manager1
        .create_session()
        .await
        .expect("create_session should succeed");

    // Session is in-memory AND in the DB.
    assert!(
        manager1
            .has_session(&session_id)
            .await
            .expect("has_session"),
        "session should exist in first manager"
    );

    // Verify DB row exists.
    let db_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sessions WHERE session_id = $1)")
            .bind(session_id.as_ref())
            .fetch_one(&pool)
            .await
            .expect("db query");
    assert!(db_exists, "session should be persisted in database");

    // --- Drop first manager (simulates server restart) ---
    drop(manager1);

    // --- Second manager: session should be discoverable via DB fallback
    // (reconstituted from DB) ---
    let manager2 = DbSessionManager::new(pool.clone(), SessionConfig::default(), factory.clone());

    // has_session should find it through the DB and reconstitute.
    assert!(
        manager2
            .has_session(&session_id)
            .await
            .expect("has_session on new manager"),
        "session should be found via DB in second manager"
    );

    // --- close_session removes from both memory and DB ---
    manager2
        .close_session(&session_id)
        .await
        .expect("close_session");

    let db_exists_after_close: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sessions WHERE session_id = $1)")
            .bind(session_id.as_ref())
            .fetch_one(&pool)
            .await
            .expect("db query after close");
    assert!(!db_exists_after_close, "session should be removed from database after close");

    assert!(
        !manager2
            .has_session(&session_id)
            .await
            .expect("has_session after close"),
        "session should not exist after close"
    );
}

#[tokio::test]
async fn stale_sessions_are_pruned() {
    let pg = PostgresTestContainer::start()
        .await
        .expect("start postgres");
    let (pool, app_state) = migrated_state(&pg).await;
    let factory = test_service_factory(&app_state);

    let manager = DbSessionManager::new(pool.clone(), SessionConfig::default(), factory.clone());

    // Create two sessions.
    let (stale_id, _t1) = manager
        .create_session()
        .await
        .expect("create stale session");
    let (fresh_id, _t2) = manager
        .create_session()
        .await
        .expect("create fresh session");

    // Backdate the first session's last_seen_at to 2 hours ago.
    sqlx::query(
        "UPDATE sessions SET last_seen_at = NOW() - INTERVAL '2 hours' WHERE session_id = $1",
    )
    .bind(stale_id.as_ref())
    .execute(&pool)
    .await
    .expect("backdate stale session");

    // Prune sessions idle for more than 1 hour.
    let pruned = rust_mcp::mcp::session::prune_stale_sessions(&pool, 3600)
        .await
        .expect("prune");
    assert_eq!(pruned, 1, "should prune exactly one stale session");

    // Stale session should be gone from DB.
    let stale_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sessions WHERE session_id = $1)")
            .bind(stale_id.as_ref())
            .fetch_one(&pool)
            .await
            .expect("check stale");
    assert!(!stale_exists, "stale session should have been pruned");

    // Fresh session should still exist.
    let fresh_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sessions WHERE session_id = $1)")
            .bind(fresh_id.as_ref())
            .fetch_one(&pool)
            .await
            .expect("check fresh");
    assert!(fresh_exists, "fresh session should NOT have been pruned");
}

#[tokio::test]
async fn prune_with_no_stale_sessions_returns_zero() {
    let pg = PostgresTestContainer::start()
        .await
        .expect("start postgres");
    let (pool, app_state) = migrated_state(&pg).await;
    let factory = test_service_factory(&app_state);

    let manager = DbSessionManager::new(pool.clone(), SessionConfig::default(), factory.clone());
    let (_id, _t) = manager
        .create_session()
        .await
        .expect("create session");

    // Nothing is stale — all sessions were just created.
    let pruned = rust_mcp::mcp::session::prune_stale_sessions(&pool, 3600)
        .await
        .expect("prune");
    assert_eq!(pruned, 0, "no sessions should be pruned");
}

// ──────────────────────────────────────────────────────────────────────────────
// Session reconstitution after restart
// ──────────────────────────────────────────────────────────────────────────────

/// Simulates a server restart: initialize a session on server 1, drop it,
/// spin up server 2 sharing the same database. A tool call using the old
/// session ID should succeed because the session is transparently
/// reconstituted from the database.
#[tokio::test]
async fn session_reconstituted_after_restart_allows_tool_call() {
    let pg = PostgresTestContainer::start()
        .await
        .expect("start postgres");
    let (pool, _app_state) = migrated_state(&pg).await;
    let db_url = pg
        .connection_string()
        .to_string();

    // --- Server 1: initialize a session ---
    let mcp1 = mcp_harness(&db_url).await;
    let _ = mcp1
        .initialize("reconstitution-test-server1")
        .await
        .expect("initialize on server 1");
    let session_id = mcp1
        .session_id()
        .await
        .expect("server 1 should have a session ID");

    // Verify the session row exists in the database.
    let db_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sessions WHERE session_id = $1)")
            .bind(&session_id)
            .fetch_one(&pool)
            .await
            .expect("db query");
    assert!(db_exists, "session should be persisted in database");

    // --- Drop server 1 (simulates restart) ---
    drop(mcp1);

    // --- Server 2: same database, fresh in-memory state ---
    let mcp2 = mcp_harness(&db_url).await;

    // Inject the old session ID so the harness uses it for the next request.
    mcp2.set_session_id(session_id.clone())
        .await;

    // A tool call using the old session ID should succeed because the
    // session is reconstituted from the database.
    let ping = mcp2
        .call_tool("ping", json!({"message": "after-restart"}))
        .await
        .expect("tool call on reconstituted session should succeed");
    assert!(ping.get("result").is_some(), "ping should return a result");

    // The session should still exist in the database.
    let db_exists_after: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sessions WHERE session_id = $1)")
            .bind(&session_id)
            .fetch_one(&pool)
            .await
            .expect("db query after reconstitution");
    assert!(db_exists_after, "session should still be in database after reconstitution");
}
