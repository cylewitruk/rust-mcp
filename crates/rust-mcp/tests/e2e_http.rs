//! Docker image E2E smoke tests for rust-mcp HTTP endpoints.

use std::collections::{BTreeSet, HashMap};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use axum::extract::{Path, Query};
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use rust_mcp_testing::rust_mcp::RustMcpTestContainer;
use rustdoc_types::{
    Abi, Crate as RustdocCrate, Function, FunctionHeader, FunctionSignature, Generics, Id, Item,
    ItemEnum, ItemKind, ItemSummary, Module, Target, Type as RustdocType,
    Visibility as RustdocVisibility,
};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

const SEEDED_CRATE_NAME: &str = "demo-crate";
const SEEDED_CRATE_VERSION: &str = "1.2.3";
const SEEDED_CRATE_NEXT_VERSION: &str = "1.3.0";
const SEEDED_ALT_CRATE_NAME: &str = "demo-alt";
const SEEDED_ALT_CRATE_VERSION: &str = "0.9.0";
const SEEDED_RUSTDOC_PATH: &str = "rustdoc-json/docs.rs/demo-crate-1.2.3.json";
const SEEDED_AUDIT_MOUNT_PATH: &str = "/e2e-fixtures";

struct MockRegistryServer {
    container_base_url: String,
    shutdown_tx: Option<oneshot::Sender<()>>,
    serve_task: JoinHandle<()>,
}

impl Drop for MockRegistryServer {
    fn drop(&mut self) {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }
        self.serve_task.abort();
    }
}

impl MockRegistryServer {
    async fn start() -> Result<Self> {
        let router = Router::new()
            .route("/api/v1/crates", get(mock_search_crates))
            .route("/api/v1/crates/{crate_name}", get(mock_crate_detail))
            .route("/api/v1/crates/{crate_name}/{version}/readme", get(mock_crate_readme))
            .route(
                "/api/v1/crates/{crate_name}/{version}/dependencies",
                get(mock_crate_dependencies),
            )
            .route("/crate/{crate_name}/{version}/json.gz", get(mock_rustdoc_json_gzip));

        let listener =
            TcpListener::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))).await?;
        let port = listener.local_addr()?.port();
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let serve_task = tokio::spawn(async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await;
        });

        Ok(Self {
            container_base_url: format!("http://host.docker.internal:{port}"),
            shutdown_tx: Some(shutdown_tx),
            serve_task,
        })
    }

    fn container_base_url(&self) -> &str {
        &self.container_base_url
    }
}

struct ManifestFixtureMount {
    host_dir: PathBuf,
    container_manifest_path: String,
}

impl ManifestFixtureMount {
    fn create() -> Result<Self> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let host_dir = std::env::temp_dir().join(format!("rust-mcp-e2e-manifest-{nanos}"));
        std::fs::create_dir_all(&host_dir)?;
        std::fs::write(
            host_dir.join("Cargo.toml"),
            format!(
                "[package]
name = \"e2e-manifest\"
version = \"0.1.0\"
edition = \"2021\"
rust-version = \"1.75\"

[dependencies]
{SEEDED_CRATE_NAME} = \"^{SEEDED_CRATE_VERSION}\"
{SEEDED_ALT_CRATE_NAME} = \"^{SEEDED_ALT_CRATE_VERSION}\"
"
            ),
        )?;

        Ok(Self {
            host_dir,
            container_manifest_path: format!("{SEEDED_AUDIT_MOUNT_PATH}/Cargo.toml"),
        })
    }

    fn bind_mount(&self) -> (String, String) {
        (
            self.host_dir
                .to_string_lossy()
                .to_string(),
            SEEDED_AUDIT_MOUNT_PATH.to_string(),
        )
    }

    fn container_manifest_path(&self) -> &str {
        &self.container_manifest_path
    }
}

impl Drop for ManifestFixtureMount {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.host_dir);
    }
}

fn build_rustdoc_fixture(
    crate_name: &str,
    crate_version: &str,
    function_name: &str,
    docs: &str,
) -> RustdocCrate {
    let mut index = HashMap::new();
    let mut paths = HashMap::new();

    index.insert(
        Id(0),
        Item {
            id: Id(0),
            crate_id: 0,
            name: Some(crate_name.to_string()),
            span: None,
            visibility: RustdocVisibility::Public,
            docs: None,
            links: HashMap::new(),
            attrs: Vec::new(),
            deprecation: None,
            inner: ItemEnum::Module(Module {
                is_crate: true,
                items: vec![Id(1)],
                is_stripped: false,
            }),
        },
    );
    paths.insert(
        Id(0),
        ItemSummary {
            crate_id: 0,
            path: vec![crate_name.to_string()],
            kind: ItemKind::Module,
        },
    );

    index.insert(
        Id(1),
        Item {
            id: Id(1),
            crate_id: 0,
            name: Some(function_name.to_string()),
            span: None,
            visibility: RustdocVisibility::Public,
            docs: Some(docs.to_string()),
            links: HashMap::new(),
            attrs: Vec::new(),
            deprecation: None,
            inner: ItemEnum::Function(Function {
                sig: FunctionSignature {
                    inputs: vec![("input".to_string(), RustdocType::Primitive("str".to_string()))],
                    output: Some(RustdocType::Primitive("bool".to_string())),
                    is_c_variadic: false,
                },
                generics: Generics {
                    params: vec![],
                    where_predicates: vec![],
                },
                header: FunctionHeader {
                    is_const: false,
                    is_unsafe: false,
                    is_async: false,
                    abi: Abi::Rust,
                },
                has_body: true,
            }),
        },
    );
    paths.insert(
        Id(1),
        ItemSummary {
            crate_id: 0,
            path: vec![crate_name.to_string(), function_name.to_string()],
            kind: ItemKind::Function,
        },
    );

    RustdocCrate {
        root: Id(0),
        crate_version: Some(crate_version.to_string()),
        includes_private: false,
        index,
        paths,
        external_crates: HashMap::new(),
        format_version: 57,
        target: Target {
            triple: "x86_64-unknown-linux-gnu".to_string(),
            target_features: vec![],
        },
    }
}

async fn mock_search_crates(Query(params): Query<HashMap<String, String>>) -> Json<Value> {
    let query = params
        .get("q")
        .map(|value| {
            value
                .trim()
                .to_ascii_lowercase()
        })
        .unwrap_or_default();
    let candidates = [SEEDED_CRATE_NAME, SEEDED_ALT_CRATE_NAME];
    let crates = candidates
        .into_iter()
        .filter(|name| {
            query.is_empty()
                || name
                    .to_ascii_lowercase()
                    .contains(&query)
        })
        .map(|name| json!({ "id": name, "name": name }))
        .collect::<Vec<_>>();

    Json(json!({
        "crates": crates,
        "meta": {"total": 2}
    }))
}

async fn mock_crate_detail(Path(crate_name): Path<String>) -> impl IntoResponse {
    let payload = match crate_name.as_str() {
        SEEDED_CRATE_NAME => json!({
            "crate": {
                "name": SEEDED_CRATE_NAME,
                "description": "E2E seeded primary crate fixture",
                "repository": "https://example.test/demo-crate",
                "documentation": "https://docs.rs/demo-crate",
                "homepage": "https://example.test/demo-crate",
                "max_version": SEEDED_CRATE_NEXT_VERSION
            },
            "versions": [
                {
                    "num": SEEDED_CRATE_NEXT_VERSION,
                    "created_at": "2026-01-03T00:00:00Z",
                    "updated_at": "2026-01-03T00:00:00Z",
                    "yanked": false,
                    "downloads": 256,
                    "checksum": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                    "rust_version": "1.76",
                    "license": "MIT OR Apache-2.0",
                    "features": {
                        "default": ["std"],
                        "std": [],
                        "serde": ["dep:serde"]
                    }
                },
                {
                    "num": SEEDED_CRATE_VERSION,
                    "created_at": "2026-01-01T00:00:00Z",
                    "updated_at": "2026-01-01T00:00:00Z",
                    "yanked": false,
                    "downloads": 123,
                    "checksum": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "rust_version": "1.75",
                    "license": "MIT OR Apache-2.0",
                    "features": {
                        "default": [],
                        "std": []
                    }
                }
            ],
            "keywords": [{
                "id": "kw-demo",
                "keyword": "demo"
            }],
            "categories": [{
                "id": "cat-devtools",
                "slug": "development-tools",
                "category": "Development tools"
            }]
        }),
        SEEDED_ALT_CRATE_NAME => json!({
            "crate": {
                "name": SEEDED_ALT_CRATE_NAME,
                "description": "E2E seeded alternative crate fixture",
                "repository": "https://example.test/demo-alt",
                "documentation": "https://docs.rs/demo-alt",
                "homepage": "https://example.test/demo-alt",
                "max_version": SEEDED_ALT_CRATE_VERSION
            },
            "versions": [{
                "num": SEEDED_ALT_CRATE_VERSION,
                "created_at": "2026-01-02T00:00:00Z",
                "updated_at": "2026-01-02T00:00:00Z",
                "yanked": false,
                "downloads": 88,
                "checksum": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
                "rust_version": "1.75",
                "license": "MIT",
                "features": {
                    "default": []
                }
            }],
            "keywords": [{
                "id": "kw-demo-alt",
                "keyword": "demo"
            }],
            "categories": [{
                "id": "cat-devtools",
                "slug": "development-tools",
                "category": "Development tools"
            }]
        }),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };

    (StatusCode::OK, Json(payload)).into_response()
}

