//! Stdio-to-HTTP MCP adapter for running rust-mcp behind stdio-only clients.

use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use clap::Parser;
use reqwest::StatusCode;
use rust_mcp_types::protocol::{
    LATEST_MCP_PROTOCOL_VERSION, MCP_SESSION_ID_HEADER, insert_stateless_request_meta,
    request_meta_protocol_version, stateless_request_headers,
};
use rust_mcp_types::types::schema::ToolSchemasResponse;
use serde_json::{Value, json};
use tokio::io::{
    self, AsyncBufRead, AsyncBufReadExt as _, AsyncWrite, AsyncWriteExt as _, BufReader, BufWriter,
};
use tracing::{debug, error, info, warn};

/// Client name used for requests the adapter upgrades to the stateless
/// protocol.
const ADAPTER_CLIENT_NAME: &str = "rust-mcp-stdio";

/// Runs an stdio MCP proxy that forwards JSON-RPC messages to a remote
/// Streamable HTTP MCP endpoint.
#[derive(Debug, Parser)]
#[command(author, version, about)]
struct Args {
    /// Full Streamable HTTP MCP endpoint URL.
    #[arg(long, env = "MCP_URL", default_value = "http://127.0.0.1:43173/mcp")]
    mcp_url: String,

    /// HTTP connect timeout in seconds.
    #[arg(long, env = "MCP_CONNECT_TIMEOUT_SECS", default_value_t = 10)]
    connect_timeout_secs: u64,

    /// HTTP request timeout in seconds.
    #[arg(long, env = "MCP_REQUEST_TIMEOUT_SECS", default_value_t = 120)]
    request_timeout_secs: u64,

    /// Logging level for adapter diagnostics.
    #[arg(long, env = "RUST_LOG", default_value = "warn")]
    log_level: String,

    /// Validate upstream tool contracts from `/schemas` before proxying
    /// traffic.
    #[arg(long, env = "MCP_PREFLIGHT_SCHEMA", default_value_t = true)]
    preflight_schema: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    init_tracing(&args.log_level)?;

    debug!(
        mcp_url = %args.mcp_url,
        protocol_version = LATEST_MCP_PROTOCOL_VERSION,
        "starting rust-mcp-stdio adapter"
    );

    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(args.connect_timeout_secs))
        .timeout(Duration::from_secs(args.request_timeout_secs))
        .build()
        .context("failed to build HTTP client")?;

    if args.preflight_schema {
        let tool_count = fetch_tool_contract_count(&client, &args.mcp_url).await?;
        info!(tool_count, "loaded upstream MCP tool contracts");
    }

    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = BufReader::new(stdin);
    let mut writer = BufWriter::new(stdout);
    let mut upstream = Upstream::new(client, args.mcp_url);

    while let Some(line) = read_stdio_line(&mut reader).await? {
        let payload = match serde_json::from_str::<Value>(&line) {
            Ok(payload) => payload,
            Err(error) => {
                let parse_error =
                    jsonrpc_error(Value::Null, -32700, format!("parse error: {error}"));
                write_stdio_jsonrpc_message(&mut writer, &parse_error).await?;
                continue;
            }
        };
        let is_request = payload.get("id").is_some();

        let mut upstream_messages = upstream
            .forward(&payload)
            .await
            .unwrap_or_else(|error| {
                vec![jsonrpc_error_for_payload(
                    &payload,
                    -32000,
                    format!("stdio proxy upstream call failed: {error}"),
                )]
            });

        if upstream_messages.is_empty() && is_request {
            upstream_messages.push(jsonrpc_error_for_payload(
                &payload,
                -32000,
                "stdio proxy upstream returned no JSON-RPC payload".to_string(),
            ));
        }

        for message in upstream_messages {
            if message.get("id").is_none() && !is_request {
                continue;
            }
            write_stdio_jsonrpc_message(&mut writer, &message).await?;
        }
    }

    writer
        .flush()
        .await
        .context("failed to flush stdio output")?;
    Ok(())
}

fn init_tracing(level: &str) -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(level)
        .with_writer(std::io::stderr)
        .try_init()
        .map_err(|error| anyhow!("failed to initialize tracing subscriber: {error}"))
}

