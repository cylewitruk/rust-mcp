//! Integration tests for `index.*` MCP tools.

mod common;

use std::collections::HashMap;
use std::io::Write;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use axum::extract::Path;
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use flate2::Compression;
use flate2::write::GzEncoder;
use rust_mcp::config::{Config, LogFormat, TransportMode};
use rust_mcp::http;
use rust_mcp::mcp::{run_refresh_worker_for_tests, run_startup_rustdoc_json_refresh_for_tests};
use rust_mcp::state::AppState;
use rust_mcp_testing::fixtures::seed_crate_release;
use rust_mcp_testing::local_mcp::LocalMcpHttpHarness;
use rust_mcp_testing::postgres::PostgresTestContainer;
use rustdoc_types::{
    Abi, Crate as RustdocCrate, Function, FunctionHeader, FunctionSignature, Generics, Id, Item,
    ItemEnum, ItemKind, ItemSummary, Module, Target, Type as RustdocType,
    Visibility as RustdocVisibility,
};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep};

fn test_config(
    database_url: String,
    crates_io_base_url: String,
    rustdoc_json_dir: Option<PathBuf>,
) -> Config {
    Config {
        mcp_transport: TransportMode::Http,
        http_bind: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)),
        mcp_sse_keep_alive_secs: 15,
        mcp_sse_retry_ms: 3000,
        database_url,
        crates_io_base_url: crates_io_base_url.clone(),
        crates_io_user_agent: "rust-mcp-tests/0.1.0".to_string(),
        crates_io_timeout_secs: 20,
        crates_io_min_interval_ms: 1,
        docs_rs_base_url: crates_io_base_url,
        docs_rs_min_interval_ms: 1,
        osv_min_interval_ms: 1,
        database_min_connections: 1,
        database_max_connections: 4,
        max_concurrent_requests: 32,
        prometheus_bind: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)),
        auto_migrate: false,
        cargo_registry_dir: PathBuf::from("/tmp"),
        data_dir: PathBuf::from("/tmp"),
        rustsec_db_dir: None,
        rustdoc_json_dir,
        rust_log: "warn".to_string(),
        log_format: LogFormat::Pretty,
    }
}

struct MockCratesIoServer {
    base_url: String,
    shutdown_tx: Option<oneshot::Sender<()>>,
    serve_task: JoinHandle<()>,
}

impl Drop for MockCratesIoServer {
    fn drop(&mut self) {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }
        self.serve_task.abort();
    }
}

impl MockCratesIoServer {
    async fn start() -> Result<Self> {
        let router = Router::new()
            .route("/api/v1/crates", get(mock_search_crates))
            .route("/api/v1/crates/{crate_name}", get(mock_crate_detail))
            .route("/api/v1/crates/{crate_name}/{version}/readme", get(mock_crate_readme))
            .route(
                "/api/v1/crates/{crate_name}/{version}/dependencies",
                get(mock_crate_dependencies),
            )
            .route("/crate/{crate_name}/{version}/json", get(mock_rustdoc_json))
            .route("/crate/{crate_name}/{version}/json.gz", get(mock_rustdoc_json_gzip));

        let listener =
            TcpListener::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))).await?;
        let addr = listener.local_addr()?;
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let serve_task = tokio::spawn(async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await;
        });

        Ok(Self {
            base_url: format!("http://{addr}"),
            shutdown_tx: Some(shutdown_tx),
            serve_task,
        })
    }

    fn base_url(&self) -> &str {
        &self.base_url
    }
}

struct MockIndexSyncContext {
    mcp: LocalMcpHttpHarness,
    state: AppState,
    _postgres: PostgresTestContainer,
    _crates_io: MockCratesIoServer,
}

async fn mock_index_sync_context() -> Result<MockIndexSyncContext> {
    mock_index_sync_context_with_rustdoc_dir(None).await
}

