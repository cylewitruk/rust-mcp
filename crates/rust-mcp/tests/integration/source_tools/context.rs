use super::{Value, common, json};

#[tokio::test]
async fn source_context_resolves_line_from_symbol_name() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    let response = context
        .mcp
        .call_tool(
            "source_context",
            json!({
                "crate_name": "serde_json",
                "path": "src/lib.rs",
                "symbol_name": "from_str"
            }),
        )
        .await
        .expect("source_context call failed");
    let payload = common::structured_content(&response);

    assert_eq!(
        payload
            .get("module_path")
            .and_then(Value::as_str),
        Some("serde_json")
    );
    assert_eq!(
        payload
            .get("line")
            .and_then(Value::as_u64),
        Some(1)
    );
    assert_eq!(
        payload
            .get("symbol_name")
            .and_then(Value::as_str),
        Some("from_str")
    );
}
