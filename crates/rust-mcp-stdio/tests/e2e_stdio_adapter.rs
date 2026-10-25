//! End-to-end tests for the rust-mcp-stdio adapter process.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolRequestParams, ProtocolVersion, ServerCapabilities, ServerConfig};
use rmcp::schemars::JsonSchema;
use rmcp::service::RunningService;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{
    ConfigureCommandExt as _, StreamableHttpServerConfig, StreamableHttpService, TokioChildProcess,
};
use rmcp::{
    ClientLifecycleMode, ClientServiceExt as _, RoleClient, ServerHandler, tool, tool_handler,
    tool_router,
};
use rust_mcp_types::protocol::{LATEST_MCP_PROTOCOL_VERSION, MCP_SESSION_ID_HEADER};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::TcpListener;
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::oneshot;

#[derive(Debug)]
struct MockMcpState {
    initialized_notification_seen: AtomicBool,
    initialize_calls: AtomicUsize,
    stateless_calls: AtomicUsize,
    session_calls: AtomicUsize,
    /// The currently valid legacy session; each `initialize` issues a new one.
    active_session: Mutex<Option<String>>,
    /// Expire the active session after this many session-based tool calls.
    expire_session_after_calls: Option<usize>,
    /// 1-based `initialize` attempts that fail with HTTP 503.
    failing_initialize_calls: Vec<usize>,
}

impl MockMcpState {
    fn new() -> Self {
        Self {
            initialized_notification_seen: AtomicBool::new(false),
            initialize_calls: AtomicUsize::new(0),
            stateless_calls: AtomicUsize::new(0),
            session_calls: AtomicUsize::new(0),
            active_session: Mutex::new(None),
            expire_session_after_calls: None,
            failing_initialize_calls: Vec::new(),
        }
    }

    fn active_session(&self) -> Option<String> {
        self.active_session
            .lock()
            .expect("session lock")
            .clone()
    }

    fn set_active_session(&self, session_id: Option<String>) {
        *self
            .active_session
            .lock()
            .expect("session lock") = session_id;
    }
}

/// An axum router served on an ephemeral localhost port.
#[derive(Debug)]
struct TestHttpServer {
    base_url: String,
    shutdown_tx: Option<oneshot::Sender<()>>,
    serve_task: tokio::task::JoinHandle<()>,
}

impl TestHttpServer {
    async fn spawn(router: Router) -> Self {
        let listener = TcpListener::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)))
            .await
            .expect("bind listener");
        let addr = listener
            .local_addr()
            .expect("listener local addr");

        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let serve_task = tokio::spawn(async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await;
        });

        Self {
            base_url: format!("http://{addr}"),
            shutdown_tx: Some(shutdown_tx),
            serve_task,
        }
    }

    fn mcp_url(&self) -> String {
        format!("{}/mcp", self.base_url)
    }
}

impl Drop for TestHttpServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        self.serve_task.abort();
    }
}

/// Hand-rolled upstream recording which protocol path each call took.
#[derive(Debug)]
struct MockServer {
    http: TestHttpServer,
    state: Arc<MockMcpState>,
}

impl MockServer {
    async fn spawn() -> Self {
        Self::spawn_with(MockMcpState::new()).await
    }

    async fn spawn_with(state: MockMcpState) -> Self {
        let state = Arc::new(state);
        let router = Router::new()
            .route("/schemas", get(schemas_handler))
            .route("/mcp", post(mcp_handler))
            .with_state(state.clone());

        Self {
            http: TestHttpServer::spawn(router).await,
            state,
        }
    }
}

async fn schemas_handler() -> impl IntoResponse {
    Json(json!({
        "tool_name": null,
        "total_tools": 2,
        "schemas": [
            {
                "tool_name": "ping",
                "request": {"type": "object"},
                "response": {"type": "object"}
            },
            {
                "tool_name": "source.search",
                "request": {"type": "object"},
                "response": {"type": "object"}
            }
        ]
    }))
}

/// Whether a request is shaped for the stateless protocol: version and
/// standard headers matching the body, plus per-request `_meta`.
fn is_stateless_request(headers: &HeaderMap, payload: &Value) -> bool {
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
    };
    let method = payload
        .get("method")
        .and_then(Value::as_str);
    let tool_name = payload
        .pointer("/params/name")
        .and_then(Value::as_str);
    let meta_version = payload
        .pointer("/params/_meta/io.modelcontextprotocol~1protocolVersion")
        .and_then(Value::as_str);

    header("mcp-protocol-version") == Some(LATEST_MCP_PROTOCOL_VERSION)
        && header("mcp-method") == method
        && header("mcp-name") == tool_name
        && meta_version == Some(LATEST_MCP_PROTOCOL_VERSION)
}