async fn mock_index_sync_context_with_rustdoc_dir(
    rustdoc_json_dir: Option<PathBuf>,
) -> Result<MockIndexSyncContext> {
    let postgres = PostgresTestContainer::start().await?;
    let crates_io = MockCratesIoServer::start().await?;

    let config = test_config(
        postgres
            .connection_string()
            .to_string(),
        crates_io
            .base_url()
            .to_string(),
        rustdoc_json_dir,
    );
    let state = AppState::connect(config.clone()).await?;
    state.run_migrations().await?;

    let router = http::router(state.clone(), config);
    let mcp = LocalMcpHttpHarness::spawn(router).await?;
    mcp.wait_until_ready(Duration::from_secs(20))
        .await?;
    let _ = mcp
        .initialize("index-tools-integration")
        .await?;

    Ok(MockIndexSyncContext {
        mcp,
        state,
        _postgres: postgres,
        _crates_io: crates_io,
    })
}

async fn wait_for_refresh_job_terminal(
    db: &sqlx::PgPool,
    job_id: i64,
    timeout: Duration,
) -> (String, Option<String>) {
    let started = Instant::now();
    loop {
        let row = sqlx::query_as::<_, (String, Option<String>)>(
            "SELECT status, last_error
             FROM refresh_jobs
             WHERE id = $1",
        )
        .bind(job_id)
        .fetch_one(db)
        .await
        .expect("failed to fetch refresh job state");

        if row.0 == "finished" || row.0 == "failed" {
            return row;
        }

        assert!(
            started.elapsed() <= timeout,
            "timed out waiting for refresh job {job_id} to reach terminal state; last status={}",
            row.0
        );
        sleep(Duration::from_millis(100)).await;
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
            docs: Some("Parse input".to_string()),
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

fn write_rustdoc_fixture_file() -> (PathBuf, String, String) {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("rust-mcp-rustdoc-json-{nanos}"));
    std::fs::create_dir_all(&dir).expect("failed to create rustdoc fixture directory");

    let crate_name = "demo-rustdoc".to_string();
    let crate_version = "1.2.3".to_string();
    let rustdoc_crate = build_rustdoc_fixture("demo_rustdoc", &crate_version);
    let file_path = dir.join(format!("{crate_name}-{crate_version}.json"));
    let encoded = serde_json::to_vec(&rustdoc_crate).expect("failed to encode rustdoc fixture");
    std::fs::write(&file_path, encoded).expect("failed to write rustdoc fixture file");

    (dir, crate_name, crate_version)
}

fn write_rustdoc_fixture_files(versions: &[&str]) -> (PathBuf, String, Vec<String>) {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("rust-mcp-rustdoc-json-multi-{nanos}"));
    std::fs::create_dir_all(&dir).expect("failed to create rustdoc fixture directory");

    let crate_name = "demo-rustdoc".to_string();
    let mut crate_versions = Vec::with_capacity(versions.len());
    for version in versions {
        let crate_version = (*version).to_string();
        let rustdoc_crate = build_rustdoc_fixture("demo_rustdoc", &crate_version);
        let file_path = dir.join(format!("{crate_name}-{crate_version}.json"));
        let encoded = serde_json::to_vec(&rustdoc_crate).expect("failed to encode rustdoc fixture");
        std::fs::write(file_path, encoded).expect("failed to write rustdoc fixture file");
        crate_versions.push(crate_version);
    }

    (dir, crate_name, crate_versions)
}

fn write_invalid_utf8_rustdoc_fixture_file() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("rust-mcp-rustdoc-json-invalid-{nanos}"));
    std::fs::create_dir_all(&dir).expect("failed to create invalid rustdoc fixture directory");
    let file_path = dir.join("demo-rustdoc-1.2.3.json");
    std::fs::write(file_path, vec![0xff, 0xfe, 0xfd]).expect("failed to write invalid utf8 file");
    dir
}

