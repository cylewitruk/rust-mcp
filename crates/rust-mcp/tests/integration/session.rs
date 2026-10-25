//! Integration tests for legacy-protocol session persistence.

use std::path::PathBuf;
use std::time::Duration;

use futures::future;
use rmcp::model::{ClientCapabilities, Implementation, InitializeRequestParams};
use rmcp::transport::streamable_http_server::session::store::{SessionState, SessionStore as _};
use rust_mcp::mcp::session::PgSessionStore;
use rust_mcp::state::AppState;
use rust_mcp_testing::local_mcp::LocalMcpHttpHarness;
use rust_mcp_testing::mcp_client::McpProtocol;
use rust_mcp_testing::postgres::PostgresTestContainer;
use serde_json::{Value, json};
use sqlx::PgPool;

/// Spins up a full MCP HTTP harness backed by the given database URL.
async fn mcp_harness(database_url: &str) -> (LocalMcpHttpHarness, AppState) {
    let config = super::common::test_config(database_url.to_string(), PathBuf::from("/tmp"));
    let state = AppState::connect(config.clone())
        .await
        .expect("connect");
    state
        .run_migrations()
        .await
        .expect("migrate");
    let router =
        rust_mcp::http::router(state.clone(), config, super::common::test_prometheus_handle());
    let mcp = LocalMcpHttpHarness::spawn(router)
        .await
        .expect("spawn harness");
    mcp.wait_until_ready(Duration::from_secs(20))
        .await
        .expect("harness ready");
    (mcp, state)
}

async fn stored_state(pool: &PgPool, session_id: &str) -> Option<Value> {
    sqlx::query_scalar("SELECT state FROM sessions WHERE session_id = $1")
        .bind(session_id)
        .fetch_optional(pool)
        .await
        .expect("session state query")
}

async fn session_count(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
        .fetch_one(pool)
        .await
        .expect("session count query")
}

async fn backdate_session(pool: &PgPool, session_id: &str) {
    sqlx::query(
        "UPDATE sessions SET last_seen_at = NOW() - INTERVAL '2 hours' WHERE session_id = $1",
    )
    .bind(session_id)
    .execute(pool)
    .await
    .expect("backdate session");
}

fn session_state(client_name: &str) -> SessionState {
    SessionState::new(InitializeRequestParams::new(
        ClientCapabilities::default(),
        Implementation::new(client_name, "0.1.0"),
    ))
}

#[tokio::test]
async fn legacy_session_persists_initialize_params() {
    let pg = PostgresTestContainer::start()
        .await
        .expect("start postgres");
    let (mcp, state) = mcp_harness(pg.connection_string()).await;

    mcp.initialize()
        .await
        .expect("legacy initialize");
    let session_id = mcp
        .session_id()
        .await
        .expect("legacy initialize should establish a session");

    let stored = stored_state(&state.db, &session_id)
        .await
        .expect("session should be persisted");
    assert_eq!(
        stored["initialize_params"]["clientInfo"]["name"],
        json!("rust-mcp-integration"),
        "persisted state should carry the client's initialize params: {stored}"
    );
}

/// Initializes a session on server 1, drops it, and spins up server 2 on the
/// same database: the old session ID keeps working because rmcp restores it
/// from the store.
#[tokio::test]
async fn legacy_session_restored_after_restart_allows_tool_call() {
    let pg = PostgresTestContainer::start()
        .await
        .expect("start postgres");

    let (mcp1, _state1) = mcp_harness(pg.connection_string()).await;
    mcp1.initialize()
        .await
        .expect("initialize on server 1");
    let session_id = mcp1
        .session_id()
        .await
        .expect("server 1 should have a session ID");
    drop(mcp1);

    let (mcp2, state2) = mcp_harness(pg.connection_string()).await;
    mcp2.client()
        .set_protocol(McpProtocol::Legacy)
        .await;
    mcp2.set_session_id(session_id.clone())
        .await;

    let ping = mcp2
        .call_tool("ping", json!({"message": "after-restart"}))
        .await
        .expect("tool call on restored session should succeed");
    assert!(ping.get("result").is_some(), "ping should return a result");
    assert!(
        stored_state(&state2.db, &session_id)
            .await
            .is_some(),
        "session should still be persisted after restore"
    );
}

