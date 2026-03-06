//! Integration tests for [`DbSessionManager`] session persistence.

use rmcp::transport::streamable_http_server::session::SessionManager;
use rmcp::transport::streamable_http_server::session::local::SessionConfig;
use rust_mcp::mcp::session::DbSessionManager;
use rust_mcp_testing::postgres::PostgresTestContainer;

/// Helper: create a migrated PgPool from a test container.
async fn migrated_pool(pg: &PostgresTestContainer) -> sqlx::PgPool {
    let state = rust_mcp::state::AppState::connect(super::common::test_config(
        pg.connection_string()
            .to_string(),
        std::path::PathBuf::from("/tmp"),
    ))
    .await
    .expect("connect");
    state
        .run_migrations()
        .await
        .expect("migrate");
    state.db
}

#[tokio::test]
async fn session_persists_across_manager_instances() {
    let pg = PostgresTestContainer::start()
        .await
        .expect("start postgres");
    let pool = migrated_pool(&pg).await;

    // --- First manager: create a session ---
    let manager1 = DbSessionManager::new(pool.clone(), SessionConfig::default());
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

    // --- Second manager: session should be discoverable via DB fallback ---
    let manager2 = DbSessionManager::new(pool.clone(), SessionConfig::default());

    // has_session should find it through the DB fallback.
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
    let pool = migrated_pool(&pg).await;

    let manager = DbSessionManager::new(pool.clone(), SessionConfig::default());

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
    let pool = migrated_pool(&pg).await;

    let manager = DbSessionManager::new(pool.clone(), SessionConfig::default());
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
