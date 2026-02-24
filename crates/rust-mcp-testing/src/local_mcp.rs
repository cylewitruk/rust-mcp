//! In-process MCP HTTP test harness utilities.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail, ensure};
use axum::Router;
use rust_mcp_types::protocol::LATEST_MCP_PROTOCOL_VERSION;
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::{Mutex, oneshot};
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep};

/// MCP session header name per the Streamable HTTP spec.
const MCP_SESSION_ID_HEADER: &str = "mcp-session-id";

/// Local HTTP server harness for MCP JSON-RPC integration tests.
#[derive(Debug)]
pub struct LocalMcpHttpHarness {
    base_url: String,
    mcp_url: String,
    client: reqwest::Client,
    next_request_id: AtomicU64,
    session_id: Mutex<Option<String>>,
    shutdown_tx: Mutex<Option<oneshot::Sender<()>>>,
    serve_task: Mutex<Option<JoinHandle<()>>>,
}

impl Drop for LocalMcpHttpHarness {
    fn drop(&mut self) {
        if let Some(shutdown_tx) = self
            .shutdown_tx
            .get_mut()
            .take()
        {
            let _ = shutdown_tx.send(());
        }
        if let Some(task) = self
            .serve_task
            .get_mut()
            .take()
        {
            task.abort();
        }
    }
}

impl LocalMcpHttpHarness {
    /// Starts an axum server on an ephemeral localhost port.
    pub async fn spawn(router: Router) -> Result<Self> {
        let listener = TcpListener::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)))
            .await
            .context("failed to bind local MCP test server listener")?;
        let addr = listener.local_addr()?;

        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let serve_task = tokio::spawn(async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await;
        });

        let base_url = format!("http://{addr}");
        let mcp_url = format!("{base_url}/mcp");

        Ok(Self {
            base_url,
            mcp_url,
            client: reqwest::Client::new(),
            next_request_id: AtomicU64::new(1),
            session_id: Mutex::new(None),
            shutdown_tx: Mutex::new(Some(shutdown_tx)),
            serve_task: Mutex::new(Some(serve_task)),
        })
    }

    /// Returns the base URL for the local HTTP server.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Returns the full MCP endpoint URL.
    pub fn mcp_url(&self) -> &str {
        &self.mcp_url
    }

    /// Waits for `/readyz` to return success or times out.
    pub async fn wait_until_ready(&self, timeout: Duration) -> Result<()> {
        let readyz_url = format!("{}/readyz", self.base_url());
        let started_at = Instant::now();
        let mut last_error = None;

        while started_at.elapsed() < timeout {
            match self
                .client
                .get(&readyz_url)
                .send()
                .await
            {
                Ok(response) if response.status().is_success() => return Ok(()),
                Ok(response) => {
                    last_error = Some(anyhow!("readyz returned status {}", response.status()));
                }
                Err(error) => {
                    last_error = Some(anyhow!("readyz request failed: {error}"));
                }
            }
            sleep(Duration::from_millis(100)).await;
        }

        match last_error {
            Some(error) => Err(error).context("timed out waiting for local MCP server readiness"),
            None => bail!("timed out waiting for local MCP server readiness"),
        }
    }

    /// Sends the MCP `initialize` request followed by the required
    /// `notifications/initialized` notification. Returns the initialize
    /// JSON-RPC response.
    pub async fn initialize(&self, client_name: &str) -> Result<Value> {
        let result = self
            .rpc_call(
                "initialize",
                json!({
                    "protocolVersion": LATEST_MCP_PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {"name": client_name, "version": "0.1.0"},
                }),
            )
            .await?;

        // The MCP spec requires the client to send `notifications/initialized`
        // after receiving the initialize result.
        self.notify("notifications/initialized", json!({}))
            .await?;

        Ok(result)
    }

    /// Calls an MCP tool and returns raw JSON-RPC response.
    pub async fn call_tool(&self, tool_name: &str, arguments: Value) -> Result<Value> {
        self.rpc_call(
            "tools/call",
            json!({
                "name": tool_name,
                "arguments": arguments,
            }),
        )
        .await
    }

    /// Sends a JSON-RPC notification (no `id`, no response expected).
    pub async fn notify(&self, method: &str, params: Value) -> Result<()> {
        let payload = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });

        let mut request = self
            .client
            .post(self.mcp_url())
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream");

        if let Some(sid) = self
            .session_id
            .lock()
            .await
            .as_deref()
        {
            request = request.header(MCP_SESSION_ID_HEADER, sid);
        }

        let response = request
            .json(&payload)
            .send()
            .await
            .with_context(|| format!("MCP notification failed for method `{method}`"))?;

        // Capture session ID if present.
        if let Some(sid) = response
            .headers()
            .get(MCP_SESSION_ID_HEADER)
            .and_then(|v| v.to_str().ok())
        {
            *self.session_id.lock().await = Some(sid.to_string());
        }

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

    /// Sends a JSON-RPC request against the local MCP endpoint.
    ///
    /// Handles both plain JSON and SSE (`text/event-stream`) responses
    /// transparently — the MCP Streamable HTTP transport may use either.
    /// Automatically captures and reuses the `Mcp-Session-Id` header.
    pub async fn rpc_call(&self, method: &str, params: Value) -> Result<Value> {
        let request_id = self
            .next_request_id
            .fetch_add(1, Ordering::Relaxed);
        let payload = json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": method,
            "params": params,
        });

        let mut request = self
            .client
            .post(self.mcp_url())
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream");

        // Attach session ID if we have one from a prior request.
        if let Some(sid) = self
            .session_id
            .lock()
            .await
            .as_deref()
        {
            request = request.header(MCP_SESSION_ID_HEADER, sid);
        }

        let response = request
            .json(&payload)
            .send()
            .await
            .with_context(|| format!("MCP request failed for method `{method}`"))?;

        let status = response.status();

        // Capture session ID from response headers.
        if let Some(sid) = response
            .headers()
            .get(MCP_SESSION_ID_HEADER)
            .and_then(|v| v.to_str().ok())
        {
            *self.session_id.lock().await = Some(sid.to_string());
        }

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

        // Detect tool-level errors: rmcp wraps `Err(String)` as
        // `result.isError: true` with error text in `result.content`.
        if let Some(result) = parsed.get("result")
            && result
                .get("isError")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
        {
            let message = result
                .get("content")
                .and_then(serde_json::Value::as_array)
                .and_then(|arr| arr.first())
                .and_then(|item| item.get("text"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("<no error text>");
            bail!("MCP request `{method}` returned tool-level error: {message}");
        }

        Ok(parsed)
    }
}

/// Extracts the last JSON-RPC payload from an SSE response body.
///
/// SSE events look like:
/// ```text
/// data:
/// id: 0
/// retry: 3000
///
/// data: {"jsonrpc":"2.0","id":1,"result":{...}}
/// ```
///
/// We scan for `data:` lines that contain valid JSON and return the last one,
/// which is the final JSON-RPC response in the stream.
fn parse_last_sse_json_data(body: &str) -> Result<Value> {
    let mut last_json: Option<Value> = None;

    for line in body.lines() {
        let Some(payload) = line.strip_prefix("data:") else {
            continue;
        };
        let payload = payload.trim();
        if payload.is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(payload) {
            last_json = Some(value);
        }
    }

    ensure!(last_json.is_some(), "no JSON data events found in SSE body");
    Ok(last_json.unwrap())
}