/// Concurrent requests carrying a session ID unknown to a freshly started
/// server must all succeed while the session is restored exactly once.
#[tokio::test]
async fn concurrent_requests_restore_a_legacy_session_once() {
    let pg = PostgresTestContainer::start()
        .await
        .expect("start postgres");

    let (mcp1, _state1) = mcp_harness(pg.connection_string()).await;
    mcp1.initialize()
        .await
        .expect("initialize on server 1");
    let session_id = mcp1
        .session_id()
        .await
        .expect("server 1 should have a session ID");
    drop(mcp1);

    let (mcp2, state2) = mcp_harness(pg.connection_string()).await;
    mcp2.client()
        .set_protocol(McpProtocol::Legacy)
        .await;
    mcp2.set_session_id(session_id.clone())
        .await;

    let calls =
        (0..8).map(|i| mcp2.call_tool("ping", json!({ "message": format!("concurrent-{i}") })));
    for result in future::join_all(calls).await {
        result.expect("concurrent call on restored session should succeed");
    }

    let live = state2
        .sessions
        .manager()
        .sessions
        .read()
        .await
        .keys()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    assert_eq!(live, vec![session_id], "the session should be restored exactly once");
}

#[tokio::test]
async fn legacy_session_delete_removes_persisted_state() {
    let pg = PostgresTestContainer::start()
        .await
        .expect("start postgres");
    let (mcp, state) = mcp_harness(pg.connection_string()).await;

    mcp.initialize()
        .await
        .expect("legacy initialize");
    let session_id = mcp
        .session_id()
        .await
        .expect("session ID");

    mcp.client()
        .close_session()
        .await
        .expect("session DELETE");
    assert!(
        stored_state(&state.db, &session_id)
            .await
            .is_none(),
        "DELETE should remove the persisted session"
    );

    mcp.set_session_id(session_id)
        .await;
    assert!(
        mcp.call_tool("ping", json!({}))
            .await
            .is_err(),
        "a closed session must not be restored"
    );
}

#[tokio::test]
async fn prune_removes_stale_sessions_but_keeps_live_ones() {
    let pg = PostgresTestContainer::start()
        .await
        .expect("start postgres");
    let (mcp, state) = mcp_harness(pg.connection_string()).await;

    // A live in-memory session whose row looks idle.
    mcp.initialize()
        .await
        .expect("legacy initialize");
    let live_id = mcp
        .session_id()
        .await
        .expect("session ID");
    backdate_session(&state.db, &live_id).await;

    // A persisted-only session (e.g. from before a restart) that is idle.
    let store = PgSessionStore::new(state.db.clone());
    store
        .store("stale-session", &session_state("stale-client"))
        .await
        .expect("store stale session");
    backdate_session(&state.db, "stale-session").await;

    let pruned = state
        .sessions
        .prune_stale(3600)
        .await
        .expect("prune");
    assert_eq!(pruned, 1, "only the stale persisted-only session should be pruned");
    assert!(
        stored_state(&state.db, "stale-session")
            .await
            .is_none()
    );
    assert!(
        stored_state(&state.db, &live_id)
            .await
            .is_some(),
        "live sessions must survive pruning"
    );
}

#[tokio::test]
async fn session_store_round_trips_state() {
    let pg = PostgresTestContainer::start()
        .await
        .expect("start postgres");
    let (_mcp, state) = mcp_harness(pg.connection_string()).await;
    let store = PgSessionStore::new(state.db.clone());

    assert!(
        store
            .load("missing")
            .await
            .expect("load missing")
            .is_none()
    );

    store
        .store("roundtrip", &session_state("roundtrip-client"))
        .await
        .expect("store");
    let loaded = store
        .load("roundtrip")
        .await
        .expect("load")
        .expect("stored session should load");
    assert_eq!(
        loaded
            .initialize_params
            .client_info
            .name,
        "roundtrip-client"
    );

    store
        .delete("roundtrip")
        .await
        .expect("delete");
    assert!(
        store
            .load("roundtrip")
            .await
            .expect("load deleted")
            .is_none()
    );
}

#[tokio::test]
async fn stateless_requests_do_not_create_sessions() {
    let pg = PostgresTestContainer::start()
        .await
        .expect("start postgres");
    let (mcp, state) = mcp_harness(pg.connection_string()).await;

    mcp.connect()
        .await
        .expect("server/discover");
    mcp.call_tool("ping", json!({"message": "stateless"}))
        .await
        .expect("stateless ping");

    assert!(
        mcp.session_id()
            .await
            .is_none(),
        "stateless requests must not receive a session ID"
    );
    assert_eq!(session_count(&state.db).await, 0, "stateless requests must not persist sessions");
}
