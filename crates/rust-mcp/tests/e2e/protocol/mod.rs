//! E2E protocol and transport behavior tests for MCP over HTTP.

use std::collections::BTreeSet;

use axum::http::StatusCode;
use serde_json::{Value, json};

use super::crates_io_fixtures::{MockRegistryServer, SEEDED_CRATE_NAME, mock_registry_env};
use super::helpers::{
    DEFAULT_TEST_PROTOCOL_VERSION, initialize_params, initialize_raw_session, jsonrpc_notification,
    jsonrpc_request, notify_initialized, parse_mcp_response, parse_mcp_response_events_from_http,
    parse_mcp_response_from_http, post_mcp_jsonrpc, start_ready_rust_mcp,
    start_ready_rust_mcp_with_env,
};

mod basics;
mod conformance;
mod errors;
mod progress;
mod session;
mod transport;
