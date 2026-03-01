//! crates.io/docs.rs mock server fixtures for e2e MCP tests.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use axum::extract::{Path, Query};
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use super::docs_rs_fixtures::mock_rustdoc_json_gzip;

pub const SEEDED_CRATE_NAME: &str = "demo-crate";
pub const SEEDED_CRATE_VERSION: &str = "1.2.3";
pub const SEEDED_CRATE_NEXT_VERSION: &str = "1.3.0";
pub const SEEDED_ALT_CRATE_NAME: &str = "demo-alt";
pub const SEEDED_ALT_CRATE_VERSION: &str = "0.9.0";
pub const SEEDED_TOKIO_CRATE_NAME: &str = "tokio";
pub const SEEDED_TOKIO_CRATE_VERSION: &str = "1.49.0";
const SEEDED_AUDIT_MOUNT_PATH: &str = "/e2e-fixtures";

struct DocsHtmlFixture {
    function_page: &'static str,
    title: &'static str,
    function_title: &'static str,
    function_text: &'static str,
}

pub struct MockRegistryServer {
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
    pub async fn start() -> Result<Self> {
        let router = Router::new()
            .route("/api/v1/crates", get(mock_search_crates))
            .route("/api/v1/crates/{crate_name}", get(mock_crate_detail))
            .route("/api/v1/crates/{crate_name}/{version}/readme", get(mock_crate_readme))
            .route(
                "/api/v1/crates/{crate_name}/{version}/dependencies",
                get(mock_crate_dependencies),
            )
            .route("/crate/{crate_name}/{version}/json.gz", get(mock_rustdoc_json_gzip))
            .route("/{crate_name}/{version}/{module}/", get(mock_docs_module_root_page))
            .route("/{crate_name}/{version}/{module}/{doc_page}", get(mock_docs_module_subpage));

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

pub struct ManifestFixtureMount {
    host_dir: PathBuf,
    container_manifest_path: String,
}

impl ManifestFixtureMount {
    pub fn create() -> Result<Self> {
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

    pub fn bind_mount(&self) -> (String, String) {
        (
            self.host_dir
                .to_string_lossy()
                .to_string(),
            SEEDED_AUDIT_MOUNT_PATH.to_string(),
        )
    }

    pub fn container_manifest_path(&self) -> &str {
        &self.container_manifest_path
    }
}

impl Drop for ManifestFixtureMount {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.host_dir);
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
    let candidates = [SEEDED_CRATE_NAME, SEEDED_ALT_CRATE_NAME, SEEDED_TOKIO_CRATE_NAME];
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
        "meta": {"total": 3}
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
        SEEDED_TOKIO_CRATE_NAME => json!({
            "crate": {
                "name": SEEDED_TOKIO_CRATE_NAME,
                "description": "E2E seeded tokio fixture crate",
                "repository": "https://example.test/tokio",
                "documentation": "https://docs.rs/tokio",
                "homepage": "https://example.test/tokio",
                "max_version": SEEDED_TOKIO_CRATE_VERSION
            },
            "versions": [{
                "num": SEEDED_TOKIO_CRATE_VERSION,
                "created_at": "2026-01-04T00:00:00Z",
                "updated_at": "2026-01-04T00:00:00Z",
                "yanked": false,
                "downloads": 1_000_000,
                "checksum": "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
                "rust_version": "1.75",
                "license": "MIT",
                "features": {
                    "default": ["rt"],
                    "rt": []
                }
            }],
            "keywords": [{
                "id": "kw-async",
                "keyword": "async"
            }],
            "categories": [{
                "id": "cat-async",
                "slug": "asynchronous",
                "category": "Asynchronous"
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
        (SEEDED_TOKIO_CRATE_NAME, SEEDED_TOKIO_CRATE_VERSION) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            "# tokio\nfixture readme",
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
        (SEEDED_TOKIO_CRATE_NAME, SEEDED_TOKIO_CRATE_VERSION) => json!({
            "dependencies": []
        }),
        _ => return (StatusCode::NOT_FOUND, Json(json!({ "dependencies": [] }))).into_response(),
    };

    (StatusCode::OK, Json(payload)).into_response()
}

fn docs_html_fixture(crate_name: &str, version: &str) -> Option<DocsHtmlFixture> {
    match (crate_name, version) {
        (SEEDED_CRATE_NAME, SEEDED_CRATE_VERSION) => Some(DocsHtmlFixture {
            function_page: "fn.parse.html",
            title: "demo-crate 1.2.3 docs",
            function_title: "parse - demo-crate",
            function_text: "Parse fixture input from docs page coverage",
        }),
        (SEEDED_CRATE_NAME, SEEDED_CRATE_NEXT_VERSION) => Some(DocsHtmlFixture {
            function_page: "fn.parse_input.html",
            title: "demo-crate 1.3.0 docs",
            function_title: "parse_input - demo-crate",
            function_text: "Parse fixture input with richer behavior",
        }),
        (SEEDED_ALT_CRATE_NAME, SEEDED_ALT_CRATE_VERSION) => Some(DocsHtmlFixture {
            function_page: "fn.call_demo_parse.html",
            title: "demo-alt 0.9.0 docs",
            function_title: "call_demo_parse - demo-alt",
            function_text: "Calls parse from demo-crate in fixture docs",
        }),
        _ => None,
    }
}

fn docs_module_html(fixture: &DocsHtmlFixture) -> String {
    format!(
        "<html><head><title>{}</title></head><body><h1>{}</h1><p>Parse docs fixture index \
         page</p><a href=\"{}\">parse</a></body></html>",
        fixture.title, fixture.title, fixture.function_page
    )
}

fn docs_function_html(fixture: &DocsHtmlFixture) -> String {
    format!(
        "<html><head><title>{}</title></head><body><h1>{}</h1><p>{}</p></body></html>",
        fixture.function_title, fixture.function_title, fixture.function_text
    )
}

async fn mock_docs_module_root_page(
    Path((crate_name, version, module)): Path<(String, String, String)>,
) -> impl IntoResponse {
    if module != crate_name {
        return StatusCode::NOT_FOUND.into_response();
    }
    let fixture = match docs_html_fixture(&crate_name, &version) {
        Some(fixture) => fixture,
        None => return StatusCode::NOT_FOUND.into_response(),
    };

    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        docs_module_html(&fixture),
    )
        .into_response()
}

async fn mock_docs_module_subpage(
    Path((crate_name, version, module, doc_page)): Path<(String, String, String, String)>,
) -> impl IntoResponse {
    if module != crate_name {
        return StatusCode::NOT_FOUND.into_response();
    }
    let fixture = match docs_html_fixture(&crate_name, &version) {
        Some(fixture) => fixture,
        None => return StatusCode::NOT_FOUND.into_response(),
    };

    if doc_page == "index.html" {
        return (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            docs_module_html(&fixture),
        )
            .into_response();
    }
    if doc_page == fixture.function_page {
        return (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            docs_function_html(&fixture),
        )
            .into_response();
    }

    StatusCode::NOT_FOUND.into_response()
}

pub fn mock_registry_env(mock_server: &MockRegistryServer) -> Vec<(String, String)> {
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