async fn mock_search_crates() -> Json<Value> {
    Json(json!({
        "crates": [
            {
                "id": "demo_crate",
                "name": "demo_crate"
            }
        ],
        "meta": {
            "total": 1
        }
    }))
}

async fn mock_crate_detail(Path(crate_name): Path<String>) -> impl IntoResponse {
    if crate_name != "demo_crate" {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "errors": [ { "detail": "crate not found" } ] })),
        )
            .into_response();
    }

    (
        StatusCode::OK,
        Json(json!({
            "crate": {
                "name": "demo_crate",
                "description": "Integration test crate",
                "repository": "https://example.test/demo",
                "documentation": "https://docs.rs/demo_crate",
                "homepage": "https://example.test",
                "max_version": "1.2.3"
            },
            "versions": [
                {
                    "num": "1.2.3",
                    "created_at": "2026-01-01T00:00:00Z",
                    "updated_at": "2026-01-01T00:00:00Z",
                    "yanked": false,
                    "downloads": 1234,
                    "checksum": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "rust_version": "1.75",
                    "license": "MIT OR Apache-2.0",
                    "features": {
                        "default": ["serde_support"],
                        "serde_support": ["dep:serde"]
                    }
                }
            ],
            "keywords": [
                {"id": "kw-demo", "keyword": "demo"}
            ],
            "categories": [
                {"id": "cat-devtools", "slug": "development-tools", "category": "Development tools"}
            ]
        })),
    )
        .into_response()
}

async fn mock_crate_readme(
    Path((crate_name, version)): Path<(String, String)>,
) -> impl IntoResponse {
    if crate_name != "demo_crate" || version != "1.2.3" {
        return StatusCode::NOT_FOUND.into_response();
    }

    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        "# demo_crate\nThis is a mocked readme.",
    )
        .into_response()
}

async fn mock_crate_dependencies(
    Path((crate_name, version)): Path<(String, String)>,
) -> impl IntoResponse {
    if crate_name != "demo_crate" || version != "1.2.3" {
        return (StatusCode::NOT_FOUND, Json(json!({ "dependencies": [] }))).into_response();
    }

    (
        StatusCode::OK,
        Json(json!({
            "dependencies": [
                {
                    "crate_id": "serde",
                    "req": "^1.0",
                    "kind": "normal",
                    "optional": false,
                    "features": []
                }
            ]
        })),
    )
        .into_response()
}

async fn mock_rustdoc_json(
    Path((crate_name, version)): Path<(String, String)>,
) -> impl IntoResponse {
    if crate_name == "docs-rustdoc" && version == "1.2.3" {
        let rustdoc = build_rustdoc_fixture("docs_rustdoc", &version);
        let bytes = serde_json::to_vec(&rustdoc).expect("failed to encode docs-rustdoc fixture");
        return (StatusCode::OK, [(header::CONTENT_TYPE, "application/json")], bytes)
            .into_response();
    }

    StatusCode::NOT_FOUND.into_response()
}

async fn mock_rustdoc_json_gzip(
    Path((crate_name, version)): Path<(String, String)>,
) -> impl IntoResponse {
    if crate_name == "docs-rustdoc" && version == "1.2.3" {
        let rustdoc = build_rustdoc_fixture("docs_rustdoc", &version);
        let bytes = serde_json::to_vec(&rustdoc).expect("failed to encode docs-rustdoc fixture");
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder
            .write_all(&bytes)
            .expect("failed to write docs-rustdoc fixture to gzip encoder");
        let gzip_bytes = encoder
            .finish()
            .expect("failed to finish docs-rustdoc gzip payload");
        return (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/gzip"), (header::CONTENT_ENCODING, "gzip")],
            gzip_bytes,
        )
            .into_response();
    }

    StatusCode::NOT_FOUND.into_response()
}

