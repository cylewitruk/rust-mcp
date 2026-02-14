use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use tower::limit::ConcurrencyLimitLayer;
use tower_http::trace::TraceLayer;
use tracing::warn;

use crate::config::{Config, TransportMode};
use crate::error::ApiResult;
use crate::mcp;
use crate::state::AppState;

/// Builds the HTTP router with health/readiness endpoints and MCP transport
/// mounting.
pub fn router(state: AppState, config: Config) -> Router {
    if matches!(config.mcp_transport, TransportMode::Stdio) {
        warn!("MCP_TRANSPORT=stdio set; HTTP endpoints stay available for health/readiness checks");
    }

    let mcp_service = mcp::streamable_http_service(state.clone(), &config);

    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .nest_service("/mcp", mcp_service)
        .with_state(state)
        .layer(ConcurrencyLimitLayer::new(
            config
                .max_concurrent_requests
                .max(1) as usize,
        ))
        .layer(TraceLayer::new_for_http())
}

#[derive(Debug, Serialize)]
struct StatusPayload<'a> {
    status: &'a str,
}

async fn healthz() -> impl IntoResponse {
    (StatusCode::OK, Json(StatusPayload { status: "ok" }))
}

async fn readyz(State(state): State<AppState>) -> ApiResult<impl IntoResponse> {
    state
        .readiness_check()
        .await?;
    Ok((StatusCode::OK, Json(StatusPayload { status: "ready" })))
}