async fn mock_crate_readme(
    Path((crate_name, version)): Path<(String, String)>,
) -> impl IntoResponse {
    match (crate_name.as_str(), version.as_str()) {
        (SEEDED_CRATE_NAME, SEEDED_CRATE_VERSION) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            "# demo-crate\nlegacy fixture readme",
        )
            .into_response(),
        (SEEDED_CRATE_NAME, SEEDED_CRATE_NEXT_VERSION) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            "# demo-crate\nmodern fixture readme",
        )
            .into_response(),
        (SEEDED_ALT_CRATE_NAME, SEEDED_ALT_CRATE_VERSION) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            "# demo-alt\nfixture readme",
        )
            .into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn mock_crate_dependencies(
    Path((crate_name, version)): Path<(String, String)>,
) -> impl IntoResponse {
    let payload = match (crate_name.as_str(), version.as_str()) {
        (SEEDED_CRATE_NAME, SEEDED_CRATE_VERSION) => json!({
            "dependencies": [
                {
                    "crate_id": "serde",
                    "req": "^1.0",
                    "kind": "normal",
                    "optional": false,
                    "features": []
                }
            ]
        }),
        (SEEDED_CRATE_NAME, SEEDED_CRATE_NEXT_VERSION) => json!({
            "dependencies": [
                {
                    "crate_id": "serde",
                    "req": "^1.0",
                    "kind": "normal",
                    "optional": true,
                    "features": []
                }
            ]
        }),
        (SEEDED_ALT_CRATE_NAME, SEEDED_ALT_CRATE_VERSION) => json!({
            "dependencies": [
                {
                    "crate_id": SEEDED_CRATE_NAME,
                    "req": "^1.2",
                    "kind": "normal",
                    "optional": false,
                    "features": []
                }
            ]
        }),
        _ => return (StatusCode::NOT_FOUND, Json(json!({ "dependencies": [] }))).into_response(),
    };

    (StatusCode::OK, Json(payload)).into_response()
}

async fn mock_rustdoc_json_gzip(
    Path((crate_name, version)): Path<(String, String)>,
) -> impl IntoResponse {
    let rustdoc = match (crate_name.as_str(), version.as_str()) {
        (SEEDED_CRATE_NAME, SEEDED_CRATE_VERSION) => build_rustdoc_fixture(
            "demo_crate",
            SEEDED_CRATE_VERSION,
            "parse",
            "Parse fixture input",
        ),
        (SEEDED_CRATE_NAME, SEEDED_CRATE_NEXT_VERSION) => build_rustdoc_fixture(
            "demo_crate",
            SEEDED_CRATE_NEXT_VERSION,
            "parse_input",
            "Parse fixture input with richer behavior",
        ),
        (SEEDED_ALT_CRATE_NAME, SEEDED_ALT_CRATE_VERSION) => build_rustdoc_fixture(
            "demo_alt",
            SEEDED_ALT_CRATE_VERSION,
            "call_demo_parse",
            "Calls demo_crate::parse for fixture coverage",
        ),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };

    let bytes = serde_json::to_vec(&rustdoc).expect("failed to encode rustdoc fixture");
    (StatusCode::OK, [(header::CONTENT_TYPE, "application/json")], bytes).into_response()
}

fn tool_result<'a>(response: &'a Value, tool_name: &str) -> &'a Value {
    let result = response
        .get("result")
        .unwrap_or_else(|| panic!("MCP tools/call {tool_name} returned no result payload"));
    let is_error = result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    assert!(!is_error, "MCP tools/call {tool_name} returned isError=true: {result}");
    result
}

fn structured_content<'a>(result: &'a Value, tool_name: &str) -> &'a Value {
    result
        .get("structuredContent")
        .or_else(|| result.get("structured_content"))
        .unwrap_or_else(|| panic!("MCP tools/call {tool_name} returned no structured content"))
}

fn parse_last_sse_json_data(body: &str) -> Option<Value> {
    parse_sse_json_data_events(body)
        .into_iter()
        .last()
}

fn parse_sse_json_data_events(body: &str) -> Vec<Value> {
    body.lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim)
        .filter(|payload| !payload.is_empty())
        .filter_map(|payload| serde_json::from_str::<Value>(payload).ok())
        .collect()
}

fn parse_mcp_response_events(content_type: &str, body: &str) -> Vec<Value> {
    if content_type.contains("text/event-stream") {
        let events = parse_sse_json_data_events(body);
        assert!(
            !events.is_empty(),
            "expected at least one JSON data event in SSE response body: {body}"
        );
        events
    } else {
        vec![
            serde_json::from_str::<Value>(body).unwrap_or_else(|error| {
                panic!("failed to parse MCP response JSON ({error}): {body}")
            }),
        ]
    }
}

fn parse_mcp_response(content_type: &str, body: &str) -> Value {
    if content_type.contains("text/event-stream") {
        parse_last_sse_json_data(body)
            .unwrap_or_else(|| panic!("expected JSON data event in SSE response body: {body}"))
    } else {
        serde_json::from_str::<Value>(body)
            .unwrap_or_else(|error| panic!("failed to parse MCP response JSON ({error}): {body}"))
    }
}

fn mock_registry_env(mock_server: &MockRegistryServer) -> Vec<(String, String)> {
    vec![
        (
            "CRATES_IO_BASE_URL".to_string(),
            mock_server
                .container_base_url()
                .to_string(),
        ),
        (
            "DOCS_RS_BASE_URL".to_string(),
            mock_server
                .container_base_url()
                .to_string(),
        ),
        ("CRATES_IO_MIN_INTERVAL_MS".to_string(), "1".to_string()),
        ("DOCS_RS_MIN_INTERVAL_MS".to_string(), "1".to_string()),
    ]
}

#[tokio::test]
async fn rust_mcp_container_serves_health_and_ready_endpoints() {
    let rust_mcp = RustMcpTestContainer::start()
        .await
        .expect("failed to start rust-mcp container");
    rust_mcp
        .wait_until_ready(Duration::from_secs(120))
        .await
        .expect("container did not become ready");

    let client = reqwest::Client::new();

    let healthz = client
        .get(format!("{}/healthz", rust_mcp.base_url()))
        .send()
        .await
        .expect("healthz request failed");
    assert!(healthz.status().is_success());

    let readyz = client
        .get(format!("{}/readyz", rust_mcp.base_url()))
        .send()
        .await
        .expect("readyz request failed");
    assert!(readyz.status().is_success());

    let mcp_response = client
        .post(rust_mcp.mcp_url())
        .body("{}")
        .send()
        .await
        .expect("mcp endpoint request failed");
    assert!(
        mcp_response
            .status()
            .is_client_error()
            || mcp_response
                .status()
                .is_success()
    );
}

#[tokio::test]
async fn rust_mcp_container_supports_initialize_and_ping_tool() {
    let rust_mcp = RustMcpTestContainer::start()
        .await
        .expect("failed to start rust-mcp container");
    rust_mcp
        .wait_until_ready(Duration::from_secs(120))
        .await
        .expect("container did not become ready");

    let initialize = rust_mcp
        .initialize_mcp()
        .await
        .expect("MCP initialize failed");
    assert!(
        initialize
            .get("result")
            .is_some()
    );

    let ping = rust_mcp
        .call_tool("ping", json!({ "message": "e2e-smoke" }))
        .await
        .expect("MCP tools/call ping failed");
    assert!(ping.get("result").is_some());
}

