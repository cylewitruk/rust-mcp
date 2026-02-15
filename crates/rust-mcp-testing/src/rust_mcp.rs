//! E2E container harness for the full rust-mcp Docker image.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use serde_json::{Value, json};
use testcontainers_modules::testcontainers::core::{BuildImageOptions, IntoContainerPort as _};
use testcontainers_modules::testcontainers::runners::{AsyncBuilder as _, AsyncRunner as _};
use testcontainers_modules::testcontainers::{
    ContainerAsync, GenericBuildableImage, GenericImage, ImageExt as _,
};
use tokio::time::{Instant, sleep};

const DEFAULT_IMAGE_NAME: &str = "rust-mcp";
const DEFAULT_IMAGE_TAG: &str = "test";
const DEFAULT_PROTOCOL_VERSION: &str = "2025-11-25";
const MCP_INTERNAL_PORT: u16 = 43173;
const METRICS_INTERNAL_PORT: u16 = 9090;

/// Running rust-mcp container plus endpoint URLs.
#[derive(Debug)]
pub struct RustMcpTestContainer {
    container: ContainerAsync<GenericImage>,
    base_url: String,
    mcp_url: String,
    metrics_url: String,
    next_request_id: AtomicU64,
}

impl RustMcpTestContainer {
    /// Builds (if needed) and starts the rust-mcp Docker image.
    ///
    /// The build uses `with_skip_if_exists(true)` so subsequent test runs can
    /// reuse an already-built `rust-mcp:test` image.
    pub async fn start() -> Result<Self> {
        Self::start_with_build_options(BuildImageOptions::new().with_skip_if_exists(true)).await
    }

    /// Builds (or reuses) and starts the rust-mcp Docker image with custom
    /// options.
    pub async fn start_with_build_options(build_options: BuildImageOptions) -> Result<Self> {
        let workspace_root = workspace_root()?;
        let image = GenericBuildableImage::new(DEFAULT_IMAGE_NAME, DEFAULT_IMAGE_TAG)
            .with_dockerfile(required_path(&workspace_root, "Dockerfile")?)
            .with_file(required_path(&workspace_root, "Cargo.toml")?, "Cargo.toml")
            .with_file(required_path(&workspace_root, "Cargo.lock")?, "Cargo.lock")
            .with_file(required_path(&workspace_root, "README.md")?, "README.md")
            .with_file(
                required_path(&workspace_root, "docker-entrypoint.sh")?,
                "docker-entrypoint.sh",
            )
            .with_file(required_path(&workspace_root, "crates")?, "crates")
            .with_file(required_path(&workspace_root, "migrations")?, "migrations")
            .build_image_with(build_options)
            .await
            .context("failed to build rust-mcp Docker image")?;

        let container = image
            .with_exposed_port(MCP_INTERNAL_PORT.tcp())
            .with_exposed_port(METRICS_INTERNAL_PORT.tcp())
            .with_env_var("OUTBOUND_FIREWALL", "false")
            .with_env_var("MCP_HTTP_BIND", "0.0.0.0:43173")
            .with_env_var("PROMETHEUS_BIND", "0.0.0.0:9090")
            .with_env_var("RUST_LOG", "warn")
            .start()
            .await
            .context("failed to start rust-mcp container")?;

        let host = container
            .get_host()
            .await
            .context("failed to resolve rust-mcp container host")?;
        let mcp_port = container
            .get_host_port_ipv4(MCP_INTERNAL_PORT)
            .await
            .context("failed to resolve rust-mcp MCP port mapping")?;
        let metrics_port = container
            .get_host_port_ipv4(METRICS_INTERNAL_PORT)
            .await
            .context("failed to resolve rust-mcp metrics port mapping")?;

        let base_url = format!("http://{host}:{mcp_port}");
        let mcp_url = format!("{base_url}/mcp");
        let metrics_url = format!("http://{host}:{metrics_port}");

        Ok(Self {
            container,
            base_url,
            mcp_url,
            metrics_url,
            next_request_id: AtomicU64::new(1),
        })
    }

    /// Returns the container base URL (e.g. `http://127.0.0.1:43173`).
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Returns the full MCP endpoint URL.
    pub fn mcp_url(&self) -> &str {
        &self.mcp_url
    }

    /// Returns the metrics endpoint base URL.
    pub fn metrics_url(&self) -> &str {
        &self.metrics_url
    }

    /// Returns the underlying container handle.
    pub fn container(&self) -> &ContainerAsync<GenericImage> {
        &self.container
    }

    /// Performs MCP `initialize` and returns the raw JSON-RPC response payload.
    pub async fn initialize_mcp(&self) -> Result<Value> {
        self.rpc_call(
            "initialize",
            json!({
                "protocolVersion": DEFAULT_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "rust-mcp-e2e", "version": "0.1.0"},
            }),
        )
        .await
    }

    /// Calls an MCP tool over JSON-RPC and returns the raw JSON-RPC response.
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

    /// Polls `/readyz` until the container is ready or times out.
    pub async fn wait_until_ready(&self, timeout: Duration) -> Result<()> {
        let readyz_url = format!("{}/readyz", self.base_url);
        let client = reqwest::Client::new();
        let started_at = Instant::now();
        let mut last_error = None;

        while started_at.elapsed() < timeout {
            match client
                .get(&readyz_url)
                .send()
                .await
            {
                Ok(response) if response.status().is_success() => return Ok(()),
                Ok(response) => {
                    last_error =
                        Some(anyhow!("readyz returned unexpected status {}", response.status()));
                }
                Err(error) => {
                    last_error = Some(anyhow!("readyz request failed: {error}"));
                }
            }

            sleep(Duration::from_millis(250)).await;
        }

        match last_error {
            Some(error) => Err(error).context(format!("timed out waiting for {readyz_url}")),
            None => bail!("timed out waiting for {readyz_url}"),
        }
    }

    async fn rpc_call(&self, method: &str, params: Value) -> Result<Value> {
        let request_id = self
            .next_request_id
            .fetch_add(1, Ordering::Relaxed);
        let payload = json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": method,
            "params": params,
        });

        let response = reqwest::Client::new()
            .post(&self.mcp_url)
            .header("content-type", "application/json")
            .header("accept", "application/json")
            .json(&payload)
            .send()
            .await
            .with_context(|| format!("failed MCP request for method `{method}`"))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .context("failed to read MCP response body")?;
        if !status.is_success() {
            bail!("MCP request `{method}` failed with HTTP {status}: {body}");
        }

        let parsed = serde_json::from_str::<Value>(&body)
            .with_context(|| format!("MCP request `{method}` returned invalid JSON: {body}"))?;
        if parsed.get("error").is_some() {
            bail!("MCP request `{method}` returned error payload: {parsed}");
        }

        Ok(parsed)
    }
}

fn workspace_root() -> Result<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .context("failed to derive workspace root from CARGO_MANIFEST_DIR")
}

fn required_path(workspace_root: &Path, relative: &str) -> Result<PathBuf> {
    let path = workspace_root.join(relative);
    if !path.exists() {
        bail!("required path missing for rust-mcp image build: {}", path.display());
    }
    Ok(path)
}
