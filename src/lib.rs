//! Local-first Rust dependency intelligence MCP server library.

/// Application startup and runtime orchestration.
pub mod app;
/// Typed runtime configuration loaded from CLI flags and environment variables.
pub mod config;
/// HTTP-layer error types and shared API result alias.
pub mod error;
/// Axum HTTP router and health/readiness handlers.
pub mod http;
/// Tracing/logging initialization.
pub mod logging;
/// MCP transport/service integration.
pub mod mcp;
/// Shared state (database + HTTP client + config).
pub mod state;