/// Reads the next newline-delimited message from stdin, skipping blank lines.
/// Returns `None` at end of input.
async fn read_stdio_line<R>(reader: &mut R) -> Result<Option<String>>
where
    R: AsyncBufRead + Unpin,
{
    loop {
        let mut line = String::new();
        let bytes = reader
            .read_line(&mut line)
            .await
            .context("failed to read stdio message")?;
        if bytes == 0 {
            return Ok(None);
        }
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            return Ok(Some(trimmed.to_string()));
        }
    }
}

/// Writes one JSON-RPC message as a single line; `serde_json` escapes any
/// newlines inside strings, so the encoding never contains a raw newline.
async fn write_stdio_jsonrpc_message<W>(writer: &mut W, payload: &Value) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    let mut encoded = serde_json::to_vec(payload).context("failed to encode JSON-RPC payload")?;
    encoded.push(b'\n');

    writer
        .write_all(&encoded)
        .await
        .context("failed to write stdio JSON-RPC message")?;
    writer
        .flush()
        .await
        .context("failed to flush stdio output")?;

    Ok(())
}

/// Upstream connection state for one adapter process.
struct Upstream {
    http: reqwest::Client,
    mcp_url: String,
    /// Legacy session established by the downstream client's `initialize`.
    session_id: Option<String>,
    /// The downstream client's `initialize`, replayed if its session expires
    /// upstream (e.g. after a server restart).
    initialize: Option<Value>,
}

impl Upstream {
    fn new(http: reqwest::Client, mcp_url: String) -> Self {
        Self {
            http,
            mcp_url,
            session_id: None,
            initialize: None,
        }
    }

    /// Forwards one downstream message and returns the upstream messages to
    /// relay back.
    ///
    /// A client that sent `initialize` stays on the legacy protocol for the
    /// life of the process. When its upstream session is gone (HTTP 404, or a
    /// previous recovery failed), the saved handshake is replayed before the
    /// request is sent (again); if that fails the request errors rather than
    /// silently switching protocols.
    async fn forward(&mut self, payload: &Value) -> Result<Vec<Value>> {
        let is_initialize = payload
            .get("method")
            .and_then(Value::as_str)
            == Some("initialize");
        if is_initialize {
            self.session_id = None;
            self.initialize = Some(payload.clone());
        }

        let legacy = self.initialize.is_some();
        if legacy && !is_initialize && self.session_id.is_none() {
            self.reestablish_session()
                .await?;
        }

        let had_session = self.session_id.is_some();
        let (upstream_payload, stateless_version) = prepare_upstream(payload, legacy);
        let reply = self
            .send(&upstream_payload, stateless_version.as_deref())
            .await?;

        if had_session && reply.status == StatusCode::NOT_FOUND {
            self.reestablish_session()
                .await?;
            return self
                .send(payload, None)
                .await?
                .into_messages();
        }
        reply.into_messages()
    }