async fn mcp_handler(
    State(state): State<Arc<MockMcpState>>,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> impl IntoResponse {
    let method = payload
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();

    let request_id = payload
        .get("id")
        .cloned()
        .unwrap_or(Value::Null);

    match method {
        "initialize" => {
            let attempt = state
                .initialize_calls
                .fetch_add(1, Ordering::Relaxed)
                + 1;
            if state
                .failing_initialize_calls
                .contains(&attempt)
            {
                return (StatusCode::SERVICE_UNAVAILABLE, "Service Unavailable").into_response();
            }
            let session_id = format!("session-{attempt}");
            state.set_active_session(Some(session_id.clone()));
            state
                .initialized_notification_seen
                .store(false, Ordering::Relaxed);
            let response = json!({
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {
                    "protocolVersion": "2025-03-26",
                    "capabilities": {},
                    "serverInfo": {
                        "name": "mock-rust-mcp",
                        "version": "0.1.0"
                    }
                }
            });

            let mut resp_headers = HeaderMap::new();
            resp_headers.insert(
                MCP_SESSION_ID_HEADER,
                HeaderValue::from_str(&session_id).expect("header value"),
            );

            (StatusCode::OK, resp_headers, Json(response)).into_response()
        }
        "notifications/initialized" => {
            let session = headers
                .get(MCP_SESSION_ID_HEADER)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default();
            if state
                .active_session()
                .as_deref()
                == Some(session)
            {
                state
                    .initialized_notification_seen
                    .store(true, Ordering::Relaxed);
            }
            (StatusCode::ACCEPTED, "").into_response()
        }
        "tools/call" => {
            let session = headers
                .get(MCP_SESSION_ID_HEADER)
                .and_then(|v| v.to_str().ok());
            if session.is_some() && session.map(str::to_string) != state.active_session() {
                return (StatusCode::NOT_FOUND, "Not Found: Session not found").into_response();
            }
            let has_valid_session = session.is_some();
            let initialized = state
                .initialized_notification_seen
                .load(Ordering::Relaxed);
            let stateless = is_stateless_request(&headers, &payload);
            if stateless {
                state
                    .stateless_calls
                    .fetch_add(1, Ordering::Relaxed);
            }

            if !stateless && (!has_valid_session || !initialized) {
                let error_payload = json!({
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "error": {
                        "code": -32001,
                        "message": "missing or invalid MCP session"
                    }
                });
                return (StatusCode::UNAUTHORIZED, Json(error_payload)).into_response();
            }

            let tool_name = payload
                .get("params")
                .and_then(|v| v.get("name"))
                .and_then(Value::as_str)
                .unwrap_or_default();

            match tool_name {
                "ping" => {
                    let success = json!({
                        "jsonrpc": "2.0",
                        "id": request_id,
                        "result": {
                            "content": [{"type": "text", "text": "pong"}],
                            "structuredContent": {"message": "pong"}
                        }
                    });
                    if !stateless {
                        let calls = state
                            .session_calls
                            .fetch_add(1, Ordering::Relaxed)
                            + 1;
                        if state.expire_session_after_calls == Some(calls) {
                            state.set_active_session(None);
                        }
                    }
                    (StatusCode::OK, Json(success)).into_response()
                }
                "source.search" => {
                    let cursor = payload
                        .get("params")
                        .and_then(|v| v.get("arguments"))
                        .and_then(|v| v.get("cursor"))
                        .and_then(Value::as_str);

                    let response = if cursor == Some("cursor-1") {
                        json!({
                            "jsonrpc": "2.0",
                            "id": request_id,
                            "result": {
                                "structuredContent": {
                                    "cursor": "cursor-1",
                                    "next_cursor": null,
                                    "has_more": false,
                                    "count": 1,
                                    "hits": [
                                        {"path": "src/lib.rs", "snippet": "page2"}
                                    ]
                                }
                            }
                        })
                    } else {
                        json!({
                            "jsonrpc": "2.0",
                            "id": request_id,
                            "result": {
                                "structuredContent": {
                                    "cursor": null,
                                    "next_cursor": "cursor-1",
                                    "has_more": true,
                                    "count": 1,
                                    "hits": [
                                        {"path": "src/lib.rs", "snippet": "page1"}
                                    ]
                                }
                            }
                        })
                    };

                    (StatusCode::OK, Json(response)).into_response()
                }
                _ => {
                    let error_payload = json!({
                        "jsonrpc": "2.0",
                        "id": request_id,
                        "error": {
                            "code": -32601,
                            "message": format!("unknown tool: {tool_name}")
                        }
                    });
                    (StatusCode::NOT_FOUND, Json(error_payload)).into_response()
                }
            }
        }
        _ => {
            let error_payload = json!({
                "jsonrpc": "2.0",
                "id": request_id,
                "error": {
                    "code": -32601,
                    "message": format!("unknown method: {method}")
                }
            });
            (StatusCode::NOT_FOUND, Json(error_payload)).into_response()
        }
    }
}

async fn spawn_stdio_adapter(mcp_url: &str) -> Child {
    Command::new(env!("CARGO_BIN_EXE_rust-mcp-stdio"))
        .arg("--mcp-url")
        .arg(mcp_url)
        .arg("--preflight-schema")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn stdio adapter")
}

async fn write_json_line(stdin: &mut ChildStdin, payload: &Value) {
    let mut line = serde_json::to_vec(payload).expect("encode json");
    line.push(b'\n');
    stdin
        .write_all(&line)
        .await
        .expect("write message");
    stdin
        .flush()
        .await
        .expect("flush stdin");
}

async fn read_json_line(reader: &mut BufReader<tokio::process::ChildStdout>) -> Value {
    let mut line = String::new();
    let bytes = reader
        .read_line(&mut line)
        .await
        .expect("read line");
    assert!(bytes > 0, "unexpected EOF while reading adapter output");
    serde_json::from_str::<Value>(line.trim_end()).expect("decode json line")
}

/// Sends `payloads` through one adapter process, returning the responses to
/// those carrying an `id`.
async fn exchange(mcp_url: &str, payloads: &[Value]) -> Vec<Value> {
    let mut child = spawn_stdio_adapter(mcp_url).await;

    let mut stdin = child
        .stdin
        .take()
        .expect("adapter stdin");
    let stdout = child
        .stdout
        .take()
        .expect("adapter stdout");
    let mut reader = BufReader::new(stdout);

    let mut responses = Vec::new();
    for payload in payloads {
        write_json_line(&mut stdin, payload).await;
        if payload.get("id").is_some() {
            responses.push(read_json_line(&mut reader).await);
        }
    }

    drop(stdin);
    let _ = child.wait().await;
    responses
}

async fn request_once(mcp_url: &str, payload: Value) -> Value {
    exchange(mcp_url, &[payload])
        .await
        .pop()
        .expect("adapter response")
}

#[tokio::test]
async fn one_shot_tools_call_is_forwarded_statelessly() {
    let server = MockServer::spawn().await;
    let mcp_url = server.http.mcp_url();

    let response = request_once(
        &mcp_url,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "ping",
                "arguments": {"message": "hello"}
            }
        }),
    )
    .await;

    assert!(
        response
            .get("error")
            .is_none(),
        "expected success response: {response}"
    );
    assert_eq!(
        response
            .get("result")
            .and_then(|v| v.get("structuredContent"))
            .and_then(|v| v.get("message"))
            .and_then(Value::as_str),
        Some("pong")
    );

    assert_eq!(
        server
            .state
            .stateless_calls
            .load(Ordering::Relaxed),
        1,
        "expected adapter to forward the call on the stateless protocol"
    );
    assert_eq!(
        server
            .state
            .initialize_calls
            .load(Ordering::Relaxed),
        0,
        "stateless forwarding must not initialize a session"
    );
}

