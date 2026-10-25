//! Minimal MCP Streamable HTTP JSON-RPC client shared by the test harnesses.

use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context as _, Result, bail, ensure};
use rust_mcp_types::protocol::{
    LATEST_MCP_PROTOCOL_VERSION, LEGACY_MCP_PROTOCOL_VERSION, MCP_SESSION_ID_HEADER,
    insert_stateless_request_meta, stateless_request_headers,
};
use serde_json::{Value, json};
use tokio::sync::Mutex;

const CLIENT_VERSION: &str = "0.1.0";

/// Protocol flavour a [`McpHttpClient`] speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpProtocol {
    /// [`LATEST_MCP_PROTOCOL_VERSION`]: per-request `_meta` and standard
    /// headers, no `initialize` and no session.
    Stateless,
    /// Session-based protocol: `initialize` handshake plus `Mcp-Session-Id`.
    Legacy,
}

/// MCP JSON-RPC client over Streamable HTTP.
///
/// Defaults to [`McpProtocol::Stateless`]; [`McpHttpClient::initialize`]
/// switches to [`McpProtocol::Legacy`].
#[derive(Debug)]
pub struct McpHttpClient {
    http: reqwest::Client,
    mcp_url: String,
    client_name: String,
    protocol: Mutex<McpProtocol>,
    next_request_id: AtomicU64,
    session_id: Mutex<Option<String>>,
}

