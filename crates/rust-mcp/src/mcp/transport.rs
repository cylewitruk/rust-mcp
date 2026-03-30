use std::sync::Arc;
use std::time::Duration;

use rmcp::transport::streamable_http_server::session::local::SessionConfig;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};

use super::server::McpServer;
use super::session::DbSessionManager;
use crate::config::Config;
use crate::state::AppState;

/// Creates the RMCP streamable HTTP service using the configured SSE settings.
pub fn streamable_http_service(
    state: AppState,
    config: &Config,
) -> StreamableHttpService<McpServer, DbSessionManager> {
    let service_state = state.clone();
    let mut http_config = StreamableHttpServerConfig::default();
    http_config.stateful_mode = true;

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

    let service_factory: super::session::ServiceFactory =
        Arc::new(move || Ok(McpServer::new(service_state.clone())));
    let session_manager = Arc::new(DbSessionManager::new(
        state.db.clone(),
        SessionConfig::default(),
        service_factory.clone(),
    ));

    StreamableHttpService::new(move || (service_factory)(), session_manager, http_config)
}
