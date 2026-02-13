use axum::{Json, Router, extract::State, http::StatusCode, response::IntoResponse, routing::get};
use serde::Serialize;
use tower_http::trace::TraceLayer;
use tracing::warn;

use crate::{
    config::{Config, TransportMode},
    error::ApiResult,
    mcp,
    state::AppState,
};

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
    state.readiness_check().await?;
    Ok((StatusCode::OK, Json(StatusPayload { status: "ready" })))
}