#[tokio::test]
async fn rust_mcp_container_initialize_response_matches_expected_contract() {
    let rust_mcp = RustMcpTestContainer::start()
        .await
        .expect("failed to start rust-mcp container");
    rust_mcp
        .wait_until_ready(Duration::from_secs(120))
        .await
        .expect("container did not become ready");

    let initialize = rust_mcp
        .initialize_only_mcp()
        .await
        .expect("MCP initialize failed");
    let result = initialize
        .get("result")
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("initialize response was missing result object: {initialize}"));

    let protocol_version = result
        .get("protocolVersion")
        .and_then(Value::as_str)
        .expect("initialize result.protocolVersion should be present");
    assert_eq!(
        protocol_version, "2025-03-26",
        "initialize should negotiate the server-supported MCP protocol version"
    );

    assert!(
        result
            .get("capabilities")
            .and_then(Value::as_object)
            .and_then(|caps| caps.get("tools"))
            .and_then(Value::as_object)
            .is_some(),
        "initialize result.capabilities.tools should be present and object-like: {initialize}"
    );

    let server_info = result
        .get("serverInfo")
        .and_then(Value::as_object)
        .expect("initialize result.serverInfo should be present");
    assert!(
        server_info
            .get("name")
            .and_then(Value::as_str)
            .is_some_and(|name| !name.trim().is_empty()),
        "initialize result.serverInfo.name should be a non-empty string: {initialize}"
    );
    assert!(
        server_info
            .get("version")
            .and_then(Value::as_str)
            .is_some_and(|version| !version.trim().is_empty()),
        "initialize result.serverInfo.version should be a non-empty string: {initialize}"
    );
    assert!(
        result
            .get("instructions")
            .and_then(Value::as_str)
            .is_some_and(|instructions| !instructions.trim().is_empty()),
        "initialize result.instructions should be present and non-empty: {initialize}"
    );
}

#[tokio::test]
async fn rust_mcp_container_negotiates_initialize_from_newer_protocol_version() {
    let rust_mcp = RustMcpTestContainer::start()
        .await
        .expect("failed to start rust-mcp container");
    rust_mcp
        .wait_until_ready(Duration::from_secs(120))
        .await
        .expect("container did not become ready");

    let requested_protocol_version = "2099-12-31";
    let client = reqwest::Client::new();
    let initialize_response = client
        .post(rust_mcp.mcp_url())
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": requested_protocol_version,
                "capabilities": {},
                "clientInfo": {"name": "rust-mcp-e2e", "version": "0.1.0"}
            }
        }))
        .send()
        .await
        .expect("raw initialize request failed");
    assert!(
        initialize_response
            .status()
            .is_success()
    );
    let initialize_content_type = initialize_response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let session_id = initialize_response
        .headers()
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
        .expect("initialize response did not include mcp-session-id header")
        .to_string();
    let initialize_body = initialize_response
        .text()
        .await
        .expect("failed to read initialize response body");
    let initialize_payload = parse_mcp_response(&initialize_content_type, &initialize_body);

    let negotiated_protocol_version = initialize_payload
        .get("result")
        .and_then(|result| result.get("protocolVersion"))
        .and_then(Value::as_str)
        .expect("initialize result.protocolVersion should be present");
    assert_ne!(
        negotiated_protocol_version, requested_protocol_version,
        "initialize should not echo unsupported client protocol version"
    );
    assert_eq!(
        negotiated_protocol_version, "2025-03-26",
        "initialize should negotiate to server-supported MCP protocol version"
    );

    let initialized_notification = client
        .post(rust_mcp.mcp_url())
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-session-id", &session_id)
        .json(&json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        }))
        .send()
        .await
        .expect("raw notifications/initialized failed");
    assert!(
        initialized_notification
            .status()
            .is_success()
    );

    let ping_response = client
        .post(rust_mcp.mcp_url())
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-session-id", &session_id)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "ping",
                "arguments": {"message": "negotiated-protocol"}
            }
        }))
        .send()
        .await
        .expect("raw tools/call ping request failed");
    assert!(
        ping_response
            .status()
            .is_success()
    );
    let ping_content_type = ping_response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let ping_body = ping_response
        .text()
        .await
        .expect("failed to read tools/call ping response body");
    let ping_payload = parse_mcp_response(&ping_content_type, &ping_body);
    assert!(
        ping_payload
            .get("result")
            .is_some()
    );
}

#[tokio::test]
async fn rust_mcp_container_supports_trivial_tool_calls() {
    let rust_mcp = RustMcpTestContainer::start()
        .await
        .expect("failed to start rust-mcp container");
    rust_mcp
        .wait_until_ready(Duration::from_secs(120))
        .await
        .expect("container did not become ready");

    let initialize = rust_mcp
        .initialize_mcp()
        .await
        .expect("MCP initialize failed");
    assert!(
        initialize
            .get("result")
            .is_some()
    );

    let calls: [(&str, Value); 5] = [
        ("ping", json!({ "message": "e2e-tool-coverage" })),
        ("index.status", json!({})),
        ("crate.search", json!({ "query": "serde", "limit": 5 })),
        ("symbol.search", json!({ "query": "serde", "limit": 5 })),
        ("docs.search", json!({ "query": "serde", "limit": 5 })),
    ];

    for (tool_name, arguments) in calls {
        let response = rust_mcp
            .call_tool(tool_name, arguments)
            .await
            .unwrap_or_else(|error| panic!("MCP tools/call {tool_name} failed: {error}"));

        let _ = tool_result(&response, tool_name);
    }
}

#[tokio::test]
async fn rust_mcp_container_supports_seeded_source_and_api_reads() {
    let mock_server = MockRegistryServer::start()
        .await
        .expect("failed to start mock crates/docs server");
    let rust_mcp = RustMcpTestContainer::start_with_env(mock_registry_env(&mock_server))
        .await
        .expect("failed to start rust-mcp container");
    rust_mcp
        .wait_until_ready(Duration::from_secs(120))
        .await
        .expect("container did not become ready");
    let _ = rust_mcp
        .initialize_mcp()
        .await
        .expect("MCP initialize failed");

    let sync = rust_mcp
        .call_tool(
            "index.sync_crates",
            json!({
                "query": SEEDED_CRATE_NAME,
                "page": 1,
                "per_page": 10,
                "include_dependencies": false
            }),
        )
        .await
        .expect("MCP tools/call index.sync_crates failed");
    let sync_payload =
        structured_content(tool_result(&sync, "index.sync_crates"), "index.sync_crates");
    let expected_selected = format!("{SEEDED_CRATE_NAME}@{SEEDED_CRATE_NEXT_VERSION}");
    assert_eq!(
        sync_payload
            .get("synced_crates")
            .and_then(Value::as_u64),
        Some(1)
    );
    assert!(
        sync_payload
            .get("selected_versions")
            .and_then(Value::as_array)
            .is_some_and(|versions| versions
                .iter()
                .filter_map(Value::as_str)
                .any(|version| version == expected_selected))
    );

    let refresh = rust_mcp
        .call_tool(
            "index.refresh",
            json!({
                "scope": "rustdoc_json",
                "crate_name": SEEDED_CRATE_NAME,
                "page": 1,
                "per_page": 10
            }),
        )
        .await
        .expect("MCP tools/call index.refresh failed");
    let refresh_payload =
        structured_content(tool_result(&refresh, "index.refresh"), "index.refresh");
    assert_eq!(
        refresh_payload
            .get("status")
            .and_then(Value::as_str),
        Some("completed")
    );

    let source_search = rust_mcp
        .call_tool(
            "source.search",
            json!({
                "query": "parse",
                "crate_name": SEEDED_CRATE_NAME,
                "version": SEEDED_CRATE_VERSION,
                "limit": 5
            }),
        )
        .await
        .expect("MCP tools/call source.search failed");
    let source_search_payload =
        structured_content(tool_result(&source_search, "source.search"), "source.search");
    assert!(
        source_search_payload
            .get("hits")
            .and_then(Value::as_array)
            .is_some_and(|hits| hits.iter().any(|hit| hit
                .get("path")
                .and_then(Value::as_str)
                == Some(SEEDED_RUSTDOC_PATH)))
    );

    let source_read = rust_mcp
        .call_tool(
            "source.read",
            json!({
                "crate_name": SEEDED_CRATE_NAME,
                "version": SEEDED_CRATE_VERSION,
                "path": SEEDED_RUSTDOC_PATH,
                "start_line": 1,
                "end_line": 200
            }),
        )
        .await
        .expect("MCP tools/call source.read failed");
    let source_read_payload =
        structured_content(tool_result(&source_read, "source.read"), "source.read");
    let source_content = source_read_payload
        .get("content")
        .and_then(Value::as_str)
        .expect("source.read content should be present");
    assert!(source_content.contains("parse"));

    let crate_api = rust_mcp
        .call_tool(
            "crate.api",
            json!({
                "crate_name": SEEDED_CRATE_NAME,
                "version": SEEDED_CRATE_VERSION,
                "kinds": ["function"],
                "limit": 10
            }),
        )
        .await
        .expect("MCP tools/call crate.api failed");
    let crate_api_payload = structured_content(tool_result(&crate_api, "crate.api"), "crate.api");
    assert!(
        crate_api_payload
            .get("symbols")
            .and_then(Value::as_array)
            .is_some_and(|symbols| symbols
                .iter()
                .any(|symbol| symbol
                    .get("name")
                    .and_then(Value::as_str)
                    == Some("parse")))
    );
}

