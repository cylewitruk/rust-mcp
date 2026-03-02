use serde_json::{Value, json};

use super::{call_tool_payload, call_tool_response, first_content_text, initialized_container};

#[tokio::test]
async fn tool_ping_returns_pong_with_message() {
    let rust_mcp = initialized_container().await;

    let response =
        call_tool_response(&rust_mcp, "ping", json!({ "message": "one-tool-one-test" })).await;
    let text = first_content_text(&response, "ping");

    assert!(text.contains("pong"), "expected ping response to include pong, got: {text}");
    assert!(
        text.contains("one-tool-one-test"),
        "expected ping response to include original message, got: {text}"
    );
}

#[tokio::test]
async fn tool_schema_get_returns_single_contract_for_selected_tool() {
    let rust_mcp = initialized_container().await;

    let payload = call_tool_payload(&rust_mcp, "schema_get", json!({ "tool_name": "ping" })).await;
    assert_eq!(
        payload
            .get("total_tools")
            .and_then(Value::as_u64),
        Some(1),
        "expected schema.get to return exactly one schema entry: {payload}"
    );

    let first_schema = payload
        .get("schemas")
        .and_then(Value::as_array)
        .and_then(|schemas| schemas.first())
        .expect("schema_get should return one schema entry");
    assert_eq!(
        first_schema
            .get("tool_name")
            .and_then(Value::as_str),
        Some("ping"),
        "schema_get should return ping contract when filtered to ping: {first_schema}"
    );
    assert_eq!(
        first_schema
            .get("request")
            .and_then(|schema| schema.get("type"))
            .and_then(Value::as_str),
        Some("object"),
        "ping request schema should be object-shaped: {first_schema}"
    );
}
