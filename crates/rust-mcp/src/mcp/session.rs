//! Database-backed MCP session manager with session reconstitution.
//!
//! Wraps `rmcp`'s session primitives with a PostgreSQL backing store so that
//! session IDs are persisted across the server's lifetime. When a client
//! presents a session ID that exists in the database but has no in-memory
//! worker (e.g. after a server restart), the session is **reconstituted**: a
//! fresh worker + handler pair is created and primed with a synthetic
//! `initialize` handshake, allowing the client to resume transparently.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures::Stream;
use rmcp::model::{
    ClientCapabilities, ClientJsonRpcMessage, ClientNotification, ClientRequest, Implementation,
    InitializeRequestParams, InitializedNotification, NumberOrString, ProtocolVersion,
    ServerJsonRpcMessage,
};
use rmcp::serve_server;
use rmcp::transport::WorkerTransport;
use rmcp::transport::streamable_http_server::session::local::{
    LocalSessionHandle, LocalSessionManagerError, LocalSessionWorker, SessionConfig, SessionError,
    create_local_session,
};
use rmcp::transport::streamable_http_server::session::{
    ServerSseMessage, SessionId, SessionManager,
};
use sqlx::PgPool;
use thiserror::Error;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, info, warn};

use super::server::McpServer;
use crate::db::sessions as db;
use crate::state::AppState;

/// Service factory type for creating new [`McpServer`] instances.
pub type ServiceFactory = Arc<dyn Fn() -> Result<McpServer, std::io::Error> + Send + Sync>;

/// A session manager backed by PostgreSQL with session reconstitution.
///
/// Sessions that exist only in the database (stale after a server restart) are
/// transparently reconstituted: a new worker + handler pair is spun up and
/// primed with a synthetic `initialize` handshake so the client can resume
/// without re-initializing.
pub struct DbSessionManager {
    db: PgPool,
    sessions: Arc<RwLock<HashMap<SessionId, LocalSessionHandle>>>,
    session_config: SessionConfig,
    service_factory: ServiceFactory,
    /// Serializes reconstitution attempts to prevent double-reconstituting the
    /// same session from concurrent requests.
    reconstitution_lock: Mutex<()>,
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
    /// Session reconstitution failed.
    #[error("Session reconstitution failed: {0}")]
    Reconstitution(String),
    /// Session-level error (e.g. channel closed).
    #[error("Session error: {0}")]
    Session(#[from] SessionError),
}

impl DbSessionManager {
    /// Creates a new database-backed session manager.
    pub fn new(db: PgPool, session_config: SessionConfig, service_factory: ServiceFactory) -> Self {
        Self {
            db,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            session_config,
            service_factory,
            reconstitution_lock: Mutex::new(()),
        }
    }

    /// Ensures a session is live in memory, reconstituting from the database if
    /// necessary.
    ///
    /// Returns `true` if the session is (now) live, `false` if it doesn't exist
    /// anywhere.
    async fn ensure_session_live(&self, id: &SessionId) -> Result<bool, DbSessionManagerError> {
        // Fast path: session is already in memory.
        if self
            .sessions
            .read()
            .await
            .contains_key(id)
        {
            return Ok(true);
        }

        // Slow path: check DB and reconstitute if needed.
        let _guard = self
            .reconstitution_lock
            .lock()
            .await;

        // Double-check after acquiring the lock — another request may have
        // reconstituted it while we were waiting.
        if self
            .sessions
            .read()
            .await
            .contains_key(id)
        {
            return Ok(true);
        }

        // Check DB.
        if !db::session_exists(&self.db, id.as_ref()).await? {
            return Ok(false);
        }

        // Session exists in DB but not in memory — reconstitute it.
        info!(session_id = %id, "reconstituting stale session from database");
        self.reconstitute_session(id)
            .await?;
        Ok(true)
    }

    /// Reconstitutes a session that exists in the database but has no in-memory
    /// worker. Creates a new worker + handler pair, primes it with a synthetic
    /// `initialize` handshake, and registers it in the sessions map.
    async fn reconstitute_session(&self, id: &SessionId) -> Result<(), DbSessionManagerError> {
        // Create a new worker + handle pair reusing the old session ID.
        let (handle, worker) = create_local_session(id.clone(), self.session_config.clone());

        // Create a new MCP server service instance.
        let service = (self.service_factory)()
            .map_err(|e| DbSessionManagerError::Reconstitution(format!("service factory: {e}")))?;

        // Spawn the handler, connecting it to the worker via the transport.
        // `serve_server` expects InitializeRequest as the first message from
        // the transport, which we'll feed via the synthetic init below.
        let db = self.db.clone();
        let sessions = self.sessions.clone();
        let session_id = id.clone();
        tokio::spawn(async move {
            match serve_server(service, worker).await {
                Ok(running) => {
                    let _ = running.waiting().await;
                }
                Err(e) => {
                    warn!(
                        session_id = %session_id,
                        error = %e,
                        "reconstituted session handler failed"
                    );
                }
            }
            // Cleanup when the service exits.
            let mut sessions = sessions.write().await;
            if let Some(handle) = sessions.remove(&session_id) {
                let _ = handle.close().await;
            }
            let _ = db::delete_session(&db, session_id.as_ref()).await;
            debug!(session_id = %session_id, "reconstituted session cleaned up");
        });

        // Build and send the synthetic initialize request.
        let init_params = InitializeRequestParams::new(
            ClientCapabilities::default(),
            Implementation::new("rust-mcp-session-reconstitution", "internal"),
        )
        .with_protocol_version(ProtocolVersion::LATEST);
        let init_request = rmcp::model::InitializeRequest::new(init_params);
        let init_msg = ClientJsonRpcMessage::request(
            ClientRequest::InitializeRequest(init_request),
            NumberOrString::Number(0),
        );
        let _init_response = handle
            .initialize(init_msg)
            .await
            .map_err(|e| {
                DbSessionManagerError::Reconstitution(format!("synthetic initialize: {e}"))
            })?;

        // Send the required `notifications/initialized` notification.
        let notification = ClientJsonRpcMessage::notification(
            ClientNotification::InitializedNotification(InitializedNotification {
                method: Default::default(),
                extensions: Default::default(),
            }),
        );
        handle
            .push_message(notification, None)
            .await
            .map_err(|e| {
                DbSessionManagerError::Reconstitution(format!(
                    "synthetic initialized notification: {e}"
                ))
            })?;

        // Register the handle so future requests hit the fast path.
        self.sessions
            .write()
            .await
            .insert(id.clone(), handle);

        info!(session_id = %id, "session reconstituted successfully");
        Ok(())
    }

