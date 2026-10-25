//! In-process MCP HTTP test harness utilities.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use axum::Router;
use serde_json::Value;
use tokio::net::TcpListener;
use tokio::sync::{Mutex, oneshot};
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep};
use tracing::debug;

use crate::mcp_client::McpHttpClient;

const CLIENT_NAME: &str = "rust-mcp-integration";

/// Local HTTP server harness for MCP JSON-RPC integration tests.
///
/// Speaks the stateless protocol unless [`LocalMcpHttpHarness::initialize`]
/// establishes a legacy session.
#[derive(Debug)]
pub struct LocalMcpHttpHarness {
    base_url: String,
    client: McpHttpClient,
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
        let client = McpHttpClient::new(format!("{base_url}/mcp"), CLIENT_NAME);

        Ok(Self {
            base_url,
            client,
            shutdown_tx: Mutex::new(Some(shutdown_tx)),
            serve_task: Mutex::new(Some(serve_task)),
        })
    }

    /// Returns the base URL for the local HTTP server.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Returns the MCP client used by this harness.
    pub fn client(&self) -> &McpHttpClient {
        &self.client
    }

    /// Returns the current legacy session ID, if one has been established.
    pub async fn session_id(&self) -> Option<String> {
        self.client.session_id().await
    }

    /// Overrides the legacy session ID used for subsequent requests.
    pub async fn set_session_id(&self, session_id: String) {
        self.client
            .set_session_id(session_id)
            .await;
    }

    /// Waits for `/readyz` to return success or times out.
    pub async fn wait_until_ready(&self, timeout: Duration) -> Result<()> {
        let readyz_url = format!("{}/readyz", self.base_url());
        let http = reqwest::Client::new();
        let started_at = Instant::now();
        let mut last_error = None;

        while started_at.elapsed() < timeout {
            match http
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

    /// Prepares the harness for tool calls (`server/discover` when stateless).
    pub async fn connect(&self) -> Result<Value> {
        self.client.connect().await
    }

    /// Performs the legacy `initialize` handshake, switching this harness to a
    /// session-based protocol.
    pub async fn initialize(&self) -> Result<Value> {
        self.client.initialize().await
    }

    /// Calls an MCP tool and returns raw JSON-RPC response.
    #[tracing::instrument(skip(self, arguments))]
    pub async fn call_tool(&self, tool_name: &str, arguments: Value) -> Result<Value> {
        debug!(?arguments, "Calling tool");
        let start = Instant::now();

        let response = self
            .client
            .call_tool(tool_name, arguments)
            .await?;
        ensure_not_tool_error("tools/call", &response)?;

        let elapsed_secs = start.elapsed().as_secs_f32();
        debug!(%elapsed_secs, "Tool call finished");
        Ok(response)
    }

    /// Sends a JSON-RPC notification (no `id`, no response expected).
    pub async fn notify(&self, method: &str, params: Value) -> Result<()> {
        self.client
            .notify(method, params)
            .await
    }

    /// Sends a JSON-RPC request, failing on JSON-RPC and tool-level errors.
    pub async fn rpc_call(&self, method: &str, params: Value) -> Result<Value> {
        let response = self
            .client
            .request(method, params)
            .await?;
        ensure_not_tool_error(method, &response)?;
        Ok(response)
    }
}

/// Fails when rmcp reported a tool-level error (`result.isError: true`).
fn ensure_not_tool_error(method: &str, response: &Value) -> Result<()> {
    let Some(result) = response
        .get("result")
        .filter(|result| {
            result
                .get("isError")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
    else {
        return Ok(());
    };
    let message = result
        .get("content")
        .and_then(Value::as_array)
        .and_then(|arr| arr.first())
        .and_then(|item| item.get("text"))
        .and_then(Value::as_str)
        .unwrap_or("<no error text>");
    bail!("MCP request `{method}` returned tool-level error: {message}")
}