#[tokio::test]
async fn legacy_client_session_is_passed_through() {
    let server = MockServer::spawn().await;
    let mcp_url = server.http.mcp_url();

    let responses = exchange(
        &mcp_url,
        &[
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-11-25",
                    "capabilities": {},
                    "clientInfo": {"name": "legacy-client", "version": "1.0.0"}
                }
            }),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}}),
            json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {"name": "ping", "arguments": {}}
            }),
        ],
    )
    .await;

    let call = responses
        .last()
        .expect("tools/call response");
    assert!(call.get("error").is_none(), "expected session-based success: {call}");
    assert_eq!(
        server
            .state
            .initialize_calls
            .load(Ordering::Relaxed),
        1
    );
    assert_eq!(
        server
            .state
            .stateless_calls
            .load(Ordering::Relaxed),
        0,
        "legacy clients must stay on their session"
    );
}

#[tokio::test]
async fn cursor_pagination_works_across_short_lived_stdio_invocations() {
    let server = MockServer::spawn().await;
    let mcp_url = server.http.mcp_url();

    let page_one = request_once(
        &mcp_url,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "source.search",
                "arguments": {
                    "query": "tokio",
                    "limit": 1
                }
            }
        }),
    )
    .await;

    let next_cursor = page_one
        .get("result")
        .and_then(|v| v.get("structuredContent"))
        .and_then(|v| v.get("next_cursor"))
        .and_then(Value::as_str)
        .expect("page one next_cursor");
    assert_eq!(next_cursor, "cursor-1");
    assert_eq!(
        page_one
            .get("result")
            .and_then(|v| v.get("structuredContent"))
            .and_then(|v| v.get("has_more"))
            .and_then(Value::as_bool),
        Some(true)
    );

    let page_two = request_once(
        &mcp_url,
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "source.search",
                "arguments": {
                    "query": "tokio",
                    "cursor": next_cursor,
                    "limit": 1
                }
            }
        }),
    )
    .await;

    assert_eq!(
        page_two
            .get("result")
            .and_then(|v| v.get("structuredContent"))
            .and_then(|v| v.get("cursor"))
            .and_then(Value::as_str),
        Some("cursor-1")
    );
    assert_eq!(
        page_two
            .get("result")
            .and_then(|v| v.get("structuredContent"))
            .and_then(|v| v.get("has_more"))
            .and_then(Value::as_bool),
        Some(false)
    );
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct PingArgs {
    message: String,
}

