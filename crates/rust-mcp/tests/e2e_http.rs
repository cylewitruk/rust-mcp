//! Docker image E2E smoke tests for rust-mcp HTTP endpoints.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::Duration;

use anyhow::Result;
use axum::extract::Path;
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
const SEEDED_RUSTDOC_PATH: &str = "rustdoc-json/docs.rs/demo-crate-1.2.3.json";

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

fn build_rustdoc_fixture(crate_name: &str, crate_version: &str) -> RustdocCrate {
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
            name: Some("parse".to_string()),
            span: None,
            visibility: RustdocVisibility::Public,
            docs: Some("Parse fixture input".to_string()),
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
            path: vec![crate_name.to_string(), "parse".to_string()],
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

async fn mock_search_crates() -> Json<Value> {
    Json(json!({
        "crates": [{
            "id": SEEDED_CRATE_NAME,
            "name": SEEDED_CRATE_NAME
        }],
        "meta": {"total": 1}
    }))
}

async fn mock_crate_detail(Path(crate_name): Path<String>) -> impl IntoResponse {
    if crate_name != SEEDED_CRATE_NAME {
        return StatusCode::NOT_FOUND.into_response();
    }

    (
        StatusCode::OK,
        Json(json!({
            "crate": {
                "name": SEEDED_CRATE_NAME,
                "description": "E2E seeded crate fixture",
                "repository": "https://example.test/demo-crate",
                "documentation": "https://docs.rs/demo-crate",
                "homepage": "https://example.test",
                "max_version": SEEDED_CRATE_VERSION
            },
            "versions": [{
                "num": SEEDED_CRATE_VERSION,
                "created_at": "2026-01-01T00:00:00Z",
                "updated_at": "2026-01-01T00:00:00Z",
                "yanked": false,
                "downloads": 123,
                "checksum": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "rust_version": "1.75",
                "license": "MIT OR Apache-2.0",
                "features": {
                    "default": []
                }
            }],
            "keywords": [{
                "id": "kw-demo",
                "keyword": "demo"
            }],
            "categories": [{
                "id": "cat-devtools",
                "slug": "development-tools",
                "category": "Development tools"
            }]
        })),
    )
        .into_response()
}

async fn mock_crate_readme(
    Path((crate_name, version)): Path<(String, String)>,
) -> impl IntoResponse {
    if crate_name != SEEDED_CRATE_NAME || version != SEEDED_CRATE_VERSION {
        return StatusCode::NOT_FOUND.into_response();
    }

    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        "# demo-crate\nfixture readme",
    )
        .into_response()
}

async fn mock_crate_dependencies(
    Path((crate_name, version)): Path<(String, String)>,
) -> impl IntoResponse {
    if crate_name != SEEDED_CRATE_NAME || version != SEEDED_CRATE_VERSION {
        return (StatusCode::NOT_FOUND, Json(json!({ "dependencies": [] }))).into_response();
    }

    (StatusCode::OK, Json(json!({ "dependencies": [] }))).into_response()
}

async fn mock_rustdoc_json_gzip(
    Path((crate_name, version)): Path<(String, String)>,
) -> impl IntoResponse {
    if crate_name != SEEDED_CRATE_NAME || version != SEEDED_CRATE_VERSION {
        return StatusCode::NOT_FOUND.into_response();
    }

    let rustdoc = build_rustdoc_fixture("demo_crate", SEEDED_CRATE_VERSION);
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
    let rust_mcp = RustMcpTestContainer::start_with_env(vec![
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
    ])
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
                .any(|version| version == "demo-crate@1.2.3"))
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
