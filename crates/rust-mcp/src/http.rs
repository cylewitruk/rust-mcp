use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use tower::limit::ConcurrencyLimitLayer;
use tower_http::trace::TraceLayer;

use crate::config::Config;
use crate::error::ApiResult;
use crate::mcp;
use crate::state::AppState;

/// Builds the HTTP router with health/readiness endpoints and MCP transport
/// mounting.
pub fn router(state: AppState, config: Config) -> Router {
    let mcp_service = mcp::streamable_http_service(state.clone(), &config);

    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/schemas", get(list_tool_schemas))
        .route("/schemas/{tool_name}", get(get_tool_schema))
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

#[derive(Debug, Serialize)]
struct ErrorPayload {
    error: String,
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

async fn list_tool_schemas() -> impl IntoResponse {
    match crate::contracts::tool_schemas_response(None) {
        Ok(payload) => (StatusCode::OK, Json(payload)).into_response(),
        Err(error) => {
            (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorPayload { error })).into_response()
        }
    }
}

async fn get_tool_schema(Path(tool_name): Path<String>) -> impl IntoResponse {
    match crate::contracts::tool_schemas_response(Some(tool_name)) {
        Ok(payload) => (StatusCode::OK, Json(payload)).into_response(),
        Err(error) => (StatusCode::NOT_FOUND, Json(ErrorPayload { error })).into_response(),
    }
}
