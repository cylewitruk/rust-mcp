use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::http::header::{
    ACCEPT, ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS,
    ACCESS_CONTROL_ALLOW_ORIGIN, CACHE_CONTROL, CONTENT_TYPE, WARNING,
};
use axum::middleware::{self, Next};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use metrics_exporter_prometheus::PrometheusHandle;
use rmcp::model::ProtocolVersion;
use serde::Serialize;
use tower::limit::ConcurrencyLimitLayer;
use tower_http::trace::TraceLayer;

use crate::config::Config;
use crate::error::ApiResult;
use crate::mcp;
use crate::state::AppState;

/// Builds the HTTP router with health/readiness endpoints and MCP transport
/// mounting.
pub fn router(state: AppState, config: Config, prometheus_handle: PrometheusHandle) -> Router {
    let mcp_service = mcp::streamable_http_service(state.clone(), &config);
    let strict_accept = config.mcp_strict_accept;
    let mcp_router = Router::new()
        .route_service("/", mcp_service)
        .layer(middleware::from_fn_with_state(strict_accept, relax_mcp_accept_header))
        .layer(middleware::from_fn(rewrite_mcp_protocol_version));

    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics))
        .route("/schemas", get(list_tool_schemas))
        .route("/schemas/{tool_name}", get(get_tool_schema))
        .route("/.well-known/mcp.json", get(mcp_server_card))
        .route("/.well-known/mcp/server-card.json", get(mcp_server_card))
        .route("/.well-known/{*path}", get(oauth_not_supported))
        .nest("/mcp", mcp_router)
        .with_state(state)
        .layer(axum::Extension(prometheus_handle))
        .layer(ConcurrencyLimitLayer::new(
            config
                .max_concurrent_requests
                .max(1) as usize,
        ))
        .layer(TraceLayer::new_for_http())
}

/// Rewrites MCP-Protocol-Version headers that rmcp doesn't recognize.
///
/// rmcp's `ProtocolVersion::KNOWN_VERSIONS` may lag behind the latest MCP spec,
/// so clients sending newer versions (e.g. Claude Code with `2025-11-25`) get a
/// 400 before our code runs. This middleware maps unknown versions to rmcp's
/// `ProtocolVersion::LATEST`. The actual protocol negotiation still happens at
/// the JSON-RPC `initialize` level, where we report our true
/// `SUPPORTED_MCP_PROTOCOL_VERSION`.
async fn rewrite_mcp_protocol_version(
    mut request: axum::http::Request<Body>,
    next: Next,
) -> impl IntoResponse {
    const HEADER_NAME: &str = "mcp-protocol-version";

    let needs_rewrite = request
        .headers()
        .get(HEADER_NAME)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            !ProtocolVersion::KNOWN_VERSIONS
                .iter()
                .any(|known| known.as_str() == v)
        });

    if needs_rewrite
        && let Ok(value) = axum::http::HeaderValue::from_str(ProtocolVersion::LATEST.as_str())
    {
        request
            .headers_mut()
            .insert(axum::http::HeaderName::from_static(HEADER_NAME), value);
    }

    next.run(request).await
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

/// MCP Server Card per SEP-1649.
///
/// Served at both `/.well-known/mcp.json` (common convention) and
/// `/.well-known/mcp/server-card.json` (SEP-1649 draft path).
async fn mcp_server_card() -> impl IntoResponse {
    let body = serde_json::json!({
        "$schema": "https://static.modelcontextprotocol.io/schemas/mcp-server-card/v1.json",
        "version": "1.0",
        "protocolVersion": rust_mcp_types::protocol::SUPPORTED_MCP_PROTOCOL_VERSION,
        "serverInfo": {
            "name": "rust-mcp",
            "title": "Rust MCP",
            "version": env!("CARGO_PKG_VERSION"),
        },
        "description": "Local-first Rust dependency intelligence server",
        "transport": {
            "type": "streamable-http",
            "endpoint": "/mcp",
        },
        "capabilities": {
            "tools": { "listChanged": false },
        },
        "authentication": {
            "required": false,
        },
        "tools": rust_mcp_types::tools::ALL_TOOLS,
    });

    (
        [
            (ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
            (ACCESS_CONTROL_ALLOW_METHODS, "GET"),
            (ACCESS_CONTROL_ALLOW_HEADERS, "Content-Type"),
            (CACHE_CONTROL, "public, max-age=3600"),
            (CONTENT_TYPE, "application/json"),
        ],
        Json(body),
    )
}

async fn oauth_not_supported() -> impl IntoResponse {
    (
        StatusCode::NOT_FOUND,
        Json(ErrorPayload {
            error: "OAuth is not supported by this server".to_string(),
        }),
    )
}

async fn healthz() -> impl IntoResponse {
    (StatusCode::OK, Json(StatusPayload { status: "ok" }))
}

async fn metrics(axum::Extension(handle): axum::Extension<PrometheusHandle>) -> impl IntoResponse {
    handle.render()
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