#[tokio::test]
async fn index_status_reports_seeded_coverage_counts() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    let response = context
        .mcp
        .call_tool("index.status", json!({}))
        .await
        .expect("index.status call failed");
    let payload = common::structured_content(&response);

    let coverage = payload
        .get("coverage")
        .and_then(Value::as_object)
        .expect("coverage should be an object");

    assert_eq!(
        coverage
            .get("crates")
            .and_then(Value::as_i64),
        Some(3)
    );
    assert_eq!(
        coverage
            .get("crate_versions")
            .and_then(Value::as_i64),
        Some(3)
    );
    assert_eq!(
        coverage
            .get("dependency_edges")
            .and_then(Value::as_i64),
        Some(2)
    );
    assert_eq!(
        coverage
            .get("source_files")
            .and_then(Value::as_i64),
        Some(1)
    );
}

#[tokio::test]
async fn index_refresh_local_cache_returns_terminal_status() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    let response = context
        .mcp
        .call_tool(
            "index.refresh",
            json!({
                "scope": "local_cache",
                "crate_name": "serde_json"
            }),
        )
        .await
        .expect("index.refresh call failed");
    let payload = common::structured_content(&response);

    assert_eq!(
        payload
            .get("scope")
            .and_then(Value::as_str),
        Some("local_cache")
    );
    assert_eq!(
        payload
            .get("accepted")
            .and_then(Value::as_bool),
        Some(true)
    );

    let status = payload
        .get("status")
        .and_then(Value::as_str)
        .expect("status should be a string");
    assert!(matches!(status, "completed" | "completed_with_errors"));
}

#[tokio::test]
async fn index_sync_crates_writes_versions_features_and_dependencies_from_mock_api() {
    let context = mock_index_sync_context()
        .await
        .expect("failed to build mock index sync context");

    let response = context
        .mcp
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
        .expect("index.sync_crates call failed");
    let payload = common::structured_content(&response);

    assert_eq!(
        payload
            .get("total_candidates")
            .and_then(Value::as_u64),
        Some(1)
    );
    assert_eq!(
        payload
            .get("synced_crates")
            .and_then(Value::as_u64),
        Some(1)
    );
    assert_eq!(
        payload
            .get("synced_versions")
            .and_then(Value::as_u64),
        Some(1)
    );
    assert_eq!(
        payload
            .get("synced_dependencies")
            .and_then(Value::as_u64),
        Some(1)
    );
    assert_eq!(
        payload
            .get("errors")
            .and_then(Value::as_array)
            .map(|errors| errors.len()),
        Some(0)
    );
    assert!(
        payload
            .get("selected_versions")
            .and_then(Value::as_array)
            .is_some_and(|versions| {
                versions
                    .iter()
                    .any(|version| version.as_str() == Some("demo_crate@1.2.3"))
            })
    );

    let demo_crate_exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM crates WHERE name = 'demo_crate')",
    )
    .fetch_one(&context.state.db)
    .await
    .expect("failed to verify demo_crate row");
    assert!(demo_crate_exists);

    let feature_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*)::BIGINT
         FROM crate_version_features cvf
         JOIN crate_versions cv ON cv.id = cvf.crate_version_id
         JOIN crates c ON c.id = cv.crate_id
         WHERE c.name = 'demo_crate'",
    )
    .fetch_one(&context.state.db)
    .await
    .expect("failed to count crate_version_features");
    assert_eq!(feature_count, 2);

    let dependency_edge_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*)::BIGINT
         FROM dependency_edges de
         JOIN crate_versions cv ON cv.id = de.from_version_id
         JOIN crates from_c ON from_c.id = cv.crate_id
         JOIN crates to_c ON to_c.id = de.to_crate_id
         WHERE from_c.name = 'demo_crate' AND to_c.name = 'serde'",
    )
    .fetch_one(&context.state.db)
    .await
    .expect("failed to count dependency edges");
    assert_eq!(dependency_edge_count, 1);
}

