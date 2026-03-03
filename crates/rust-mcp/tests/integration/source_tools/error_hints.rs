use super::{common, json};

#[tokio::test]
async fn source_read_error_includes_alternative_tool_hints() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    // Request a file path that doesn't exist in the seeded data.
    // The harness converts tool-level isError responses into Err values,
    // so we expect an error here and check the error message text.
    let err = context
        .mcp
        .call_tool(
            "source_read",
            json!({
                "crate_name": "serde_json",
                "version": "1.0.145",
                "path": "src/nonexistent.rs"
            }),
        )
        .await
        .expect_err("source_read should return tool-level error for missing file");

    let error_text = err.to_string();
    assert!(
        error_text.contains("crate_api") || error_text.contains("symbol_search"),
        "error message should suggest alternative tools, got: {error_text}"
    );
}

#[tokio::test]
async fn source_context_error_includes_alternative_tool_hints() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");

    // Request a file path that doesn't exist.
    let err = context
        .mcp
        .call_tool(
            "source_context",
            json!({
                "crate_name": "serde_json",
                "version": "1.0.145",
                "path": "src/nonexistent.rs",
                "line": 1
            }),
        )
        .await
        .expect_err("source_context should return tool-level error for missing file");

    let error_text = err.to_string();
    assert!(
        error_text.contains("crate_api") || error_text.contains("symbol_search"),
        "error message should suggest alternative tools, got: {error_text}"
    );
}
