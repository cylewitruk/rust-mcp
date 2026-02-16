use serde_json::json;

use super::{call_tool_response, first_content_text, initialized_container};

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
