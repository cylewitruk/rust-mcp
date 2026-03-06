//! Database-backed MCP session manager.
//!
//! Wraps `rmcp`'s [`LocalSessionManager`] with a PostgreSQL backing store so
//! that session IDs survive server restarts. When a client presents a session
//! ID that exists in the database but has no in-memory worker, the manager
//! lazily reconstitutes the session instead of returning "Session not found".

use std::time::Duration;

use futures::Stream;
use rmcp::model::{ClientJsonRpcMessage, ServerJsonRpcMessage};
use rmcp::transport::WorkerTransport;
use rmcp::transport::streamable_http_server::session::local::{
    LocalSessionManager, LocalSessionManagerError, LocalSessionWorker, SessionConfig,
    create_local_session,
};
use rmcp::transport::streamable_http_server::session::{
    ServerSseMessage, SessionId, SessionManager,
};
use sqlx::PgPool;
use thiserror::Error;
use tracing::{debug, info, warn};

use crate::db::sessions as db;
use crate::state::AppState;

/// A session manager backed by PostgreSQL.
///
/// Session IDs are persisted to the `sessions` table. On every request the
/// manager checks the in-memory map first; if the session is missing but
/// exists in the database it is reconstituted transparently, allowing clients
/// to survive server restarts without re-initializing.
pub struct DbSessionManager {
    db: PgPool,
    /// The inner local manager handles all in-memory session state.
    inner: LocalSessionManager,
}

#[derive(Debug, Error)]
/// Errors produced by [`DbSessionManager`].
pub enum DbSessionManagerError {
    /// Forwarded from the in-memory local session manager.
    #[error(transparent)]
    Local(#[from] LocalSessionManagerError),
    /// Database query failed.
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),
}

impl DbSessionManager {
    /// Creates a new database-backed session manager.
    pub fn new(db: PgPool, session_config: SessionConfig) -> Self {
        Self {
            db,
            inner: LocalSessionManager {
                sessions: Default::default(),
                session_config,
            },
        }
    }

    /// If the session exists in the DB but not in memory, reconstitute it
    /// by creating a fresh in-memory worker and inserting it into the inner
    /// manager's session map.
    ///
    /// Returns `true` if the session was reconstituted, `false` if it was
    /// already in memory, and an error if it doesn't exist anywhere.
    async fn ensure_session(&self, id: &SessionId) -> Result<bool, DbSessionManagerError> {
        // Fast path: already in memory.
        if self
            .inner
            .sessions
            .read()
            .await
            .contains_key(id)
        {
            return Ok(false);
        }

        // Check the database.
        if !db::session_exists(&self.db, id.as_ref()).await? {
            return Err(LocalSessionManagerError::SessionNotFound(id.clone()).into());
        }

        // Reconstitute: create a fresh in-memory worker for this session ID.
        info!(session_id = %id, "reconstituting session from database");
        let (handle, worker) = create_local_session(
            id.clone(),
            self.inner
                .session_config
                .clone(),
        );
        self.inner
            .sessions
            .write()
            .await
            .insert(id.clone(), handle);

        // The worker must be spawned so the transport is driven.
        // We spawn it in a background task that keeps the transport alive
        // until the session handle is dropped (on close_session).
        tokio::spawn(async move {
            let _transport = WorkerTransport::spawn(worker);
            // Keep the transport alive indefinitely. When the session
            // handle is dropped the worker's event_rx channel closes,
            // which naturally shuts down the worker.
            std::future::pending::<()>().await;
        });

        // Touch last_seen_at.
        let _ = db::touch_session(&self.db, id.as_ref()).await;

        Ok(true)
    }
}

impl SessionManager for DbSessionManager {
    type Error = DbSessionManagerError;
    type Transport = WorkerTransport<LocalSessionWorker>;

    async fn create_session(&self) -> Result<(SessionId, Self::Transport), Self::Error> {
        let (id, transport) = self
            .inner
            .create_session()
            .await?;

        // Persist to database.
        db::insert_session(&self.db, id.as_ref()).await?;

        info!(session_id = %id, "created new session (persisted to DB)");
        Ok((id, transport))
    }

    async fn initialize_session(
        &self,
        id: &SessionId,
        message: ClientJsonRpcMessage,
    ) -> Result<ServerJsonRpcMessage, Self::Error> {
        self.ensure_session(id)
            .await?;
        Ok(self
            .inner
            .initialize_session(id, message)
            .await?)
    }

    async fn has_session(&self, id: &SessionId) -> Result<bool, Self::Error> {
        // Check in-memory first.
        if self
            .inner
            .has_session(id)
            .await?
        {
            return Ok(true);
        }

        // Fall back to database.
        Ok(db::session_exists(&self.db, id.as_ref()).await?)
    }

    async fn close_session(&self, id: &SessionId) -> Result<(), Self::Error> {
        // Remove from in-memory.
        let _ = self
            .inner
            .close_session(id)
            .await;

        // Remove from database.
        db::delete_session(&self.db, id.as_ref()).await?;

        info!(session_id = %id, "closed session (removed from DB)");
        Ok(())
    }

    async fn create_stream(
        &self,
        id: &SessionId,
        message: ClientJsonRpcMessage,
    ) -> Result<impl Stream<Item = ServerSseMessage> + Send + 'static, Self::Error> {
        self.ensure_session(id)
            .await?;
        Ok(self
            .inner
            .create_stream(id, message)
            .await?)
    }

    async fn create_standalone_stream(
        &self,
        id: &SessionId,
    ) -> Result<impl Stream<Item = ServerSseMessage> + Send + 'static, Self::Error> {
        self.ensure_session(id)
            .await?;
        Ok(self
            .inner
            .create_standalone_stream(id)
            .await?)
    }

    async fn resume(
        &self,
        id: &SessionId,
        last_event_id: String,
    ) -> Result<impl Stream<Item = ServerSseMessage> + Send + 'static, Self::Error> {
        self.ensure_session(id)
            .await?;
        Ok(self
            .inner
            .resume(id, last_event_id)
            .await?)
    }

    async fn accept_message(
        &self,
        id: &SessionId,
        message: ClientJsonRpcMessage,
    ) -> Result<(), Self::Error> {
        self.ensure_session(id)
            .await?;
        Ok(self
            .inner
            .accept_message(id, message)
            .await?)
    }
}

/// Periodically prunes idle sessions from the database.
///
/// Runs every `timeout / 2` seconds (but at least every 60 s). Sessions
/// whose `last_seen_at` is older than `session_idle_timeout_secs` are
/// deleted. Disabled when the timeout is 0.
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

        match db::delete_stale_sessions(&state.db, timeout_secs as i64).await {
            Ok(0) => {}
            Ok(pruned) => info!(pruned, "pruned stale sessions"),
            Err(err) => warn!(%err, "session reaper: failed to delete stale sessions"),
        }
    }
}

/// Runs a single reaper pass and returns the number of pruned sessions.
#[cfg(feature = "testing")]
pub async fn prune_stale_sessions(db: &PgPool, max_age_secs: i64) -> Result<u64, sqlx::Error> {
    db::delete_stale_sessions(db, max_age_secs).await
}