impl McpHttpClient {
    /// Creates a stateless client for `mcp_url`, identifying as `client_name`.
    pub fn new(mcp_url: impl Into<String>, client_name: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            mcp_url: mcp_url.into(),
            client_name: client_name.into(),
            protocol: Mutex::new(McpProtocol::Stateless),
            next_request_id: AtomicU64::new(1),
            session_id: Mutex::new(None),
        }
    }

    /// Returns the protocol flavour used for subsequent requests.
    pub async fn protocol(&self) -> McpProtocol {
        *self.protocol.lock().await
    }

    /// Sets the protocol flavour used for subsequent requests.
    pub async fn set_protocol(&self, protocol: McpProtocol) {
        *self.protocol.lock().await = protocol;
    }

    /// Returns the current session ID, if one has been established.
    pub async fn session_id(&self) -> Option<String> {
        self.session_id
            .lock()
            .await
            .clone()
    }

    /// Overrides the session ID used for subsequent requests.
    pub async fn set_session_id(&self, session_id: String) {
        *self.session_id.lock().await = Some(session_id);
    }

    /// Clears the locally cached session ID.
    pub async fn clear_session(&self) {
        *self.session_id.lock().await = None;
    }

    /// Prepares the client for tool calls in its current protocol:
    /// `server/discover` when stateless, the full `initialize` handshake
    /// when legacy.
    pub async fn connect(&self) -> Result<Value> {
        match self.protocol().await {
            McpProtocol::Stateless => self.discover().await,
            McpProtocol::Legacy => self.initialize().await,
        }
    }

    /// Sends `server/discover`.
    pub async fn discover(&self) -> Result<Value> {
        self.request("server/discover", json!({}))
            .await
    }

    /// Performs the legacy `initialize` handshake, including the
    /// `notifications/initialized` notification.
    pub async fn initialize(&self) -> Result<Value> {
        let result = self.initialize_only().await?;
        self.notify("notifications/initialized", json!({}))
            .await?;
        Ok(result)
    }

    /// Sends the legacy `initialize` request without the follow-up
    /// notification.
    pub async fn initialize_only(&self) -> Result<Value> {
        self.set_protocol(McpProtocol::Legacy)
            .await;
        self.request(
            "initialize",
            json!({
                "protocolVersion": LEGACY_MCP_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": self.client_name, "version": CLIENT_VERSION},
            }),
        )
        .await
    }

    /// Calls `tools/list`.
    pub async fn list_tools(&self) -> Result<Value> {
        self.request("tools/list", json!({}))
            .await
    }

    /// Calls an MCP tool and returns the raw JSON-RPC response.
    pub async fn call_tool(&self, tool_name: &str, arguments: Value) -> Result<Value> {
        self.request("tools/call", json!({ "name": tool_name, "arguments": arguments }))
            .await
    }

    /// Ends the legacy session with an HTTP `DELETE`.
    pub async fn close_session(&self) -> Result<()> {
        let Some(session_id) = self.session_id().await else {
            bail!("no session to close");
        };
        let response = self
            .http
            .delete(&self.mcp_url)
            .header(MCP_SESSION_ID_HEADER, &session_id)
            .send()
            .await
            .context("MCP session DELETE failed")?;
        let status = response.status();
        ensure!(status.is_success(), "MCP session DELETE failed with HTTP {status}");
        self.clear_session().await;
        Ok(())
    }

    /// Sends a JSON-RPC notification (no `id`, no response expected).
    pub async fn notify(&self, method: &str, params: Value) -> Result<()> {
        let payload = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        let response = self
            .post(payload)
            .await
            .with_context(|| format!("MCP notification failed for method `{method}`"))?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_default();
            bail!("MCP notification `{method}` failed with HTTP {status}: {body}");
        }
        Ok(())
    }

    /// Sends a JSON-RPC request and returns the final JSON-RPC response,
    /// failing on HTTP errors and JSON-RPC error payloads.
    ///
    /// Handles both plain JSON and SSE (`text/event-stream`) responses.
    pub async fn request(&self, method: &str, params: Value) -> Result<Value> {
        let request_id = self
            .next_request_id
            .fetch_add(1, Ordering::Relaxed);
        let payload = json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": method,
            "params": params,
        });
        let response = self
            .post(payload)
            .await
            .with_context(|| format!("MCP request failed for method `{method}`"))?;

        let status = response.status();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let body = response
            .text()
            .await
            .context("failed to read MCP response body")?;
        if !status.is_success() {
            bail!("MCP request `{method}` failed with HTTP {status}: {body}");
        }

        let parsed = if content_type.contains("text/event-stream") {
            parse_last_sse_json_data(&body).with_context(|| {
                format!("MCP request `{method}` returned SSE with no JSON data event: {body}")
            })?
        } else {
            serde_json::from_str::<Value>(&body)
                .with_context(|| format!("MCP request `{method}` returned invalid JSON: {body}"))?
        };

        if parsed.get("error").is_some() {
            bail!("MCP request `{method}` returned JSON-RPC error payload: {parsed}");
        }
        Ok(parsed)
    }

    /// POSTs a JSON-RPC message, adding the protocol-specific metadata and
    /// capturing any `Mcp-Session-Id` the server returns.
    async fn post(&self, mut payload: Value) -> Result<reqwest::Response> {
        let mut request = self
            .http
            .post(&self.mcp_url)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream");

        match self.protocol().await {
            McpProtocol::Stateless => {
                if payload.get("id").is_some() {
                    insert_stateless_request_meta(&mut payload, &self.client_name, CLIENT_VERSION);
                }
                for (name, value) in
                    stateless_request_headers(&payload, LATEST_MCP_PROTOCOL_VERSION)
                {
                    request = request.header(name, value);
                }
            }
            McpProtocol::Legacy => {
                if let Some(session_id) = self.session_id().await {
                    request = request.header(MCP_SESSION_ID_HEADER, session_id);
                }
            }
        }

        let response = request
            .json(&payload)
            .send()
            .await?;

        if let Some(session_id) = response
            .headers()
            .get(MCP_SESSION_ID_HEADER)
            .and_then(|v| v.to_str().ok())
        {
            self.set_session_id(session_id.to_string())
                .await;
        }
        Ok(response)
    }
}

/// Extracts the last JSON-RPC payload from an SSE response body.
fn parse_last_sse_json_data(body: &str) -> Result<Value> {
    body.lines()
        .rev()
        .filter_map(|line| line.strip_prefix("data:"))
        .find_map(|payload| serde_json::from_str::<Value>(payload.trim()).ok())
        .context("no JSON data events found in SSE body")
}