#[tokio::test]
async fn index_refresh_crate_scope_uses_mock_sync_pipeline() {
    let context = mock_index_sync_context()
        .await
        .expect("failed to build mock index sync context");

    let response = context
        .mcp
        .call_tool(
            "index.refresh",
            json!({
                "scope": "crate",
                "crate_name": "demo_crate",
                "include_dependencies": true
            }),
        )
        .await
        .expect("index.refresh crate-scope call failed");
    let payload = common::structured_content(&response);

    assert_eq!(
        payload
            .get("scope")
            .and_then(Value::as_str),
        Some("crate")
    );
    assert_eq!(
        payload
            .get("status")
            .and_then(Value::as_str),
        Some("completed")
    );

    let result = payload
        .get("result")
        .and_then(Value::as_object)
        .expect("result should be an object");
    assert_eq!(
        result
            .get("synced_crates")
            .and_then(Value::as_u64),
        Some(1)
    );
    assert_eq!(
        result
            .get("synced_versions")
            .and_then(Value::as_u64),
        Some(1)
    );
    assert_eq!(
        result
            .get("synced_dependencies")
            .and_then(Value::as_u64),
        Some(1)
    );
}

#[tokio::test]
async fn index_refresh_rustdoc_json_scope_ingests_docs_rs_payload() {
    let context = mock_index_sync_context()
        .await
        .expect("failed to build rustdoc index context");

    let seeded = seed_crate_release(
        &context.state.db,
        "docs-rustdoc",
        "1.2.3",
        84,
        Some("2026-01-01T00:00:00Z"),
    )
    .await
    .expect("failed to seed crate release for docs.rs rustdoc sync");
    assert!(seeded.version_id > 0);

    let response = context
        .mcp
        .call_tool(
            "index.refresh",
            json!({
                "scope": "rustdoc_json",
                "crate_name": "docs-rustdoc",
                "page": 1,
                "per_page": 20
            }),
        )
        .await
        .expect("index.refresh rustdoc_json docs.rs call failed");
    let payload = common::structured_content(&response);

    assert_eq!(
        payload
            .get("scope")
            .and_then(Value::as_str),
        Some("rustdoc_json")
    );
    assert_eq!(
        payload
            .get("status")
            .and_then(Value::as_str),
        Some("completed")
    );

    let rustdoc_symbol_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*)::BIGINT
         FROM symbols s
         JOIN crate_versions cv ON cv.id = s.crate_version_id
         JOIN crates c ON c.id = cv.crate_id
         WHERE c.name = $1
           AND cv.version = $2
           AND s.index_source = 'rustdoc_json'",
    )
    .bind("docs-rustdoc")
    .bind("1.2.3")
    .fetch_one(&context.state.db)
    .await
    .expect("failed to count docs.rs rustdoc symbols");
    assert!(rustdoc_symbol_count >= 1);
}

