use rust_mcp_testing::fixtures::{
    seed_crate_release, seed_source_file, seed_symbol, write_fixture_source_to_registry,
};

use super::{Value, common, json};

const MIXED_CASE_NAME: &str = "TinyUFO";
const SOURCE_CONTENT: &str = "pub fn new(capacity: usize) -> Self { todo!() }";

/// Seed a mixed-case crate with source and symbol data.
async fn seed_mixed_case_crate(ctx: &common::SeededMcpContext) {
    let seeded = seed_crate_release(&ctx.state.db, MIXED_CASE_NAME, "0.8.0", 5000, None)
        .await
        .expect("seed crate release");

    let sf_id = seed_source_file(
        &ctx.state.db,
        seeded.version_id,
        "src/lib.rs",
        Some("rust"),
        SOURCE_CONTENT,
    )
    .await
    .expect("seed source file");

    seed_symbol(&ctx.state.db, seeded.version_id, sf_id, "new", "function", 1, 1)
        .await
        .expect("seed symbol");

    write_fixture_source_to_registry(
        &ctx.registry_dir,
        MIXED_CASE_NAME,
        "0.8.0",
        "src/lib.rs",
        SOURCE_CONTENT,
    );
}

#[tokio::test]
async fn source_search_finds_mixed_case_crate_with_lowercase_filter() {
    let ctx = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");
    seed_mixed_case_crate(&ctx).await;

    let response = ctx
        .mcp
        .call_tool(
            "source_search",
            json!({
                "query": "fn new",
                "crate_name": "tinyufo",
                "limit": 10
            }),
        )
        .await
        .expect("source_search call failed");
    let payload = common::structured_content(&response);

    assert!(
        payload
            .get("count")
            .and_then(Value::as_u64)
            .unwrap_or_default()
            >= 1,
        "lowercase crate_name should match mixed-case TinyUFO"
    );
}

#[tokio::test]
async fn source_read_finds_mixed_case_crate_with_lowercase_filter() {
    let ctx = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");
    seed_mixed_case_crate(&ctx).await;

    let response = ctx
        .mcp
        .call_tool(
            "source_read",
            json!({
                "crate_name": "tinyufo",
                "path": "src/lib.rs",
                "start_line": 1,
                "end_line": 1
            }),
        )
        .await
        .expect("source_read call failed");
    let payload = common::structured_content(&response);

    assert!(
        payload
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .contains("fn new"),
        "lowercase crate_name should read source from mixed-case TinyUFO"
    );
}

#[tokio::test]
async fn symbol_search_finds_mixed_case_crate_with_lowercase_filter() {
    let ctx = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");
    seed_mixed_case_crate(&ctx).await;

    let response = ctx
        .mcp
        .call_tool(
            "symbol_search",
            json!({
                "query": "new",
                "crate_name": "tinyufo",
                "limit": 10
            }),
        )
        .await
        .expect("symbol_search call failed");
    let payload = common::structured_content(&response);

    assert!(
        payload
            .get("count")
            .and_then(Value::as_u64)
            .unwrap_or_default()
            >= 1,
        "lowercase crate_name should match mixed-case TinyUFO symbols"
    );
}

#[tokio::test]
async fn crate_intel_finds_mixed_case_crate_with_lowercase_name() {
    let ctx = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded MCP context");
    seed_mixed_case_crate(&ctx).await;

    let response = ctx
        .mcp
        .call_tool(
            "crate_intel",
            json!({
                "crate_name": "tinyufo"
            }),
        )
        .await
        .expect("crate_intel call failed");
    let payload = common::structured_content(&response);

    assert_eq!(
        payload
            .get("crate_name")
            .and_then(Value::as_str),
        Some(MIXED_CASE_NAME),
        "crate_intel should return the canonical mixed-case name"
    );
}
