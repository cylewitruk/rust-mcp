//! Stdio-to-HTTP MCP adapter for running rust-mcp behind stdio-only clients.

use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use clap::Parser;
use rust_mcp_types::protocol::LATEST_MCP_PROTOCOL_VERSION;
use rust_mcp_types::types::schema::ToolSchemasResponse;
use serde_json::{Value, json};
use tokio::io::{
    self, AsyncBufRead, AsyncBufReadExt as _, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _,
    BufReader, BufWriter,
};
use tracing::{debug, error, info, warn};

/// MCP session header name per Streamable HTTP transport behavior.
const MCP_SESSION_ID_HEADER: &str = "mcp-session-id";

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

    /// Automatically establish an upstream MCP session for requests that
    /// require one (for short-lived one-request-per-process clients).
    #[arg(long, env = "MCP_AUTO_BOOTSTRAP_SESSION", default_value_t = true)]
    auto_bootstrap_session: bool,
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
    let mut session_id: Option<String> = None;

    while let Some(payload) = read_stdio_jsonrpc_message(&mut reader).await? {
        let is_request = payload.get("id").is_some();

        let mut upstream_messages = forward_to_http(
            &client,
            &args.mcp_url,
            &mut session_id,
            &payload,
            args.auto_bootstrap_session,
        )
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

async fn read_stdio_jsonrpc_message<R>(reader: &mut R) -> Result<Option<Value>>
where
    R: AsyncBufRead + Unpin,
{
    let mut content_length: Option<usize> = None;
    let mut saw_any_header_line = false;

    loop {
        let mut line = String::new();
        let bytes = reader
            .read_line(&mut line)
            .await
            .context("failed to read stdio header line")?;

        if bytes == 0 {
            if !saw_any_header_line {
                return Ok(None);
            }
            bail!("unexpected EOF while reading stdio message headers")
        }

        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }

        saw_any_header_line = true;

        let Some((name, value)) = trimmed.split_once(':') else {
            continue;
        };

        if name.eq_ignore_ascii_case("content-length") {
            let parsed = value
                .trim()
                .parse::<usize>()
                .with_context(|| format!("invalid Content-Length header value: `{value}`"))?;
            content_length = Some(parsed);
        }
    }

    let content_length = content_length.ok_or_else(|| anyhow!("missing Content-Length header"))?;
    let mut body = vec![0_u8; content_length];
    reader
        .read_exact(&mut body)
        .await
        .context("failed to read stdio JSON-RPC payload")?;

    let payload = serde_json::from_slice::<Value>(&body).with_context(|| {
        format!("invalid JSON-RPC payload on stdio: {}", String::from_utf8_lossy(&body))
    })?;
    Ok(Some(payload))
}

async fn write_stdio_jsonrpc_message<W>(writer: &mut W, payload: &Value) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    let encoded = serde_json::to_vec(payload).context("failed to encode JSON-RPC payload")?;
    let header =
        format!("Content-Length: {}\r\nContent-Type: application/json\r\n\r\n", encoded.len());

    writer
        .write_all(header.as_bytes())
        .await
        .context("failed to write stdio JSON-RPC headers")?;
    writer
        .write_all(&encoded)
        .await
        .context("failed to write stdio JSON-RPC body")?;
    writer
        .flush()
        .await
        .context("failed to flush stdio output")?;

    Ok(())
}

async fn forward_to_http(
    client: &reqwest::Client,
    mcp_url: &str,
    session_id: &mut Option<String>,
    payload: &Value,
    auto_bootstrap_session: bool,
) -> Result<Vec<Value>> {
    if auto_bootstrap_session && should_auto_bootstrap_session(session_id, payload) {
        bootstrap_upstream_session(client, mcp_url, session_id).await?;
    }

    send_upstream_http(client, mcp_url, session_id, payload).await
}

fn should_auto_bootstrap_session(session_id: &Option<String>, payload: &Value) -> bool {
    if session_id.is_some() {
        return false;
    }

    let Some(method) = payload
        .get("method")
        .and_then(Value::as_str)
    else {
        return false;
    };

    !matches!(method, "initialize" | "notifications/initialized")
}

async fn bootstrap_upstream_session(
    client: &reqwest::Client,
    mcp_url: &str,
    session_id: &mut Option<String>,
) -> Result<()> {
    let initialize_payload = json!({
        "jsonrpc": "2.0",
        "id": "rust-mcp-stdio-bootstrap-init",
        "method": "initialize",
        "params": {
            "protocolVersion": LATEST_MCP_PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {
                "name": "rust-mcp-stdio",
                "version": env!("CARGO_PKG_VERSION")
            }
        }
    });

    let initialize_response =
        send_upstream_http(client, mcp_url, session_id, &initialize_payload).await?;
    let initialize_ok = initialize_response
        .iter()
        .any(|message| {
            message
                .get("result")
                .is_some()
        });
    if !initialize_ok {
        bail!("failed to bootstrap MCP session: upstream initialize returned no result payload");
    }

    let initialized_payload = json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized",
        "params": {}
    });

    let _ = send_upstream_http(client, mcp_url, session_id, &initialized_payload).await?;
    Ok(())
}

async fn send_upstream_http(
    client: &reqwest::Client,
    mcp_url: &str,
    session_id: &mut Option<String>,
    payload: &Value,
) -> Result<Vec<Value>> {
    let mut request = client
        .post(mcp_url)
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream");

    if let Some(existing_session) = session_id.as_deref() {
        request = request.header(MCP_SESSION_ID_HEADER, existing_session);
    }

    let response = request
        .json(payload)
        .send()
        .await
        .context("failed to send upstream MCP HTTP request")?;

    if let Some(new_session_id) = response
        .headers()
        .get(MCP_SESSION_ID_HEADER)
        .and_then(|value| value.to_str().ok())
    {
        *session_id = Some(new_session_id.to_string());
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
    use serde_json::json;

    use super::{
        jsonrpc_error_for_payload, parse_sse_json_events, schemas_url_from_mcp_url,
        should_auto_bootstrap_session,
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

    #[test]
    fn bootstrap_required_for_non_initialize_requests_without_session() {
        let payload = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{}});
        assert!(should_auto_bootstrap_session(&None, &payload));
    }

    #[test]
    fn bootstrap_not_required_for_initialize_messages() {
        let initialize = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}});
        let initialized = json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}});
        assert!(!should_auto_bootstrap_session(&None, &initialize));
        assert!(!should_auto_bootstrap_session(&None, &initialized));
    }
}