#[tokio::test]
async fn rust_mcp_container_supports_tools_list_after_initialize() {
    let rust_mcp = RustMcpTestContainer::start()
        .await
        .expect("failed to start rust-mcp container");
    rust_mcp
        .wait_until_ready(Duration::from_secs(120))
        .await
        .expect("container did not become ready");
    let _ = rust_mcp
        .initialize_mcp()
        .await
        .expect("MCP initialize failed");

    let tools_list = rust_mcp
        .list_tools_mcp()
        .await
        .expect("MCP tools/list failed");
    let tools = tools_list
        .get("result")
        .and_then(|result| result.get("tools"))
        .and_then(Value::as_array)
        .expect("MCP tools/list did not include result.tools");
    let listed_names = tools
        .iter()
        .filter_map(|tool| tool.get("name"))
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
    let expected_names = [
        "ping",
        "index.sync_crates",
        "index.status",
        "index.refresh",
        "crate.search",
        "crate.intel",
        "crate.features",
        "crate.api_diff",
        "crate.api",
        "crate.type_info",
        "crate.trait_impls",
        "crate.re_exports",
        "crate.error_types",
        "crate.derive_macros",
        "crate.compare",
        "crate.compatibility",
        "crate.compatibility_matrix",
        "crate.migration_path",
        "crate.license_check",
        "crate.alternatives",
        "crate.versions",
        "crate.graph",
        "crate.hotspots",
        "dependency.audit",
        "dependency.resolve",
        "dependency.feature_impact",
        "source.search",
        "source.read",
        "source.context",
        "symbol.search",
        "docs.search",
        "crate.usage_patterns",
    ]
    .into_iter()
    .map(str::to_string)
    .collect::<BTreeSet<_>>();
    assert_eq!(
        listed_names, expected_names,
        "tools/list contract changed; update expected tool set if intentional"
    );
}

#[tokio::test]
async fn rust_mcp_container_tools_list_entries_include_descriptions_and_input_schemas() {
    let rust_mcp = RustMcpTestContainer::start()
        .await
        .expect("failed to start rust-mcp container");
    rust_mcp
        .wait_until_ready(Duration::from_secs(120))
        .await
        .expect("container did not become ready");
    let _ = rust_mcp
        .initialize_mcp()
        .await
        .expect("MCP initialize failed");

    let tools_list = rust_mcp
        .list_tools_mcp()
        .await
        .expect("MCP tools/list failed");
    let tools = tools_list
        .get("result")
        .and_then(|result| result.get("tools"))
        .and_then(Value::as_array)
        .expect("MCP tools/list did not include result.tools");
    assert!(!tools.is_empty(), "tools/list returned no tool entries");

    let listed_names = tools
        .iter()
        .filter_map(|tool| tool.get("name"))
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect::<Vec<_>>();
    let unique_names = listed_names
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    assert_eq!(
        listed_names.len(),
        unique_names.len(),
        "tools/list should not include duplicate tool names"
    );

    for tool in tools {
        let name = tool
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("tool entry missing name: {tool}"));
        assert!(!name.trim().is_empty(), "tool entry has empty name: {tool}");

        let description = tool
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("tool `{name}` missing description: {tool}"));
        assert!(!description.trim().is_empty(), "tool `{name}` has empty description");

        let input_schema = tool
            .get("inputSchema")
            .or_else(|| tool.get("input_schema"))
            .and_then(Value::as_object)
            .unwrap_or_else(|| panic!("tool `{name}` missing object input schema: {tool}"));
        assert!(
            input_schema.contains_key("type")
                || input_schema.contains_key("oneOf")
                || input_schema.contains_key("anyOf")
                || input_schema.contains_key("allOf"),
            "tool `{name}` input schema is missing expected schema keys: {input_schema:?}"
        );
        if let Some(schema_type) = input_schema
            .get("type")
            .and_then(Value::as_str)
        {
            assert_eq!(schema_type, "object", "tool `{name}` expected object input schema");
        }
    }
}

#[tokio::test]
async fn rust_mcp_container_rejects_tools_list_before_initialize() {
    let rust_mcp = RustMcpTestContainer::start()
        .await
        .expect("failed to start rust-mcp container");
    rust_mcp
        .wait_until_ready(Duration::from_secs(120))
        .await
        .expect("container did not become ready");

    let pre_initialize_tools_list = rust_mcp
        .list_tools_mcp()
        .await;
    assert!(
        pre_initialize_tools_list.is_err(),
        "tools/list unexpectedly succeeded before initialize: {pre_initialize_tools_list:?}"
    );
}

#[tokio::test]
async fn rust_mcp_container_requires_streamable_accept_for_mcp_requests() {
    let rust_mcp = RustMcpTestContainer::start()
        .await
        .expect("failed to start rust-mcp container");
    rust_mcp
        .wait_until_ready(Duration::from_secs(120))
        .await
        .expect("container did not become ready");

    let client = reqwest::Client::new();

    let json_only_initialize = client
        .post(rust_mcp.mcp_url())
        .header("content-type", "application/json")
        .header("accept", "application/json")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "rust-mcp-e2e", "version": "0.1.0"}
            }
        }))
        .send()
        .await
        .expect("raw initialize request with JSON-only accept failed");
    assert!(
        json_only_initialize
            .status()
            .is_client_error(),
        "expected client error for JSON-only accept, got {}",
        json_only_initialize.status()
    );

    let initialize_response = client
        .post(rust_mcp.mcp_url())
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "rust-mcp-e2e", "version": "0.1.0"}
            }
        }))
        .send()
        .await
        .expect("raw initialize request failed");
    assert!(
        initialize_response
            .status()
            .is_success()
    );
    let initialize_content_type = initialize_response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let session_id = initialize_response
        .headers()
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
        .expect("initialize response did not include mcp-session-id header")
        .to_string();
    let initialize_body = initialize_response
        .text()
        .await
        .expect("failed to read initialize response body");
    let initialize_payload = parse_mcp_response(&initialize_content_type, &initialize_body);
    assert!(
        initialize_payload
            .get("result")
            .is_some()
    );

    let initialized_notification = client
        .post(rust_mcp.mcp_url())
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-session-id", &session_id)
        .json(&json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        }))
        .send()
        .await
        .expect("raw notifications/initialized failed");
    assert!(
        initialized_notification
            .status()
            .is_success()
    );

    let tools_list_response = client
        .post(rust_mcp.mcp_url())
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-session-id", &session_id)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }))
        .send()
        .await
        .expect("raw tools/list request failed");
    assert!(
        tools_list_response
            .status()
            .is_success()
    );
    let tools_list_content_type = tools_list_response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let tools_list_body = tools_list_response
        .text()
        .await
        .expect("failed to read tools/list response body");
    let tools_list_payload = parse_mcp_response(&tools_list_content_type, &tools_list_body);
    assert!(
        tools_list_payload
            .get("result")
            .and_then(|result| result.get("tools"))
            .and_then(Value::as_array)
            .is_some_and(|tools| !tools.is_empty()),
        "tools/list returned unexpected payload: {tools_list_payload}"
    );
}

