use std::time::Duration;

use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};

use super::server::McpServer;
use crate::config::Config;
use crate::state::AppState;

/// Creates the RMCP streamable HTTP service using the configured SSE settings.
///
/// Requests on the stateless protocol are served per-request; legacy
/// (`initialize`-based) clients get sessions persisted via `state.sessions`.
pub fn streamable_http_service(
    state: AppState,
    config: &Config,
) -> StreamableHttpService<McpServer, LocalSessionManager> {
    let mut http_config = StreamableHttpServerConfig::default();
    http_config.legacy_session_mode = true;
    http_config.session_store = Some(state.sessions.store());

    if config.mcp_sse_keep_alive_secs == 0 {
        http_config.sse_keep_alive = None;
    } else {
        http_config.sse_keep_alive = Some(Duration::from_secs(config.mcp_sse_keep_alive_secs));
    }

    if config.mcp_sse_retry_ms == 0 {
        http_config.sse_retry = None;
    } else {
        http_config.sse_retry = Some(Duration::from_millis(config.mcp_sse_retry_ms));
    }

    let session_manager = state.sessions.manager();
    StreamableHttpService::new(
        move || Ok(McpServer::new(state.clone())),
        session_manager,
        http_config,
    )
}
