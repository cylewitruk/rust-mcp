//! Shared MCP protocol compatibility constants and stateless request helpers.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
pub use rmcp::transport::common::http_header::HEADER_SESSION_ID as MCP_SESSION_ID_HEADER;
use rmcp::transport::common::http_header::{
    BASE64_HEADER_PREFIX, BASE64_HEADER_SUFFIX, HEADER_MCP_METHOD, HEADER_MCP_NAME,
    HEADER_MCP_PROTOCOL_VERSION,
};
use serde_json::{Map, Value, json};

/// Latest MCP protocol version; stateless (no `initialize`, no sessions).
pub const LATEST_MCP_PROTOCOL_VERSION: &str = "2026-07-28";

/// Newest legacy (session-based, `initialize` handshake) MCP protocol version.
pub const LEGACY_MCP_PROTOCOL_VERSION: &str = "2025-11-25";

const META_KEY: &str = "_meta";
const META_PROTOCOL_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
const META_CLIENT_INFO: &str = "io.modelcontextprotocol/clientInfo";
const META_CLIENT_CAPABILITIES: &str = "io.modelcontextprotocol/clientCapabilities";

/// Methods whose `Mcp-Name` header is sourced from `params.name`.
const NAME_FROM_NAME: &[&str] = &["tools/call", "prompts/get"];
/// Methods whose `Mcp-Name` header is sourced from `params.uri`.
const NAME_FROM_URI: &[&str] = &["resources/read"];

/// Returns the protocol version carried in a JSON-RPC message's
/// `params._meta`, if any.
pub fn request_meta_protocol_version(message: &Value) -> Option<&str> {
    message
        .get("params")?
        .get(META_KEY)?
        .get(META_PROTOCOL_VERSION)?
        .as_str()
}

/// Adds the per-request `_meta` keys required by the stateless protocol
/// ([`LATEST_MCP_PROTOCOL_VERSION`]) to a JSON-RPC message, preserving any
/// existing `_meta` entries (e.g. `progressToken`).
pub fn insert_stateless_request_meta(message: &mut Value, client_name: &str, client_version: &str) {
    let Some(object) = message.as_object_mut() else {
        return;
    };
    let params = object
        .entry("params")
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(params) = params.as_object_mut() else {
        return;
    };
    let meta = params
        .entry(META_KEY)
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(meta) = meta.as_object_mut() else {
        return;
    };
    meta.insert(META_PROTOCOL_VERSION.to_string(), json!(LATEST_MCP_PROTOCOL_VERSION));
    meta.entry(META_CLIENT_CAPABILITIES)
        .or_insert_with(|| json!({}));
    meta.entry(META_CLIENT_INFO)
        .or_insert_with(|| json!({ "name": client_name, "version": client_version }));
}

/// Builds the HTTP headers a stateless request must carry:
/// `MCP-Protocol-Version` plus the standard `Mcp-Method` / `Mcp-Name` routing
/// headers.
pub fn stateless_request_headers(
    message: &Value,
    protocol_version: &str,
) -> Vec<(&'static str, String)> {
    let mut headers = vec![(HEADER_MCP_PROTOCOL_VERSION, protocol_version.to_string())];
    let Some(method) = message
        .get("method")
        .and_then(Value::as_str)
    else {
        return headers;
    };
    headers.push((HEADER_MCP_METHOD, method.to_string()));

    let name_key = if NAME_FROM_NAME.contains(&method) {
        Some("name")
    } else if NAME_FROM_URI.contains(&method) {
        Some("uri")
    } else {
        None
    };
    let name = name_key.and_then(|key| {
        message
            .get("params")?
            .get(key)?
            .as_str()
    });
    if let Some(name) = name {
        headers.push((HEADER_MCP_NAME, encode_header_value(name)));
    }
    headers
}