#[tokio::test]
async fn rust_mcp_container_rejects_initialized_notification_before_initialize() {
    let rust_mcp = RustMcpTestContainer::start()
        .await
        .expect("failed to start rust-mcp container");
    rust_mcp
        .wait_until_ready(Duration::from_secs(120))
        .await
        .expect("container did not become ready");

    let notification_response = reqwest::Client::new()
        .post(rust_mcp.mcp_url())
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .json(&json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        }))
        .send()
        .await
        .expect("raw notifications/initialized request failed");
    assert!(
        notification_response
            .status()
            .is_client_error(),
        "expected client error when notifying initialized before initialize, got {}",
        notification_response.status()
    );
}

#[tokio::test]
async fn rust_mcp_container_initialized_notification_returns_accepted_without_jsonrpc_payload() {
    let rust_mcp = RustMcpTestContainer::start()
        .await
        .expect("failed to start rust-mcp container");
    rust_mcp
        .wait_until_ready(Duration::from_secs(120))
        .await
        .expect("container did not become ready");

    let client = reqwest::Client::new();
    let initialize_response = client
        .post(rust_mcp.mcp_url())
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "rust-mcp-e2e", "version": "0.1.0"}
            }
        }))
        .send()
        .await
        .expect("raw initialize request failed");
    assert!(
        initialize_response
            .status()
            .is_success()
    );
    let session_id = initialize_response
        .headers()
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
        .expect("initialize response did not include mcp-session-id header")
        .to_string();

    let initialized_notification = client
        .post(rust_mcp.mcp_url())
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-session-id", &session_id)
        .json(&json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        }))
        .send()
        .await
        .expect("raw notifications/initialized failed");
    assert_eq!(
        initialized_notification.status(),
        StatusCode::ACCEPTED,
        "notifications/initialized should be accepted as a notification (no response payload)"
    );
    let body = initialized_notification
        .text()
        .await
        .expect("failed to read notifications/initialized response body");
    assert!(
        body.trim().is_empty(),
        "notifications/initialized should not return JSON-RPC payload body, got: {body}"
    );
}

#[tokio::test]
async fn rust_mcp_container_requires_initialized_notification_before_tool_calls() {
    let rust_mcp = RustMcpTestContainer::start()
        .await
        .expect("failed to start rust-mcp container");
    rust_mcp
        .wait_until_ready(Duration::from_secs(120))
        .await
        .expect("container did not become ready");

    let pre_initialize = rust_mcp
        .call_tool("ping", json!({ "message": "pre-init" }))
        .await;
    assert!(
        pre_initialize.is_err(),
        "tools/call ping unexpectedly succeeded before initialize: {pre_initialize:?}"
    );

    rust_mcp.clear_session().await;

    let initialize = rust_mcp
        .initialize_only_mcp()
        .await
        .expect("MCP initialize failed");
    assert!(
        initialize
            .get("result")
            .is_some()
    );

    let before_initialized_notification = rust_mcp
        .call_tool("ping", json!({ "message": "pre-notification" }))
        .await;
    assert!(
        before_initialized_notification.is_err(),
        "tools/call ping unexpectedly succeeded before notifications/initialized: \
         {before_initialized_notification:?}"
    );

    rust_mcp.clear_session().await;

    let _ = rust_mcp
        .initialize_mcp()
        .await
        .expect("MCP re-initialize failed");

    let ping = rust_mcp
        .call_tool("ping", json!({ "message": "post-init" }))
        .await
        .expect("MCP tools/call ping failed after notifications/initialized");
    assert!(ping.get("result").is_some());
}

#[tokio::test]
async fn rust_mcp_container_reports_invalid_params_for_tools_call() {
    let rust_mcp = RustMcpTestContainer::start()
        .await
        .expect("failed to start rust-mcp container");
    rust_mcp
        .wait_until_ready(Duration::from_secs(120))
        .await
        .expect("container did not become ready");

    let client = reqwest::Client::new();
    let initialize_response = client
        .post(rust_mcp.mcp_url())
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "rust-mcp-e2e", "version": "0.1.0"}
            }
        }))
        .send()
        .await
        .expect("raw initialize request failed");
    assert!(
        initialize_response
            .status()
            .is_success()
    );
    let session_id = initialize_response
        .headers()
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
        .expect("initialize response did not include mcp-session-id header")
        .to_string();

    let initialized_notification = client
        .post(rust_mcp.mcp_url())
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-session-id", &session_id)
        .json(&json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        }))
        .send()
        .await
        .expect("raw notifications/initialized failed");
    assert!(
        initialized_notification
            .status()
            .is_success()
    );

    let invalid_params_response = client
        .post(rust_mcp.mcp_url())
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-session-id", &session_id)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "arguments": {}
            }
        }))
        .send()
        .await
        .expect("raw tools/call invalid-params request failed");

    let status = invalid_params_response.status();
    let content_type = invalid_params_response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let body = invalid_params_response
        .text()
        .await
        .expect("failed to read invalid-params response body");

    if status.is_success() {
        let payload = parse_mcp_response(&content_type, &body);
        assert!(
            payload.get("error").is_some(),
            "expected JSON-RPC error payload for invalid params, got: {payload}"
        );
    } else {
        assert!(
            status.is_client_error(),
            "expected invalid params to produce client error or JSON-RPC error, got status \
             {status}"
        );
        assert!(
            body.to_ascii_lowercase()
                .contains("error")
                || body
                    .to_ascii_lowercase()
                    .contains("invalid")
                || body
                    .to_ascii_lowercase()
                    .contains("name"),
            "unexpected invalid params error body: {body}"
        );
    }
}

#[tokio::test]
async fn rust_mcp_container_reports_tool_validation_failure_as_error_result() {
    let rust_mcp = RustMcpTestContainer::start()
        .await
        .expect("failed to start rust-mcp container");
    rust_mcp
        .wait_until_ready(Duration::from_secs(120))
        .await
        .expect("container did not become ready");
    let _ = rust_mcp
        .initialize_mcp()
        .await
        .expect("MCP initialize failed");

    let invalid_tool_response = rust_mcp
        .call_tool(
            "dependency.feature_impact",
            json!({
                "crate_name": "serde",
                "features": []
            }),
        )
        .await
        .expect("tool validation failure should still return MCP tool result envelope");

    let result = invalid_tool_response
        .get("result")
        .expect("MCP tools/call dependency.feature_impact returned no result payload");
    let is_error = result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    assert!(
        is_error,
        "expected tool-level isError=true for invalid dependency.feature_impact request: {result}"
    );
}

#[tokio::test]
async fn rust_mcp_container_reports_error_for_unknown_tool_call() {
    let rust_mcp = RustMcpTestContainer::start()
        .await
        .expect("failed to start rust-mcp container");
    rust_mcp
        .wait_until_ready(Duration::from_secs(120))
        .await
        .expect("container did not become ready");
    let _ = rust_mcp
        .initialize_mcp()
        .await
        .expect("MCP initialize failed");

    let unknown_tool = rust_mcp
        .call_tool("tool.does_not_exist", json!({}))
        .await;
    assert!(unknown_tool.is_err(), "unknown tool unexpectedly succeeded: {unknown_tool:?}");
    let error = unknown_tool
        .expect_err("unknown tool should have failed")
        .to_string();
    assert!(
        error.contains("tools/call") && error.contains("error payload"),
        "unexpected unknown tool error payload: {error}"
    );
}

#[tokio::test]
async fn rust_mcp_container_reports_error_for_unknown_jsonrpc_method() {
    let rust_mcp = RustMcpTestContainer::start()
        .await
        .expect("failed to start rust-mcp container");
    rust_mcp
        .wait_until_ready(Duration::from_secs(120))
        .await
        .expect("container did not become ready");

    let client = reqwest::Client::new();
    let initialize_response = client
        .post(rust_mcp.mcp_url())
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "rust-mcp-e2e", "version": "0.1.0"}
            }
        }))
        .send()
        .await
        .expect("raw initialize request failed");
    assert!(
        initialize_response
            .status()
            .is_success()
    );
    let session_id = initialize_response
        .headers()
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
        .expect("initialize response did not include mcp-session-id header")
        .to_string();

    let initialized_notification = client
        .post(rust_mcp.mcp_url())
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-session-id", &session_id)
        .json(&json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        }))
        .send()
        .await
        .expect("raw notifications/initialized failed");
    assert!(
        initialized_notification
            .status()
            .is_success()
    );

    let unknown_method_response = client
        .post(rust_mcp.mcp_url())
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-session-id", &session_id)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "foo/bar_baz",
            "params": {}
        }))
        .send()
        .await
        .expect("raw unknown-method request failed");
    let status = unknown_method_response.status();
    let content_type = unknown_method_response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let body = unknown_method_response
        .text()
        .await
        .expect("failed to read unknown-method response body");

    if status.is_success() {
        let payload = parse_mcp_response(&content_type, &body);
        assert!(
            payload.get("error").is_some(),
            "expected JSON-RPC error payload for unknown method, got: {payload}"
        );
    } else {
        assert!(
            status.is_client_error(),
            "expected unknown method to produce client error or JSON-RPC error, got status \
             {status}"
        );
        assert!(
            body.to_ascii_lowercase()
                .contains("error")
                || body
                    .to_ascii_lowercase()
                    .contains("method"),
            "unexpected unknown method error body: {body}"
        );
    }
}