    /// Returns a reference to the handle for a live session, or an error if
    /// the session doesn't exist in memory.
    async fn get_handle(
        &self,
        id: &SessionId,
    ) -> Result<impl std::ops::Deref<Target = LocalSessionHandle> + '_, DbSessionManagerError> {
        // Use a closure to map the RwLockReadGuard — we need the guard to live
        // long enough.
        let sessions = self.sessions.read().await;
        if sessions.contains_key(id) {
            Ok(tokio::sync::RwLockReadGuard::map(sessions, |s| s.get(id).unwrap()))
        } else {
            Err(LocalSessionManagerError::SessionNotFound(id.clone()).into())
        }
    }
}

impl SessionManager for DbSessionManager {
    type Error = DbSessionManagerError;
    type Transport = WorkerTransport<LocalSessionWorker>;

    async fn create_session(&self) -> Result<(SessionId, Self::Transport), Self::Error> {
        // Generate a unique session ID using std randomness.
        use std::collections::hash_map::RandomState;
        use std::hash::{BuildHasher as _, Hasher as _};
        let s = RandomState::new();
        let a = s.build_hasher().finish();
        let b = RandomState::new()
            .build_hasher()
            .finish();
        let id: SessionId = format!("{a:016x}{b:016x}").into();
        let (handle, worker) = create_local_session(id.clone(), self.session_config.clone());
        self.sessions
            .write()
            .await
            .insert(id.clone(), handle);

        // Persist to database.
        db::insert_session(&self.db, id.as_ref()).await?;

        info!(session_id = %id, "created new session (persisted to DB)");
        Ok((id, WorkerTransport::spawn(worker)))
    }

    async fn initialize_session(
        &self,
        id: &SessionId,
        message: ClientJsonRpcMessage,
    ) -> Result<ServerJsonRpcMessage, Self::Error> {
        let handle = self.get_handle(id).await?;
        let response = handle
            .initialize(message)
            .await?;
        Ok(response)
    }

    async fn has_session(&self, id: &SessionId) -> Result<bool, Self::Error> {
        self.ensure_session_live(id)
            .await
    }

    async fn close_session(&self, id: &SessionId) -> Result<(), Self::Error> {
        // Remove from in-memory.
        if let Some(handle) = self
            .sessions
            .write()
            .await
            .remove(id)
        {
            let _ = handle.close().await;
        }

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
        self.ensure_session_live(id)
            .await?;
        let handle = self.get_handle(id).await?;
        let receiver = handle
            .establish_request_wise_channel()
            .await?;
        handle
            .push_message(message, receiver.http_request_id)
            .await?;
        Ok(receiver_stream(receiver.inner))
    }

    async fn create_standalone_stream(
        &self,
        id: &SessionId,
    ) -> Result<impl Stream<Item = ServerSseMessage> + Send + 'static, Self::Error> {
        self.ensure_session_live(id)
            .await?;
        let handle = self.get_handle(id).await?;
        let receiver = handle
            .establish_common_channel()
            .await?;
        Ok(receiver_stream(receiver.inner))
    }

    async fn resume(
        &self,
        id: &SessionId,
        last_event_id: String,
    ) -> Result<impl Stream<Item = ServerSseMessage> + Send + 'static, Self::Error> {
        self.ensure_session_live(id)
            .await?;
        let handle = self.get_handle(id).await?;
        let last_event_id: rmcp::transport::streamable_http_server::session::local::EventId =
            last_event_id
                .parse()
                .map_err(LocalSessionManagerError::from)?;
        let receiver = handle
            .resume(last_event_id)
            .await?;
        Ok(receiver_stream(receiver.inner))
    }

    async fn accept_message(
        &self,
        id: &SessionId,
        message: ClientJsonRpcMessage,
    ) -> Result<(), Self::Error> {
        self.ensure_session_live(id)
            .await?;
        let handle = self.get_handle(id).await?;
        handle
            .push_message(message, None)
            .await?;
        Ok(())
    }
}

/// Wraps a `tokio::sync::mpsc::Receiver` into a `futures::Stream`.
fn receiver_stream(
    rx: tokio::sync::mpsc::Receiver<ServerSseMessage>,
) -> impl Stream<Item = ServerSseMessage> + Send + 'static {
    futures::stream::unfold(rx, |mut rx| async move {
        let item = rx.recv().await?;
        Some((item, rx))
    })
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