    /// Replays the downstream client's `initialize` handshake to obtain a new
    /// upstream session.
    async fn reestablish_session(&mut self) -> Result<()> {
        self.session_id = None;
        let Some(initialize) = self.initialize.clone() else {
            bail!("no legacy `initialize` handshake to replay");
        };

        let reply = self
            .send(&initialize, None)
            .await?;
        if !reply.status.is_success() || self.session_id.is_none() {
            bail!(
                "upstream MCP session expired and could not be re-established (HTTP {}); retry \
                 the request",
                reply.status
            );
        }

        let initialized =
            json!({"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}});
        self.send(&initialized, None)
            .await?;
        info!("re-established upstream MCP session");
        Ok(())
    }

    /// POSTs one message upstream: with standard headers when
    /// `stateless_version` is set, otherwise with the legacy session (if any).
    async fn send(
        &mut self,
        payload: &Value,
        stateless_version: Option<&str>,
    ) -> Result<UpstreamReply> {
        let mut request = self
            .http
            .post(&self.mcp_url)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream");

        if let Some(version) = stateless_version {
            for (name, value) in stateless_request_headers(payload, version) {
                request = request.header(name, value);
            }
        } else if let Some(session_id) = self.session_id.as_deref() {
            request = request.header(MCP_SESSION_ID_HEADER, session_id);
        }

        let response = request
            .json(payload)
            .send()
            .await
            .context("failed to send upstream MCP HTTP request")?;

        if let Some(session_id) = response
            .headers()
            .get(MCP_SESSION_ID_HEADER)
            .and_then(|value| value.to_str().ok())
        {
            self.session_id = Some(session_id.to_string());
        }

        let status = response.status();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_string();
        let body = response
            .text()
            .await
            .context("failed to read upstream MCP response body")?;

        Ok(UpstreamReply { status, content_type, body })
    }
}

/// Raw upstream HTTP response.
struct UpstreamReply {
    status: StatusCode,
    content_type: String,
    body: String,
}

impl UpstreamReply {
    /// Extracts the JSON-RPC messages to relay downstream.
    fn into_messages(self) -> Result<Vec<Value>> {
        let Self { status, content_type, body } = self;

        if !status.is_success() {
            if let Ok(json) = serde_json::from_str::<Value>(&body) {
                return Ok(vec![json]);
            }
            return Err(anyhow!("upstream MCP request returned HTTP {status} with body: {body}"));
        }

        if body.trim().is_empty() {
            return Ok(Vec::new());
        }

        if content_type.contains("text/event-stream") {
            let events = parse_sse_json_events(&body);
            if events.is_empty() {
                warn!("upstream returned SSE response with no JSON data events");
            }
            return Ok(events);
        }

        let parsed = serde_json::from_str::<Value>(&body)
            .with_context(|| format!("upstream returned invalid JSON: {body}"))?;
        Ok(vec![parsed])
    }
}

/// Prepares a downstream message for upstream delivery.
///
/// Messages from legacy clients (those that performed `initialize`) pass
/// through unchanged. Everything else goes upstream on the stateless protocol:
/// requests without per-request `_meta` get the adapter's, and the returned
/// protocol version selects the standard headers to send.
fn prepare_upstream(payload: &Value, legacy: bool) -> (Value, Option<String>) {
    let method = payload
        .get("method")
        .and_then(Value::as_str);
    if legacy || matches!(method, Some("initialize" | "notifications/initialized")) {
        return (payload.clone(), None);
    }

    let mut payload = payload.clone();
    let is_request = method.is_some() && payload.get("id").is_some();
    if is_request && request_meta_protocol_version(&payload).is_none() {
        insert_stateless_request_meta(&mut payload, ADAPTER_CLIENT_NAME, env!("CARGO_PKG_VERSION"));
    }
    let version = request_meta_protocol_version(&payload)
        .unwrap_or(LATEST_MCP_PROTOCOL_VERSION)
        .to_string();
    (payload, Some(version))
}

fn parse_sse_json_events(body: &str) -> Vec<Value> {
    let mut parsed = Vec::<Value>::new();

    for event in body.split("\n\n") {
        let mut data_lines = Vec::<&str>::new();
        for line in event.lines() {
            if let Some(data) = line.strip_prefix("data:") {
                data_lines.push(data.trim_start());
            }
        }

        if data_lines.is_empty() {
            continue;
        }

        let joined = data_lines.join("\n");
        if joined.trim().is_empty() {
            continue;
        }

        match serde_json::from_str::<Value>(&joined) {
            Ok(value) => parsed.push(value),
            Err(error) => {
                debug!(%joined, %error, "dropping non-JSON SSE data payload");
            }
        }
    }

    parsed
}

fn jsonrpc_error_for_payload(payload: &Value, code: i64, message: String) -> Value {
    let id = payload
        .get("id")
        .cloned()
        .unwrap_or(Value::Null);

    if payload.get("id").is_none() {
        error!(%message, "upstream call failed for notification; emitting synthetic null-id error");
    }

    jsonrpc_error(id, code, message)
}

fn jsonrpc_error(id: Value, code: i64, message: String) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message,
        }
    })
}

async fn fetch_tool_contract_count(client: &reqwest::Client, mcp_url: &str) -> Result<usize> {
    let schemas_url = schemas_url_from_mcp_url(mcp_url)?;
    let response = client
        .get(schemas_url)
        .send()
        .await
        .context("failed to request upstream `/schemas`")?;
    let status = response.status();
    let body = response
        .text()
        .await
        .context("failed to read upstream `/schemas` response body")?;

    if !status.is_success() {
        bail!("upstream `/schemas` returned HTTP {status}: {body}");
    }

    let typed_contracts: ToolSchemasResponse = serde_json::from_str(&body)
        .with_context(|| format!("failed to decode `/schemas` as ToolSchemasResponse: {body}"))?;

    Ok(typed_contracts.total_tools)
}

