use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::http::header::{ACCEPT, WARNING};
use axum::middleware::{self, Next};
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
    let strict_accept = config.mcp_strict_accept;
    let mcp_router = Router::new()
        .route_service("/", mcp_service)
        .layer(middleware::from_fn_with_state(strict_accept, relax_mcp_accept_header));

    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/schemas", get(list_tool_schemas))
        .route("/schemas/{tool_name}", get(get_tool_schema))
        .nest("/mcp", mcp_router)
        .with_state(state)
        .layer(ConcurrencyLimitLayer::new(
            config
                .max_concurrent_requests
                .max(1) as usize,
        ))
        .layer(TraceLayer::new_for_http())
}

async fn relax_mcp_accept_header(
    State(strict_accept): State<bool>,
    mut request: axum::http::Request<Body>,
    next: Next,
) -> impl IntoResponse {
    let should_rewrite = request
        .headers()
        .get(ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|accept| {
            accept.contains("application/json") && !accept.contains("text/event-stream")
        });

    if should_rewrite && strict_accept {
        return (
            StatusCode::NOT_ACCEPTABLE,
            "Not Acceptable: Client must accept both application/json and text/event-stream",
        )
            .into_response();
    }

    if should_rewrite {
        request.headers_mut().insert(
            ACCEPT,
            axum::http::HeaderValue::from_static("application/json, text/event-stream"),
        );
    }

    let mut response = next.run(request).await;
    if should_rewrite {
        response.headers_mut().insert(
            WARNING,
            axum::http::HeaderValue::from_static(
                "199 rust-mcp \"Non-conformant Accept header rewritten; expected application/json \
                 and text/event-stream\"",
            ),
        );
    }
    response
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
