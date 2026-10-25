//! Legacy-protocol MCP session persistence.
//!
//! Sessions exist only for legacy (`initialize`-based) protocol versions;
//! stateless requests never touch this module. rmcp's [`LocalSessionManager`]
//! holds live sessions in memory, and rmcp transparently restores unknown
//! session IDs (after a restart or in-memory idle expiry) from
//! [`PgSessionStore`] by replaying the client's original `initialize`.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::session::store::{
    SessionState, SessionStore, SessionStoreError,
};
use sqlx::PgPool;
use tracing::{debug, info, warn};

use crate::db::sessions as db;
use crate::state::AppState;

/// PostgreSQL-backed [`SessionStore`].
#[derive(Debug, Clone)]
pub struct PgSessionStore {
    db: PgPool,
}

impl PgSessionStore {
    /// Creates a store backed by the given pool.
    pub fn new(db: PgPool) -> Self {
        Self { db }
    }
}

#[async_trait::async_trait]
impl SessionStore for PgSessionStore {
    async fn load(&self, session_id: &str) -> Result<Option<SessionState>, SessionStoreError> {
        let Some(state) = db::load_session(&self.db, session_id).await? else {
            return Ok(None);
        };
        Ok(Some(serde_json::from_value(state)?))
    }

    async fn store(&self, session_id: &str, state: &SessionState) -> Result<(), SessionStoreError> {
        db::upsert_session(&self.db, session_id, &serde_json::to_value(state)?).await?;
        Ok(())
    }

    async fn delete(&self, session_id: &str) -> Result<(), SessionStoreError> {
        db::delete_session(&self.db, session_id).await?;
        Ok(())
    }
}

/// Live legacy sessions plus their durable store.
#[derive(Clone)]
pub struct McpSessions {
    manager: Arc<LocalSessionManager>,
    store: Arc<PgSessionStore>,
}

impl McpSessions {
    /// Creates an empty in-memory session manager backed by `db`.
    pub fn new(db: PgPool) -> Self {
        Self {
            manager: Arc::new(LocalSessionManager::default()),
            store: Arc::new(PgSessionStore::new(db)),
        }
    }

    /// The in-memory session manager serving live sessions.
    pub fn manager(&self) -> Arc<LocalSessionManager> {
        self.manager.clone()
    }

    /// The durable store used to restore sessions.
    pub fn store(&self) -> Arc<dyn SessionStore> {
        self.store.clone()
    }

    /// Prunes persisted sessions idle for longer than `max_idle_secs`.
    ///
    /// Sessions live in memory are marked as seen first: rmcp only consults
    /// the store on create/restore/delete, so a continuously active session
    /// would otherwise look idle.
    pub async fn prune_stale(&self, max_idle_secs: i64) -> Result<u64, sqlx::Error> {
        let live: Vec<String> = self
            .manager
            .sessions
            .read()
            .await
            .keys()
            .map(ToString::to_string)
            .collect();
        if !live.is_empty() {
            db::touch_sessions(&self.store.db, &live).await?;
        }
        db::delete_stale_sessions(&self.store.db, max_idle_secs).await
    }
}

impl fmt::Debug for McpSessions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("McpSessions")
            .field("store", &self.store)
            .finish_non_exhaustive()
    }
}

/// Periodically prunes idle sessions from the database.
///
/// Runs every `timeout / 2` seconds (but at least every 60 s). Disabled when
/// `session_idle_timeout_secs` is 0.
pub async fn run_session_reaper(state: AppState) {
    let timeout_secs = state
        .config
        .session_idle_timeout_secs;
    if timeout_secs == 0 {
        debug!("session reaper disabled (SESSION_IDLE_TIMEOUT_SECS=0)");
        return;
    }

    let poll_interval = Duration::from_secs((timeout_secs / 2).max(60));
    info!(timeout_secs, poll_interval_secs = poll_interval.as_secs(), "session reaper started");

    loop {
        tokio::time::sleep(poll_interval).await;

        match state
            .sessions
            .prune_stale(timeout_secs as i64)
            .await
        {
            Ok(0) => {}
            Ok(pruned) => info!(pruned, "pruned stale sessions"),
            Err(err) => warn!(%err, "session reaper: failed to delete stale sessions"),
        }
    }
}