fn schemas_url_from_mcp_url(mcp_url: &str) -> Result<reqwest::Url> {
    let mut url =
        reqwest::Url::parse(mcp_url).with_context(|| format!("invalid MCP URL `{mcp_url}`"))?;

    let path = url
        .path()
        .trim_end_matches('/')
        .to_string();
    let schema_path = if let Some(prefix) = path.strip_suffix("/mcp") {
        if prefix.is_empty() { "/schemas".to_string() } else { format!("{prefix}/schemas") }
    } else if path.is_empty() {
        "/schemas".to_string()
    } else {
        format!("{path}/schemas")
    };

    url.set_path(&schema_path);
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

#[cfg(test)]
mod tests {
    use rust_mcp_types::protocol::LATEST_MCP_PROTOCOL_VERSION;
    use serde_json::json;

    use super::{
        jsonrpc_error_for_payload, parse_sse_json_events, prepare_upstream, read_stdio_line,
        schemas_url_from_mcp_url, write_stdio_jsonrpc_message,
    };

    #[test]
    fn parses_multiple_json_sse_data_events() {
        let body = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n\ndata: \
                    {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"done\":true}}\n\n";

        let events = parse_sse_json_events(body);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["id"], json!(1));
        assert_eq!(events[1]["result"]["done"], json!(true));
    }

    #[test]
    fn drops_non_json_sse_data_events() {
        let body = "data: keepalive\n\ndata: {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{}}\n\n";
        let events = parse_sse_json_events(body);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["id"], json!(2));
    }

    #[test]
    fn builds_jsonrpc_error_with_payload_id() {
        let payload = json!({"jsonrpc":"2.0","id":99,"method":"tools/call","params":{}});
        let error = jsonrpc_error_for_payload(&payload, -32000, "boom".to_string());

        assert_eq!(error["jsonrpc"], json!("2.0"));
        assert_eq!(error["id"], json!(99));
        assert_eq!(error["error"]["code"], json!(-32000));
    }

    #[test]
    fn derives_schemas_url_from_mcp_endpoint() {
        let url = schemas_url_from_mcp_url("http://127.0.0.1:43173/mcp").expect("valid URL");
        assert_eq!(url.as_str(), "http://127.0.0.1:43173/schemas");
    }

    #[tokio::test]
    async fn reads_newline_delimited_messages_skipping_blank_lines() {
        let mut input: &[u8] = b"{\"id\":1}\n\n  \r\n{\"id\":2}";
        assert_eq!(
            read_stdio_line(&mut input)
                .await
                .unwrap()
                .as_deref(),
            Some("{\"id\":1}")
        );
        assert_eq!(
            read_stdio_line(&mut input)
                .await
                .unwrap()
                .as_deref(),
            Some("{\"id\":2}")
        );
        assert_eq!(
            read_stdio_line(&mut input)
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn writes_one_message_per_line() {
        let mut output = Vec::new();
        let message = json!({"jsonrpc": "2.0", "id": 1, "result": {"text": "a\nb"}});
        write_stdio_jsonrpc_message(&mut output, &message)
            .await
            .unwrap();

        let encoded = String::from_utf8(output).unwrap();
        assert_eq!(encoded.matches('\n').count(), 1, "exactly one trailing newline: {encoded:?}");
        assert!(encoded.ends_with('\n'));
        assert_eq!(serde_json::from_str::<serde_json::Value>(encoded.trim()).unwrap(), message);
    }

    #[test]
    fn sessionless_requests_are_upgraded_to_stateless() {
        let payload =
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"ping"}});
        let (upstream, version) = prepare_upstream(&payload, false);
        assert_eq!(version.as_deref(), Some(LATEST_MCP_PROTOCOL_VERSION));
        assert_eq!(
            upstream["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"],
            json!(LATEST_MCP_PROTOCOL_VERSION)
        );
    }

    #[test]
    fn stateless_client_meta_is_preserved() {
        let payload = json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{"_meta":{
            "io.modelcontextprotocol/protocolVersion":"2099-01-01",
            "io.modelcontextprotocol/clientCapabilities":{}
        }}});
        let (upstream, version) = prepare_upstream(&payload, false);
        assert_eq!(version.as_deref(), Some("2099-01-01"));
        assert_eq!(upstream, payload);
    }

    #[test]
    fn legacy_messages_pass_through() {
        let initialize = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}});
        let initialized = json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}});
        let call = json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{}});
        assert_eq!(prepare_upstream(&initialize, false), (initialize.clone(), None));
        assert_eq!(prepare_upstream(&initialized, false), (initialized.clone(), None));
        assert_eq!(prepare_upstream(&call, true), (call.clone(), None));
    }
}
