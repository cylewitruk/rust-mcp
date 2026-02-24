//! End-to-end tests for the rust-mcp-stdio adapter process.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::TcpListener;
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::oneshot;

const MCP_SESSION_ID_HEADER: &str = "mcp-session-id";

#[derive(Debug)]
struct MockMcpState {
    initialized_notification_seen: AtomicBool,
    initialize_calls: AtomicUsize,
    session_id: String,
}

impl MockMcpState {
    fn new() -> Self {
        Self {
            initialized_notification_seen: AtomicBool::new(false),
            initialize_calls: AtomicUsize::new(0),
            session_id: "session-1".to_string(),
        }
    }
}

#[derive(Debug)]
struct MockServer {
    base_url: String,
    state: Arc<MockMcpState>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    serve_task: tokio::task::JoinHandle<()>,
}

impl MockServer {
    async fn spawn() -> Self {
        let state = Arc::new(MockMcpState::new());

        let router = Router::new()
            .route("/schemas", get(schemas_handler))
            .route("/mcp", post(mcp_handler))
            .with_state(state.clone());

        let listener = TcpListener::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)))
            .await
            .expect("bind listener");
        let addr = listener
            .local_addr()
            .expect("listener local addr");
        let base_url = format!("http://{addr}");

        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let serve_task = tokio::spawn(async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await;
        });

        Self {
            base_url,
            state,
            shutdown_tx: Some(shutdown_tx),
            serve_task,
        }
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        self.serve_task.abort();
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
            state
                .initialize_calls
                .fetch_add(1, Ordering::Relaxed);
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
                HeaderValue::from_str(&state.session_id).expect("header value"),
            );

            (StatusCode::OK, resp_headers, Json(response)).into_response()
        }
        "notifications/initialized" => {
            let session = headers
                .get(MCP_SESSION_ID_HEADER)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default();
            if session == state.session_id {
                state
                    .initialized_notification_seen
                    .store(true, Ordering::Relaxed);
            }
            (StatusCode::ACCEPTED, "").into_response()
        }
        "tools/call" => {
            let has_valid_session = headers
                .get(MCP_SESSION_ID_HEADER)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v == state.session_id);
            let initialized = state
                .initialized_notification_seen
                .load(Ordering::Relaxed);

            if !has_valid_session || !initialized {
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
        .arg("--auto-bootstrap-session")
        .arg("--preflight-schema")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn stdio adapter")
}

async fn write_framed_json(stdin: &mut ChildStdin, payload: &Value) {
    let body = serde_json::to_vec(payload).expect("encode json");
    let header =
        format!("Content-Length: {}\r\nContent-Type: application/json\r\n\r\n", body.len());

    stdin
        .write_all(header.as_bytes())
        .await
        .expect("write headers");
    stdin
        .write_all(&body)
        .await
        .expect("write body");
    stdin
        .flush()
        .await
        .expect("flush stdin");
}

async fn read_framed_json(reader: &mut BufReader<tokio::process::ChildStdout>) -> Value {
    let mut content_length: Option<usize> = None;

    loop {
        let mut line = String::new();
        let bytes = reader
            .read_line(&mut line)
            .await
            .expect("read line");
        assert!(bytes > 0, "unexpected EOF while reading framed headers");
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }

        if let Some((name, value)) = trimmed.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            content_length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .expect("parse content-length"),
            );
        }
    }

    let mut body = vec![0_u8; content_length.expect("content-length present")];
    reader
        .read_exact(&mut body)
        .await
        .expect("read payload body");
    serde_json::from_slice::<Value>(&body).expect("decode framed json")
}

async fn request_once(mcp_url: &str, payload: Value) -> Value {
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

    write_framed_json(&mut stdin, &payload).await;
    let response = read_framed_json(&mut reader).await;

    drop(stdin);
    let _ = child.wait().await;
    response
}

#[tokio::test]
async fn auto_bootstrap_supports_single_tools_call_without_initialize() {
    let server = MockServer::spawn().await;
    let mcp_url = format!("{}/mcp", server.base_url);

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

    assert!(
        server
            .state
            .initialize_calls
            .load(Ordering::Relaxed)
            >= 1,
        "expected adapter to bootstrap initialize call"
    );
    assert!(
        server
            .state
            .initialized_notification_seen
            .load(Ordering::Relaxed),
        "expected adapter to send notifications/initialized"
    );
}

#[tokio::test]
async fn cursor_pagination_works_across_short_lived_stdio_invocations() {
    let server = MockServer::spawn().await;
    let mcp_url = format!("{}/mcp", server.base_url);

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