/// Encodes a header value per the spec's value-encoding rules: values that
/// cannot travel verbatim (control or non-ASCII characters, leading/trailing
/// whitespace) or that already look like the sentinel are Base64-wrapped as
/// `=?base64?...?=`.
fn encode_header_value(value: &str) -> String {
    if requires_base64(value) {
        format!("{BASE64_HEADER_PREFIX}{}{BASE64_HEADER_SUFFIX}", BASE64.encode(value))
    } else {
        value.to_string()
    }
}

fn requires_base64(value: &str) -> bool {
    if value.is_empty() {
        return false;
    }
    value.starts_with([' ', '\t'])
        || value.ends_with([' ', '\t'])
        || value
            .chars()
            .any(|c| !(' '..='~').contains(&c))
        || (value.starts_with(BASE64_HEADER_PREFIX) && value.ends_with(BASE64_HEADER_SUFFIX))
}

#[cfg(test)]
mod tests {
    use rmcp::model::ProtocolVersion;
    use serde_json::json;

    use super::*;

    #[test]
    fn version_constants_track_rmcp() {
        assert_eq!(LATEST_MCP_PROTOCOL_VERSION, ProtocolVersion::LATEST.as_str());
        assert_eq!(LEGACY_MCP_PROTOCOL_VERSION, ProtocolVersion::LATEST_WITH_INITIALIZE.as_str());
    }

    #[test]
    fn inserts_meta_without_clobbering_existing_keys() {
        let mut message = json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": "ping", "_meta": {"progressToken": "p1"}},
        });
        insert_stateless_request_meta(&mut message, "client", "1.0");

        let meta = &message["params"]["_meta"];
        assert_eq!(meta["progressToken"], "p1");
        assert_eq!(meta[META_PROTOCOL_VERSION], LATEST_MCP_PROTOCOL_VERSION);
        assert_eq!(meta[META_CLIENT_CAPABILITIES], json!({}));
        assert_eq!(meta[META_CLIENT_INFO]["name"], "client");
        assert_eq!(request_meta_protocol_version(&message), Some(LATEST_MCP_PROTOCOL_VERSION));
    }

    #[test]
    fn inserts_params_when_missing() {
        let mut message = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"});
        insert_stateless_request_meta(&mut message, "client", "1.0");
        assert_eq!(request_meta_protocol_version(&message), Some(LATEST_MCP_PROTOCOL_VERSION));
    }

    #[test]
    fn builds_standard_headers() {
        let call = json!({"method": "tools/call", "params": {"name": "crate_search"}});
        assert_eq!(
            stateless_request_headers(&call, LATEST_MCP_PROTOCOL_VERSION),
            vec![
                (HEADER_MCP_PROTOCOL_VERSION, LATEST_MCP_PROTOCOL_VERSION.to_string()),
                (HEADER_MCP_METHOD, "tools/call".to_string()),
                (HEADER_MCP_NAME, "crate_search".to_string()),
            ]
        );

        let list = json!({"method": "tools/list", "params": {}});
        assert_eq!(stateless_request_headers(&list, LATEST_MCP_PROTOCOL_VERSION).len(), 2);
    }

    #[test]
    fn encodes_names_that_cannot_travel_verbatim() {
        let name_header = |name: &str| {
            let call = json!({"method": "tools/call", "params": {"name": name}});
            stateless_request_headers(&call, LATEST_MCP_PROTOCOL_VERSION)
                .into_iter()
                .find(|(header, _)| *header == HEADER_MCP_NAME)
                .map(|(_, value)| value)
        };

        assert_eq!(name_header("plain_name").as_deref(), Some("plain_name"));
        assert_eq!(name_header("ツール").as_deref(), Some("=?base64?44OE44O844Or?="));
        assert_eq!(name_header(" padded").as_deref(), Some("=?base64?IHBhZGRlZA==?="));
        assert_eq!(
            name_header("=?base64?abc?=").as_deref(),
            Some("=?base64?PT9iYXNlNjQ/YWJjPz0=?=")
        );
    }
}