#[tokio::test]
async fn index_refresh_rustdoc_json_scope_ingests_fixture_file() {
    let (rustdoc_dir, crate_name, crate_version) = write_rustdoc_fixture_file();
    let context = mock_index_sync_context_with_rustdoc_dir(Some(rustdoc_dir))
        .await
        .expect("failed to build rustdoc index context");

    let seeded = seed_crate_release(
        &context.state.db,
        &crate_name,
        &crate_version,
        42,
        Some("2026-01-01T00:00:00Z"),
    )
    .await
    .expect("failed to seed crate release for rustdoc sync");
    assert!(seeded.version_id > 0);

    let response = context
        .mcp
        .call_tool(
            "index.refresh",
            json!({
                "scope": "rustdoc_json",
                "crate_name": crate_name,
                "page": 1,
                "per_page": 20
            }),
        )
        .await
        .expect("index.refresh rustdoc_json call failed");
    let payload = common::structured_content(&response);

    assert_eq!(
        payload
            .get("scope")
            .and_then(Value::as_str),
        Some("rustdoc_json")
    );
    assert!(matches!(
        payload
            .get("status")
            .and_then(Value::as_str),
        Some("completed" | "completed_with_errors")
    ));

    let result = payload
        .get("result")
        .and_then(Value::as_object)
        .expect("result should be present");
    assert!(
        result
            .get("synced_versions")
            .and_then(Value::as_u64)
            .unwrap_or_default()
            >= 1
    );
    assert!(
        result
            .get("synced_dependencies")
            .and_then(Value::as_u64)
            .unwrap_or_default()
            >= 1
    );

    let rustdoc_symbol_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*)::BIGINT
         FROM symbols s
         JOIN crate_versions cv ON cv.id = s.crate_version_id
         JOIN crates c ON c.id = cv.crate_id
         WHERE c.name = $1
           AND cv.version = $2
           AND s.index_source = 'rustdoc_json'",
    )
    .bind("demo-rustdoc")
    .bind("1.2.3")
    .fetch_one(&context.state.db)
    .await
    .expect("failed to count rustdoc symbols");
    assert!(rustdoc_symbol_count >= 1);
}

#[tokio::test]
async fn background_refresh_worker_processes_rustdoc_json_job() {
    let (rustdoc_dir, crate_name, crate_version) = write_rustdoc_fixture_file();
    let context = mock_index_sync_context_with_rustdoc_dir(Some(rustdoc_dir))
        .await
        .expect("failed to build rustdoc index context");

    let seeded = seed_crate_release(
        &context.state.db,
        &crate_name,
        &crate_version,
        42,
        Some("2026-01-01T00:00:00Z"),
    )
    .await
    .expect("failed to seed crate release for rustdoc worker test");
    assert!(seeded.version_id > 0);

    let job_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO refresh_jobs (
            crate_name,
            scope,
            priority,
            status,
            include_dependencies,
            payload,
            requested_at
         ) VALUES (
            $1,
            'rustdoc_json',
            10,
            'pending',
            true,
            $2::JSONB,
            NOW()
         )
         RETURNING id",
    )
    .bind("*")
    .bind(json!({ "crate_name": crate_name, "page": 1, "per_page": 20 }))
    .fetch_one(&context.state.db)
    .await
    .expect("failed to insert refresh_jobs row");

    let worker_handle = tokio::spawn(run_refresh_worker_for_tests(context.state.clone()));
    let (status, last_error) =
        wait_for_refresh_job_terminal(&context.state.db, job_id, Duration::from_secs(20)).await;
    worker_handle.abort();

    assert_eq!(status, "finished", "background rustdoc job failed with last_error={last_error:?}");

    let rustdoc_symbol_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*)::BIGINT
         FROM symbols s
         JOIN crate_versions cv ON cv.id = s.crate_version_id
         JOIN crates c ON c.id = cv.crate_id
         WHERE c.name = $1
           AND cv.version = $2
           AND s.index_source = 'rustdoc_json'",
    )
    .bind("demo-rustdoc")
    .bind("1.2.3")
    .fetch_one(&context.state.db)
    .await
    .expect("failed to count rustdoc symbols written by worker");
    assert!(rustdoc_symbol_count >= 1);
}