#[tokio::test]
async fn rust_mcp_container_preserves_jsonrpc_ids_across_initialize_and_tools_requests() {
    let rust_mcp = RustMcpTestContainer::start()
        .await
        .expect("failed to start rust-mcp container");
    rust_mcp
        .wait_until_ready(Duration::from_secs(120))
        .await
        .expect("container did not become ready");

    let initialize_id = 101_u64;
    let tools_list_id = 102_u64;
    let ping_id = 103_u64;
    let client = reqwest::Client::new();

    let initialize_response = client
        .post(rust_mcp.mcp_url())
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": initialize_id,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "rust-mcp-e2e", "version": "0.1.0"}
            }
        }))
        .send()
        .await
        .expect("raw initialize request failed");
    assert!(
        initialize_response
            .status()
            .is_success()
    );
    let initialize_content_type = initialize_response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let session_id = initialize_response
        .headers()
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
        .expect("initialize response did not include mcp-session-id header")
        .to_string();
    let initialize_body = initialize_response
        .text()
        .await
        .expect("failed to read initialize response body");
    let initialize_payload = parse_mcp_response(&initialize_content_type, &initialize_body);
    assert_eq!(initialize_payload.get("id"), Some(&json!(initialize_id)));
    assert!(
        initialize_payload
            .get("result")
            .is_some()
    );

    let initialized_notification = client
        .post(rust_mcp.mcp_url())
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-session-id", &session_id)
        .json(&json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        }))
        .send()
        .await
        .expect("raw notifications/initialized failed");
    assert!(
        initialized_notification
            .status()
            .is_success()
    );

    let tools_list_response = client
        .post(rust_mcp.mcp_url())
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-session-id", &session_id)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": tools_list_id,
            "method": "tools/list",
            "params": {}
        }))
        .send()
        .await
        .expect("raw tools/list request failed");
    assert!(
        tools_list_response
            .status()
            .is_success()
    );
    let tools_list_content_type = tools_list_response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let tools_list_body = tools_list_response
        .text()
        .await
        .expect("failed to read tools/list response body");
    let tools_list_payload = parse_mcp_response(&tools_list_content_type, &tools_list_body);
    assert_eq!(tools_list_payload.get("id"), Some(&json!(tools_list_id)));
    assert!(
        tools_list_payload
            .get("result")
            .and_then(|result| result.get("tools"))
            .and_then(Value::as_array)
            .is_some_and(|tools| !tools.is_empty())
    );

    let ping_response = client
        .post(rust_mcp.mcp_url())
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-session-id", &session_id)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": ping_id,
            "method": "tools/call",
            "params": {
                "name": "ping",
                "arguments": {"message": "id-correlation"}
            }
        }))
        .send()
        .await
        .expect("raw tools/call ping request failed");
    assert!(
        ping_response
            .status()
            .is_success()
    );
    let ping_content_type = ping_response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let ping_body = ping_response
        .text()
        .await
        .expect("failed to read tools/call ping response body");
    let ping_payload = parse_mcp_response(&ping_content_type, &ping_body);
    assert_eq!(ping_payload.get("id"), Some(&json!(ping_id)));
    assert!(
        ping_payload
            .get("result")
            .is_some()
    );
}

#[tokio::test]
async fn rust_mcp_container_supports_concurrent_requests_on_same_session() {
    let rust_mcp = RustMcpTestContainer::start()
        .await
        .expect("failed to start rust-mcp container");
    rust_mcp
        .wait_until_ready(Duration::from_secs(120))
        .await
        .expect("container did not become ready");

    let client = reqwest::Client::new();
    let initialize_response = client
        .post(rust_mcp.mcp_url())
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "rust-mcp-e2e", "version": "0.1.0"}
            }
        }))
        .send()
        .await
        .expect("raw initialize request failed");
    assert!(
        initialize_response
            .status()
            .is_success()
    );
    let session_id = initialize_response
        .headers()
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
        .expect("initialize response did not include mcp-session-id header")
        .to_string();

    let initialized_notification = client
        .post(rust_mcp.mcp_url())
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-session-id", &session_id)
        .json(&json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        }))
        .send()
        .await
        .expect("raw notifications/initialized failed");
    assert!(
        initialized_notification
            .status()
            .is_success()
    );

    let tools_list_id = 21_u64;
    let ping_id = 22_u64;
    let tools_list_request = client
        .post(rust_mcp.mcp_url())
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-session-id", &session_id)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": tools_list_id,
            "method": "tools/list",
            "params": {}
        }))
        .send();
    let ping_request = client
        .post(rust_mcp.mcp_url())
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-session-id", &session_id)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": ping_id,
            "method": "tools/call",
            "params": {
                "name": "ping",
                "arguments": {"message": "concurrent"}
            }
        }))
        .send();
    let (tools_list_response, ping_response) = tokio::join!(tools_list_request, ping_request);

    let tools_list_response = tools_list_response.expect("concurrent tools/list request failed");
    assert!(
        tools_list_response
            .status()
            .is_success()
    );
    let tools_list_content_type = tools_list_response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let tools_list_body = tools_list_response
        .text()
        .await
        .expect("failed to read concurrent tools/list response body");
    let tools_list_payload = parse_mcp_response(&tools_list_content_type, &tools_list_body);
    assert_eq!(tools_list_payload.get("id"), Some(&json!(tools_list_id)));
    assert!(
        tools_list_payload
            .get("result")
            .and_then(|result| result.get("tools"))
            .and_then(Value::as_array)
            .is_some_and(|tools| !tools.is_empty())
    );

    let ping_response = ping_response.expect("concurrent ping request failed");
    assert!(
        ping_response
            .status()
            .is_success()
    );
    let ping_content_type = ping_response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let ping_body = ping_response
        .text()
        .await
        .expect("failed to read concurrent ping response body");
    let ping_payload = parse_mcp_response(&ping_content_type, &ping_body);
    assert_eq!(ping_payload.get("id"), Some(&json!(ping_id)));
    assert!(
        ping_payload
            .get("result")
            .is_some()
    );
}

#[tokio::test]
async fn rust_mcp_container_rejects_jsonrpc_batch_requests() {
    let rust_mcp = RustMcpTestContainer::start()
        .await
        .expect("failed to start rust-mcp container");
    rust_mcp
        .wait_until_ready(Duration::from_secs(120))
        .await
        .expect("container did not become ready");

    let response = reqwest::Client::new()
        .post(rust_mcp.mcp_url())
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .json(&json!([
            {
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-11-25",
                    "capabilities": {},
                    "clientInfo": {"name": "rust-mcp-e2e", "version": "0.1.0"}
                }
            },
            {
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/list",
                "params": {}
            }
        ]))
        .send()
        .await
        .expect("raw batch request failed");

    let status = response.status();
    let body = response
        .text()
        .await
        .expect("failed to read batch request response body");
    assert!(
        status.is_client_error(),
        "expected client error for JSON-RPC batch request, got {status}"
    );
    assert!(
        body.to_ascii_lowercase()
            .contains("deserialize")
            || body
                .to_ascii_lowercase()
                .contains("unexpected"),
        "unexpected batch rejection body: {body}"
    );
}