/// Minimal real rmcp server, so SDK-to-SDK traffic crosses the adapter.
#[derive(Debug, Clone)]
struct PingServer;

#[tool_router]
impl PingServer {
    #[tool(description = "Echo a message")]
    fn ping(&self, Parameters(args): Parameters<PingArgs>) -> String {
        format!("pong {}", args.message)
    }
}

#[tool_handler]
impl ServerHandler for PingServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .build(),
        )
    }
}

async fn spawn_rmcp_server(sessions: Arc<LocalSessionManager>) -> TestHttpServer {
    let service = StreamableHttpService::new(
        || Ok(PingServer),
        sessions,
        StreamableHttpServerConfig::default(),
    );
    let router = Router::new()
        .route("/schemas", get(schemas_handler))
        .nest_service("/mcp", service);
    TestHttpServer::spawn(router).await
}

/// Connects an rmcp stdio client to `server` through the adapter binary.
async fn connect_sdk_client(
    server: &TestHttpServer,
    lifecycle: ClientLifecycleMode,
) -> RunningService<RoleClient, ()> {
    let transport = TokioChildProcess::new(
        Command::new(env!("CARGO_BIN_EXE_rust-mcp-stdio")).configure(|cmd| {
            cmd.arg("--mcp-url")
                .arg(server.mcp_url());
        }),
    )
    .expect("spawn stdio adapter");

    ().serve_with_lifecycle(transport, lifecycle)
        .await
        .expect("SDK client should connect through the adapter")
}

async fn call_ping(client: &RunningService<RoleClient, ()>, message: &str) -> Option<String> {
    let result = client
        .call_tool(
            CallToolRequestParams::new("ping").with_arguments(
                json!({ "message": message })
                    .as_object()
                    .cloned()
                    .expect("object arguments"),
            ),
        )
        .await
        .expect("tools/call through the adapter");
    result
        .content
        .first()
        .and_then(|content| content.as_text())
        .map(|text| text.text.clone())
}

async fn live_session_ids(sessions: &LocalSessionManager) -> Vec<String> {
    sessions
        .sessions
        .read()
        .await
        .keys()
        .map(ToString::to_string)
        .collect()
}

/// Drives an rmcp stdio client through the adapter binary against a real
/// rmcp Streamable HTTP server.
async fn assert_sdk_round_trip(lifecycle: ClientLifecycleMode) {
    let server = spawn_rmcp_server(Arc::new(LocalSessionManager::default())).await;
    let client = connect_sdk_client(&server, lifecycle).await;

    let tools = client
        .list_tools(None)
        .await
        .expect("tools/list through the adapter");
    assert!(
        tools
            .tools
            .iter()
            .any(|tool| tool.name == "ping"),
        "expected the upstream ping tool: {tools:?}"
    );
    assert_eq!(
        call_ping(&client, "via-adapter")
            .await
            .as_deref(),
        Some("pong via-adapter")
    );

    client
        .cancel()
        .await
        .expect("shut down SDK client");
}

