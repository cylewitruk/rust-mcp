//! E2E container harness for the full rust-mcp Docker image.

use std::fs::{self, File};
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail, ensure};
use rust_mcp_types::protocol::LATEST_MCP_PROTOCOL_VERSION;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use testcontainers_modules::testcontainers::core::{
    BuildImageOptions, Host, IntoContainerPort as _, Mount,
};
use testcontainers_modules::testcontainers::runners::{AsyncBuilder as _, AsyncRunner as _};
use testcontainers_modules::testcontainers::{
    ContainerAsync, GenericBuildableImage, GenericImage, ImageExt as _,
};
use tokio::sync::Mutex;
use tokio::time::{Instant, sleep};

use crate::env;

const DEFAULT_IMAGE_NAME: &str = "rust-mcp";
const DEFAULT_PROTOCOL_VERSION: &str = LATEST_MCP_PROTOCOL_VERSION;
const MCP_INTERNAL_PORT: u16 = 43173;

/// MCP session header name per the Streamable HTTP spec.
const MCP_SESSION_ID_HEADER: &str = "mcp-session-id";

/// Running rust-mcp container plus endpoint URLs.
#[derive(Debug)]
pub struct RustMcpTestContainer {
    container: ContainerAsync<GenericImage>,
    base_url: String,
    mcp_url: String,
    client: reqwest::Client,
    next_request_id: AtomicU64,
    session_id: Mutex<Option<String>>,
}

impl RustMcpTestContainer {
    /// Builds (if needed) and starts the rust-mcp Docker image.
    ///
    /// The build uses `with_skip_if_exists(true)` and a source-fingerprinted
    /// image tag so identical source trees are reused while source changes
    /// trigger a fresh image build.
    pub async fn start() -> Result<Self> {
        Self::start_with_build_options_and_env(
            BuildImageOptions::new().with_skip_if_exists(true),
            Vec::<(String, String)>::new(),
        )
        .await
    }

    /// Builds (or reuses) and starts the rust-mcp Docker image with custom
    /// options.
    pub async fn start_with_build_options(build_options: BuildImageOptions) -> Result<Self> {
        Self::start_with_build_options_and_env(build_options, Vec::<(String, String)>::new()).await
    }

    /// Builds (or reuses) and starts the rust-mcp Docker image with additional
    /// container environment variables.
    pub async fn start_with_env<K, V, I>(env_vars: I) -> Result<Self>
    where
        K: Into<String>,
        V: Into<String>,
        I: IntoIterator<Item = (K, V)>,
    {
        Self::start_with_build_options_and_env(
            BuildImageOptions::new().with_skip_if_exists(true),
            env_vars,
        )
        .await
    }

    /// Builds (or reuses) and starts the rust-mcp Docker image with additional
    /// container environment variables and bind mounts.
    pub async fn start_with_env_and_mounts<K, V, I, P, Q, M>(env_vars: I, mounts: M) -> Result<Self>
    where
        K: Into<String>,
        V: Into<String>,
        I: IntoIterator<Item = (K, V)>,
        P: Into<String>,
        Q: Into<String>,
        M: IntoIterator<Item = (P, Q)>,
    {
        Self::start_with_build_options_env_and_mounts(
            BuildImageOptions::new().with_skip_if_exists(true),
            env_vars,
            mounts,
        )
        .await
    }

    /// Builds (or reuses) and starts the rust-mcp Docker image with custom
    /// build options and additional container environment variables.
    pub async fn start_with_build_options_and_env<K, V, I>(
        build_options: BuildImageOptions,
        env_vars: I,
    ) -> Result<Self>
    where
        K: Into<String>,
        V: Into<String>,
        I: IntoIterator<Item = (K, V)>,
    {
        Self::start_with_build_options_env_and_mounts(
            build_options,
            env_vars,
            Vec::<(String, String)>::new(),
        )
        .await
    }