#[tokio::test]
async fn rust_mcp_container_emits_progress_notifications_for_progress_tokenized_tools_call() {
    let mock_server = MockRegistryServer::start()
        .await
        .expect("failed to start mock crates/docs server");
    let rust_mcp = RustMcpTestContainer::start_with_env(mock_registry_env(&mock_server))
        .await
        .expect("failed to start rust-mcp container");
    rust_mcp
        .wait_until_ready(Duration::from_secs(120))
        .await
        .expect("container did not become ready");

    let client = reqwest::Client::new();
    let initialize_response = client
        .post(rust_mcp.mcp_url())
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "rust-mcp-e2e", "version": "0.1.0"}
            }
        }))
        .send()
        .await
        .expect("raw initialize request failed");
    assert!(
        initialize_response
            .status()
            .is_success()
    );
    let session_id = initialize_response
        .headers()
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
        .expect("initialize response did not include mcp-session-id header")
        .to_string();

    let initialized_notification = client
        .post(rust_mcp.mcp_url())
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-session-id", &session_id)
        .json(&json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        }))
        .send()
        .await
        .expect("raw notifications/initialized failed");
    assert!(
        initialized_notification
            .status()
            .is_success()
    );

    let request_id = 2_u64;
    let progress_token = "e2e-progress-sync";
    let tools_call = client
        .post(rust_mcp.mcp_url())
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-session-id", &session_id)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "tools/call",
            "params": {
                "name": "index.sync_crates",
                "_meta": {
                    "progressToken": progress_token
                },
                "arguments": {
                    "query": SEEDED_CRATE_NAME,
                    "page": 1,
                    "per_page": 10,
                    "include_dependencies": false
                }
            }
        }))
        .send()
        .await
        .expect("raw tools/call index.sync_crates request failed");
    assert!(
        tools_call
            .status()
            .is_success()
    );

    let content_type = tools_call
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let body = tools_call
        .text()
        .await
        .expect("failed to read tools/call index.sync_crates response body");
    let events = parse_mcp_response_events(&content_type, &body);

    let progress_events = events
        .iter()
        .filter(|event| {
            event
                .get("method")
                .and_then(Value::as_str)
                == Some("notifications/progress")
        })
        .filter(|event| {
            event
                .get("params")
                .and_then(|params| params.get("progressToken"))
                .and_then(Value::as_str)
                == Some(progress_token)
        })
        .collect::<Vec<_>>();
    assert!(
        !progress_events.is_empty(),
        "expected at least one notifications/progress event with matching progress token; got \
         events: {events:?}"
    );
    assert!(
        progress_events
            .iter()
            .any(|event| {
                event
                    .get("params")
                    .and_then(|params| params.get("progress"))
                    .and_then(Value::as_f64)
                    .is_some_and(|value| (value - 1.0).abs() < f64::EPSILON)
            }),
        "expected a completion progress event (progress=1.0); got events: {events:?}"
    );

    let final_response = events
        .iter()
        .find(|event| event.get("id") == Some(&json!(request_id)))
        .unwrap_or_else(|| {
            panic!("missing JSON-RPC response event for tools/call id {request_id}")
        });
    assert!(
        final_response
            .get("result")
            .is_some(),
        "tools/call response was missing result payload: {final_response}"
    );
}

#[tokio::test]
async fn rust_mcp_container_closes_session_via_delete_and_rejects_follow_up_requests() {
    let rust_mcp = RustMcpTestContainer::start()
        .await
        .expect("failed to start rust-mcp container");
    rust_mcp
        .wait_until_ready(Duration::from_secs(120))
        .await
        .expect("container did not become ready");

    let client = reqwest::Client::new();
    let initialize_response = client
        .post(rust_mcp.mcp_url())
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "rust-mcp-e2e", "version": "0.1.0"}
            }
        }))
        .send()
        .await
        .expect("raw initialize request failed");
    assert!(
        initialize_response
            .status()
            .is_success()
    );
    let session_id = initialize_response
        .headers()
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
        .expect("initialize response did not include mcp-session-id header")
        .to_string();

    let initialized_notification = client
        .post(rust_mcp.mcp_url())
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-session-id", &session_id)
        .json(&json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        }))
        .send()
        .await
        .expect("raw notifications/initialized failed");
    assert!(
        initialized_notification
            .status()
            .is_success()
    );

    let ping_before_close = client
        .post(rust_mcp.mcp_url())
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-session-id", &session_id)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "ping",
                "arguments": {"message": "before-close"}
            }
        }))
        .send()
        .await
        .expect("raw tools/call ping request failed before close");
    assert!(
        ping_before_close
            .status()
            .is_success()
    );

    let close_session_response = client
        .delete(rust_mcp.mcp_url())
        .header("accept", "application/json, text/event-stream")
        .header("mcp-session-id", &session_id)
        .send()
        .await
        .expect("DELETE /mcp close-session request failed");
    assert_eq!(
        close_session_response.status(),
        StatusCode::ACCEPTED,
        "expected DELETE /mcp to acknowledge session termination"
    );

    let post_close_tools_list = client
        .post(rust_mcp.mcp_url())
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-session-id", &session_id)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/list",
            "params": {}
        }))
        .send()
        .await
        .expect("raw tools/list request failed after session close");
    assert_eq!(
        post_close_tools_list.status(),
        StatusCode::UNAUTHORIZED,
        "expected closed session id to be rejected by subsequent requests"
    );
}

#[tokio::test]
async fn rust_mcp_container_rejects_unknown_session_id_header() {
    let rust_mcp = RustMcpTestContainer::start()
        .await
        .expect("failed to start rust-mcp container");
    rust_mcp
        .wait_until_ready(Duration::from_secs(120))
        .await
        .expect("container did not become ready");

    let client = reqwest::Client::new();
    let initialize_response = client
        .post(rust_mcp.mcp_url())
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "rust-mcp-e2e", "version": "0.1.0"}
            }
        }))
        .send()
        .await
        .expect("raw initialize request failed");
    assert!(
        initialize_response
            .status()
            .is_success()
    );
    let initialize_content_type = initialize_response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let session_id = initialize_response
        .headers()
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
        .expect("initialize response did not include mcp-session-id header")
        .to_string();
    let initialize_body = initialize_response
        .text()
        .await
        .expect("failed to read initialize response body");
    let initialize_payload = parse_mcp_response(&initialize_content_type, &initialize_body);
    assert!(
        initialize_payload
            .get("result")
            .is_some()
    );

    let initialized_notification = client
        .post(rust_mcp.mcp_url())
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-session-id", &session_id)
        .json(&json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        }))
        .send()
        .await
        .expect("raw notifications/initialized failed");
    assert!(
        initialized_notification
            .status()
            .is_success()
    );

    let unknown_session_response = client
        .post(rust_mcp.mcp_url())
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-session-id", format!("{session_id}-unknown"))
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "ping",
                "arguments": {"message": "wrong-session"}
            }
        }))
        .send()
        .await
        .expect("raw tools/call request with unknown session failed");
    assert_eq!(
        unknown_session_response.status(),
        StatusCode::UNAUTHORIZED,
        "expected unauthorized status for unknown session id"
    );

    let ping_response = client
        .post(rust_mcp.mcp_url())
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-session-id", &session_id)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "ping",
                "arguments": {"message": "good-session"}
            }
        }))
        .send()
        .await
        .expect("raw tools/call request with valid session failed");
    assert!(
        ping_response
            .status()
            .is_success()
    );
    let ping_content_type = ping_response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let ping_body = ping_response
        .text()
        .await
        .expect("failed to read tools/call response body");
    let ping_payload = parse_mcp_response(&ping_content_type, &ping_body);
    assert!(
        ping_payload
            .get("result")
            .is_some()
    );
}

#[tokio::test]
async fn rust_mcp_container_exports_tool_metrics_after_calls() {
    let rust_mcp = RustMcpTestContainer::start()
        .await
        .expect("failed to start rust-mcp container");
    rust_mcp
        .wait_until_ready(Duration::from_secs(120))
        .await
        .expect("container did not become ready");
    let _ = rust_mcp
        .initialize_mcp()
        .await
        .expect("MCP initialize failed");
    let _ = rust_mcp
        .call_tool("ping", json!({ "message": "metrics" }))
        .await
        .expect("MCP tools/call ping failed");
    let _ = rust_mcp
        .call_tool("index.status", json!({}))
        .await
        .expect("MCP tools/call index.status failed");

    let metrics = reqwest::Client::new()
        .get(format!("{}/metrics", rust_mcp.metrics_url()))
        .send()
        .await
        .expect("metrics endpoint request failed");
    assert!(metrics.status().is_success());
    let body = metrics
        .text()
        .await
        .expect("failed to read metrics body");
    assert!(body.contains("rust_mcp_tool_invocations_total"));
    assert!(body.contains("tool=\"index.status\""));
    assert!(body.contains("rust_mcp_tool_latency_ms"));
}

