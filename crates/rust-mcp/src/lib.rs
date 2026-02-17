//! Local-first Rust dependency intelligence MCP server library.

/// Application startup and runtime orchestration.
pub mod app;
/// Typed runtime configuration loaded from CLI flags and environment variables.
pub mod config;
/// Tool-schema contract assembly and artifact export helpers.
mod contracts;
/// Database models and access layer.
pub mod db;
/// HTTP-layer error types and shared API result alias.
pub mod error;
/// Axum HTTP router and health/readiness handlers.
pub mod http;
/// External integration request/response models and validation logic.
#[cfg(feature = "testing")]
pub mod integration;
/// External integration request/response models and validation logic.
#[cfg(not(feature = "testing"))]
mod integration;
/// Tracing/logging initialization.
pub mod logging;
/// MCP transport/service integration.
pub mod mcp;
/// Shared state (database + HTTP client + config).
pub mod state;