    /// Builds (or reuses) and starts the rust-mcp Docker image with custom
    /// build options, additional container environment variables, and mounts.
    pub async fn start_with_build_options_env_and_mounts<K, V, I, P, Q, M>(
        build_options: BuildImageOptions,
        env_vars: I,
        mounts: M,
    ) -> Result<Self>
    where
        K: Into<String>,
        V: Into<String>,
        I: IntoIterator<Item = (K, V)>,
        P: Into<String>,
        Q: Into<String>,
        M: IntoIterator<Item = (P, Q)>,
    {
        let extra_env_vars = env_vars
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect::<Vec<_>>();
        let extra_mounts = mounts
            .into_iter()
            .map(|(host_path, container_path)| (host_path.into(), container_path.into()))
            .collect::<Vec<(String, String)>>();
        let workspace_root = workspace_root()?;
        let image_tag = env::optional_env_non_empty(env::vars::RUST_MCP_TEST_IMAGE_TAG)
            .unwrap_or(default_image_tag(&workspace_root)?);
        let image = GenericBuildableImage::new(DEFAULT_IMAGE_NAME, image_tag)
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

        let mut container_request = image
            .with_exposed_port(MCP_INTERNAL_PORT.tcp())
            // Support containers reaching host-bound test fixtures via
            // `host.docker.internal`.
            .with_host("host.docker.internal", Host::HostGateway)
            .with_env_var(env::vars::OUTBOUND_FIREWALL, env::defaults::OUTBOUND_FIREWALL_DISABLED)
            .with_env_var(env::vars::MCP_HTTP_BIND, env::defaults::MCP_HTTP_BIND)
            // The runtime container runs Postgres over unix socket only.
            .with_env_var(env::vars::DATABASE_URL, env::defaults::DATABASE_URL)
            .with_env_var(env::vars::RUST_LOG, env::defaults::RUST_LOG);

        for (key, value) in extra_env_vars {
            container_request = container_request.with_env_var(key, value);
        }

        for (host_path, container_path) in extra_mounts {
            container_request =
                container_request.with_mount(Mount::bind_mount(host_path, container_path));
        }

        let container = container_request
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

        let base_url = format!("http://{host}:{mcp_port}");
        let mcp_url = format!("{base_url}/mcp");

        Ok(Self {
            container,
            base_url,
            mcp_url,
            client: reqwest::Client::new(),
            next_request_id: AtomicU64::new(1),
            session_id: Mutex::new(None),
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

    /// Returns the underlying container handle.
    pub fn container(&self) -> &ContainerAsync<GenericImage> {
        &self.container
    }

    /// Clears the locally cached MCP session identifier.
    pub async fn clear_session(&self) {
        *self.session_id.lock().await = None;
    }

    /// Performs MCP `initialize` and returns the raw JSON-RPC response payload.
    pub async fn initialize_mcp(&self) -> Result<Value> {
        let result = self
            .initialize_only_mcp()
            .await?;

        // The MCP spec requires the client to send `notifications/initialized`
        // after receiving the initialize result.
        self.notify("notifications/initialized", json!({}))
            .await?;

        Ok(result)
    }

    /// Performs MCP `initialize` but does not send
    /// `notifications/initialized`.
    pub async fn initialize_only_mcp(&self) -> Result<Value> {
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

    /// Calls MCP `tools/list` and returns the raw JSON-RPC response.
    pub async fn list_tools_mcp(&self) -> Result<Value> {
        self.rpc_call("tools/list", json!({}))
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

    /// Sends a JSON-RPC notification (no `id`, no response expected).
    pub async fn notify(&self, method: &str, params: Value) -> Result<()> {
        let payload = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });

        let mut request = self
            .client
            .post(&self.mcp_url)
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

        let mut request = self
            .client
            .post(&self.mcp_url)
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
            .with_context(|| format!("failed MCP request for method `{method}`"))?;

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
            bail!("MCP request `{method}` returned error payload: {parsed}");
        }

        Ok(parsed)
    }
}

/// Extracts the last JSON-RPC payload from an SSE response body.
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

fn default_image_tag(workspace_root: &Path) -> Result<String> {
    let mut hasher = Sha256::new();

    for path in [
        required_path(workspace_root, "Dockerfile")?,
        required_path(workspace_root, "docker-entrypoint.sh")?,
        required_path(workspace_root, "Cargo.toml")?,
        required_path(workspace_root, "Cargo.lock")?,
        required_path(workspace_root, "README.md")?,
    ] {
        hash_file(&path, workspace_root, &mut hasher)?;
    }

    hash_tree(&required_path(workspace_root, "crates")?, workspace_root, &mut hasher)?;
    hash_tree(&required_path(workspace_root, "migrations")?, workspace_root, &mut hasher)?;

    let digest = hasher.finalize();
    let mut prefix = String::with_capacity(16);
    for byte in digest.iter().take(8) {
        use std::fmt::Write as _;
        let _ = write!(prefix, "{byte:02x}");
    }

    Ok(format!("test-{prefix}"))
}

fn hash_tree(path: &Path, workspace_root: &Path, hasher: &mut Sha256) -> Result<()> {
    if path.is_file() {
        return hash_file(path, workspace_root, hasher);
    }

    let mut entries = fs::read_dir(path)
        .with_context(|| format!("failed to list {}", path.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .with_context(|| format!("failed to read entries under {}", path.display()))?;
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let entry_path = entry.path();
        if entry_path.is_dir() {
            hash_tree(&entry_path, workspace_root, hasher)?;
            continue;
        }
        if entry_path.is_file() {
            hash_file(&entry_path, workspace_root, hasher)?;
        }
    }

    Ok(())
}

fn hash_file(path: &Path, workspace_root: &Path, hasher: &mut Sha256) -> Result<()> {
    let relative = path
        .strip_prefix(workspace_root)
        .with_context(|| {
            format!(
                "failed to strip workspace prefix `{}` from `{}`",
                workspace_root.display(),
                path.display()
            )
        })?;
    let relative = relative
        .components()
        .map(|component| {
            component
                .as_os_str()
                .to_string_lossy()
        })
        .collect::<Vec<_>>()
        .join("/");

    hasher.update(b"file:");
    hasher.update(relative.as_bytes());
    hasher.update(b"\n");

    let mut file =
        File::open(path).with_context(|| format!("failed to open file {}", path.display()))?;
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .with_context(|| format!("failed to read {}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    hasher.update(b"\n");

    Ok(())
}