#[tokio::test]
async fn rust_mcp_container_supports_seeded_extended_tool_calls() {
    let mock_server = MockRegistryServer::start()
        .await
        .expect("failed to start mock crates/docs server");
    let manifest_fixture =
        ManifestFixtureMount::create().expect("failed to create manifest fixture");
    let rust_mcp = RustMcpTestContainer::start_with_env_and_mounts(
        mock_registry_env(&mock_server),
        vec![manifest_fixture.bind_mount()],
    )
    .await
    .expect("failed to start rust-mcp container");
    rust_mcp
        .wait_until_ready(Duration::from_secs(120))
        .await
        .expect("container did not become ready");
    let _ = rust_mcp
        .initialize_mcp()
        .await
        .expect("MCP initialize failed");

    let sync = rust_mcp
        .call_tool(
            "index.sync_crates",
            json!({
                "query": "demo",
                "page": 1,
                "per_page": 10,
                "include_dependencies": true
            }),
        )
        .await
        .expect("MCP tools/call index.sync_crates failed");
    let sync_payload =
        structured_content(tool_result(&sync, "index.sync_crates"), "index.sync_crates");
    assert!(
        sync_payload
            .get("synced_crates")
            .and_then(Value::as_u64)
            .is_some_and(|count| count >= 2)
    );

    let refresh = rust_mcp
        .call_tool(
            "index.refresh",
            json!({
                "scope": "rustdoc_json",
                "page": 1,
                "per_page": 20
            }),
        )
        .await
        .expect("MCP tools/call index.refresh failed");
    let refresh_payload =
        structured_content(tool_result(&refresh, "index.refresh"), "index.refresh");
    assert!(
        matches!(
            refresh_payload
                .get("status")
                .and_then(Value::as_str),
            Some("completed") | Some("completed_with_errors")
        ),
        "unexpected index.refresh status payload: {refresh_payload}"
    );

    let calls: [(&str, Value); 22] = [
        ("crate.alternatives", json!({ "crate_name": SEEDED_CRATE_NAME, "limit": 5 })),
        (
            "crate.api_diff",
            json!({
                "crate_name": SEEDED_CRATE_NAME,
                "from_version": SEEDED_CRATE_VERSION,
                "to_version": SEEDED_CRATE_NEXT_VERSION,
                "limit": 20
            }),
        ),
        (
            "crate.compare",
            json!({
                "left_crate": SEEDED_CRATE_NAME,
                "right_crate": SEEDED_ALT_CRATE_NAME
            }),
        ),
        (
            "crate.compatibility",
            json!({
                "left_crate": SEEDED_CRATE_NAME,
                "right_crate": SEEDED_ALT_CRATE_NAME
            }),
        ),
        (
            "crate.compatibility_matrix",
            json!({
                "left_crate": SEEDED_CRATE_NAME,
                "right_crate": SEEDED_ALT_CRATE_NAME,
                "left_versions": [SEEDED_CRATE_VERSION, SEEDED_CRATE_NEXT_VERSION],
                "right_versions": [SEEDED_ALT_CRATE_VERSION],
                "max_pairs": 6
            }),
        ),
        (
            "crate.derive_macros",
            json!({ "crate_name": SEEDED_CRATE_NAME, "version": SEEDED_CRATE_NEXT_VERSION }),
        ),
        (
            "crate.error_types",
            json!({ "crate_name": SEEDED_CRATE_NAME, "version": SEEDED_CRATE_NEXT_VERSION, "limit": 20 }),
        ),
        (
            "crate.features",
            json!({ "crate_name": SEEDED_CRATE_NAME, "version": SEEDED_CRATE_NEXT_VERSION }),
        ),
        (
            "crate.graph",
            json!({
                "crate_name": SEEDED_CRATE_NAME,
                "version": SEEDED_CRATE_NEXT_VERSION,
                "direction": "dependencies",
                "depth": 2
            }),
        ),
        (
            "crate.hotspots",
            json!({
                "crate_name": SEEDED_CRATE_NAME,
                "version": SEEDED_CRATE_NEXT_VERSION,
                "limit": 20
            }),
        ),
        (
            "crate.intel",
            json!({ "crate_name": SEEDED_CRATE_NAME, "version": SEEDED_CRATE_NEXT_VERSION }),
        ),
        (
            "crate.license_check",
            json!({ "crate_name": SEEDED_CRATE_NAME, "version": SEEDED_CRATE_NEXT_VERSION }),
        ),
        (
            "crate.migration_path",
            json!({
                "crate_name": SEEDED_CRATE_NAME,
                "from_version": SEEDED_CRATE_VERSION,
                "to_version": SEEDED_CRATE_NEXT_VERSION,
                "limit": 20
            }),
        ),
        (
            "crate.re_exports",
            json!({ "crate_name": SEEDED_CRATE_NAME, "version": SEEDED_CRATE_NEXT_VERSION, "limit": 20 }),
        ),
        (
            "crate.trait_impls",
            json!({
                "crate_name": SEEDED_CRATE_NAME,
                "version": SEEDED_CRATE_NEXT_VERSION,
                "type_name": "Parser",
                "limit": 20
            }),
        ),
        (
            "crate.type_info",
            json!({
                "crate_name": SEEDED_CRATE_NAME,
                "version": SEEDED_CRATE_NEXT_VERSION,
                "type_name": "Parser"
            }),
        ),
        (
            "crate.usage_patterns",
            json!({
                "crate_name": SEEDED_CRATE_NAME,
                "version": SEEDED_CRATE_NEXT_VERSION,
                "symbol_name": "parse",
                "limit": 20
            }),
        ),
        ("crate.versions", json!({ "crate_name": SEEDED_CRATE_NAME, "limit": 20 })),
        (
            "dependency.audit",
            json!({ "cargo_toml_path": manifest_fixture.container_manifest_path() }),
        ),
        (
            "dependency.feature_impact",
            json!({
                "crate_name": SEEDED_CRATE_NAME,
                "version": SEEDED_CRATE_NEXT_VERSION,
                "features": ["std"],
                "heavy_threshold": 1
            }),
        ),
        (
            "dependency.resolve",
            json!({
                "dependencies": [
                    {"name": SEEDED_CRATE_NAME, "version_req": "^1.2"},
                    {"name": SEEDED_ALT_CRATE_NAME, "version_req": "^0.9"}
                ],
                "check_features": true,
                "limit": 5
            }),
        ),
        (
            "source.context",
            json!({
                "crate_name": SEEDED_CRATE_NAME,
                "version": SEEDED_CRATE_VERSION,
                "path": SEEDED_RUSTDOC_PATH,
                "line": 1
            }),
        ),
    ];

    for (tool_name, arguments) in calls {
        let response = rust_mcp
            .call_tool(tool_name, arguments)
            .await
            .unwrap_or_else(|error| panic!("MCP tools/call {tool_name} failed: {error}"));
        let payload = structured_content(tool_result(&response, tool_name), tool_name);
        assert!(
            payload.is_object(),
            "MCP tools/call {tool_name} returned unexpected structured content: {payload}"
        );

        match tool_name {
            "crate.api_diff" => assert!(
                payload
                    .get("changes")
                    .is_some()
            ),
            "crate.compare" => assert!(
                payload
                    .get("recommendation_reasons")
                    .is_some()
            ),
            "crate.compatibility" => assert!(
                payload
                    .get("resolvable")
                    .is_some()
            ),
            "crate.compatibility_matrix" => assert!(
                payload
                    .get("pairs_tested")
                    .is_some()
            ),
            "crate.graph" => assert!(payload.get("edges").is_some()),
            "crate.hotspots" => assert!(
                payload
                    .get("hotspots")
                    .is_some()
            ),
            "crate.migration_path" => assert!(
                payload
                    .get("migration_actions")
                    .is_some()
            ),
            "crate.type_info" => assert!(
                payload
                    .get("type_definition")
                    .is_some()
            ),
            "crate.trait_impls" => assert!(payload.get("impls").is_some()),
            "crate.usage_patterns" => assert!(
                payload
                    .get("patterns")
                    .is_some()
            ),
            "dependency.audit" => assert!(
                payload
                    .get("issues")
                    .is_some()
            ),
            "dependency.feature_impact" => assert!(
                payload
                    .get("per_feature")
                    .is_some()
            ),
            "dependency.resolve" => assert!(
                payload
                    .get("resolved_versions")
                    .is_some()
            ),
            "source.context" => assert!(
                payload
                    .get("module_path")
                    .is_some()
            ),
            _ => {}
        }
    }
}