#[tokio::test]
async fn startup_rustdoc_json_refresh_processes_all_pages() {
    let (rustdoc_dir, crate_name, crate_versions) =
        write_rustdoc_fixture_files(&["1.2.3", "1.2.4"]);
    let context = mock_index_sync_context_with_rustdoc_dir(Some(rustdoc_dir))
        .await
        .expect("failed to build rustdoc index context");

    for (idx, crate_version) in crate_versions
        .iter()
        .enumerate()
    {
        let seeded = seed_crate_release(
            &context.state.db,
            &crate_name,
            crate_version,
            100 + idx as i64,
            Some("2026-01-01T00:00:00Z"),
        )
        .await
        .expect("failed to seed crate release for startup rustdoc worker test");
        assert!(seeded.version_id > 0);
    }

    run_startup_rustdoc_json_refresh_for_tests(context.state.clone(), 1).await;

    for crate_version in &crate_versions {
        let rustdoc_symbol_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*)::BIGINT
             FROM symbols s
             JOIN crate_versions cv ON cv.id = s.crate_version_id
             JOIN crates c ON c.id = cv.crate_id
             WHERE c.name = $1
               AND cv.version = $2
               AND s.index_source = 'rustdoc_json'",
        )
        .bind(&crate_name)
        .bind(crate_version)
        .fetch_one(&context.state.db)
        .await
        .expect("failed to count rustdoc symbols written by startup refresh");
        assert!(
            rustdoc_symbol_count >= 1,
            "expected rustdoc symbols for {crate_name}@{crate_version}"
        );
    }
}

#[tokio::test]
async fn index_refresh_rustdoc_json_scope_reports_remote_and_fallback_failures() {
    let context = mock_index_sync_context()
        .await
        .expect("failed to build index context");

    let seeded = seed_crate_release(
        &context.state.db,
        "demo-rustdoc",
        "1.2.3",
        42,
        Some("2026-01-01T00:00:00Z"),
    )
    .await
    .expect("failed to seed crate release for rustdoc failure-path test");
    assert!(seeded.version_id > 0);

    let response = context
        .mcp
        .call_tool(
            "index.refresh",
            json!({
                "scope": "rustdoc_json",
                "crate_name": "demo-rustdoc",
                "page": 1,
                "per_page": 20
            }),
        )
        .await
        .expect("index.refresh rustdoc_json failure-path call failed");
    let payload = common::structured_content(&response);

    assert_eq!(
        payload
            .get("status")
            .and_then(Value::as_str),
        Some("completed_with_errors")
    );
    let result = payload
        .get("result")
        .and_then(Value::as_object)
        .expect("result should be present");
    assert!(
        result
            .get("errors")
            .and_then(Value::as_array)
            .is_some_and(|errors| {
                errors
                    .iter()
                    .filter_map(Value::as_str)
                    .any(|error| {
                        error.contains("docs.rs rustdoc JSON request failed")
                            && error.contains("local rustdoc fallback unavailable")
                    })
            })
    );
}

#[tokio::test]
async fn index_refresh_rustdoc_json_scope_reports_invalid_utf8_fixture() {
    let rustdoc_dir = write_invalid_utf8_rustdoc_fixture_file();
    let context = mock_index_sync_context_with_rustdoc_dir(Some(rustdoc_dir))
        .await
        .expect("failed to build invalid-rustdoc index context");

    let seeded = seed_crate_release(
        &context.state.db,
        "demo-rustdoc",
        "1.2.3",
        42,
        Some("2026-01-01T00:00:00Z"),
    )
    .await
    .expect("failed to seed crate release for invalid-utf8 rustdoc sync");
    assert!(seeded.version_id > 0);

    let response = context
        .mcp
        .call_tool(
            "index.refresh",
            json!({
                "scope": "rustdoc_json",
                "crate_name": "demo-rustdoc",
                "page": 1,
                "per_page": 20
            }),
        )
        .await
        .expect("index.refresh rustdoc_json invalid-utf8 call failed");
    let payload = common::structured_content(&response);

    assert_eq!(
        payload
            .get("status")
            .and_then(Value::as_str),
        Some("completed_with_errors")
    );
    let result = payload
        .get("result")
        .and_then(Value::as_object)
        .expect("result should be present");
    assert!(
        result
            .get("errors")
            .and_then(Value::as_array)
            .is_some_and(|errors| {
                errors
                    .iter()
                    .filter_map(Value::as_str)
                    .any(|error| error.contains("invalid UTF-8 in rustdoc JSON file"))
            })
    );
}