#[tokio::test]
async fn sdk_client_with_discover_lifecycle_works_through_adapter() {
    assert_sdk_round_trip(ClientLifecycleMode::Discover {
        preferred_versions: vec![ProtocolVersion::LATEST],
    })
    .await;
}

#[tokio::test]
async fn sdk_client_with_initialize_lifecycle_works_through_adapter() {
    assert_sdk_round_trip(ClientLifecycleMode::Initialize).await;
}

/// The upstream forgets the legacy session (as after a restart without a
/// session store); the adapter must replay `initialize` and retry so the
/// stdio client never notices.
#[tokio::test]
async fn sdk_legacy_client_recovers_after_upstream_session_expires() {
    let sessions = Arc::new(LocalSessionManager::default());
    let server = spawn_rmcp_server(sessions.clone()).await;
    let client = connect_sdk_client(&server, ClientLifecycleMode::Initialize).await;

    assert_eq!(
        call_ping(&client, "before")
            .await
            .as_deref(),
        Some("pong before")
    );
    let original_sessions = live_session_ids(&sessions).await;
    assert_eq!(original_sessions.len(), 1, "expected one upstream session");

    sessions
        .sessions
        .write()
        .await
        .clear();

    assert_eq!(
        call_ping(&client, "after")
            .await
            .as_deref(),
        Some("pong after")
    );
    let new_sessions = live_session_ids(&sessions).await;
    assert_eq!(new_sessions.len(), 1, "expected a re-established upstream session");
    assert_ne!(new_sessions, original_sessions, "adapter should have opened a new session");

    client
        .cancel()
        .await
        .expect("shut down SDK client");
}

/// A repeated `initialize` must open a fresh upstream session rather than
/// carry the previous session ID.
#[tokio::test]
async fn repeated_initialize_starts_a_fresh_upstream_session() {
    let sessions = Arc::new(LocalSessionManager::default());
    let server = spawn_rmcp_server(sessions.clone()).await;
    let initialize = |id: u64| {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "reinit-client", "version": "1.0.0"}
            }
        })
    };

    let responses = exchange(
        &server.mcp_url(),
        &[
            initialize(1),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}}),
            initialize(2),
        ],
    )
    .await;

    let second = responses
        .last()
        .expect("second initialize response");
    assert!(second.get("result").is_some(), "re-initialize should succeed: {second}");
    assert_eq!(
        live_session_ids(&sessions)
            .await
            .len(),
        2,
        "expected two upstream sessions"
    );
}

/// Session expires, the replayed `initialize` fails transiently: that request
/// must fail explicitly, and the next one must retry the legacy handshake
/// rather than silently switch to the stateless protocol.
#[tokio::test]
async fn failed_session_recovery_errors_and_retries_without_switching_protocols() {
    let mut state = MockMcpState::new();
    state.expire_session_after_calls = Some(1);
    state.failing_initialize_calls = vec![2];
    let server = MockServer::spawn_with(state).await;

    let ping = |id: u64| {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {"name": "ping", "arguments": {}}
        })
    };
    let responses = exchange(
        &server.http.mcp_url(),
        &[
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-11-25",
                    "capabilities": {},
                    "clientInfo": {"name": "legacy-client", "version": "1.0.0"}
                }
            }),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}}),
            ping(2),
            ping(3),
            ping(4),
        ],
    )
    .await;

    assert!(
        responses[1]
            .get("result")
            .is_some(),
        "first call succeeds: {}",
        responses[1]
    );
    let failed = responses[2]
        .pointer("/error/message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("failed recovery should error: {}", responses[2]));
    assert!(failed.contains("could not be re-established"), "{failed}");
    assert!(
        responses[3]
            .get("result")
            .is_some(),
        "retried recovery succeeds: {}",
        responses[3]
    );

    assert_eq!(
        server
            .state
            .stateless_calls
            .load(Ordering::Relaxed),
        0,
        "a legacy client must never be switched to the stateless protocol"
    );
    assert_eq!(
        server
            .state
            .initialize_calls
            .load(Ordering::Relaxed),
        3,
        "original handshake, failed replay, successful replay"
    );
}
