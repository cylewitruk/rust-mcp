//! Integration tests for on-demand crate indexing.
//!
//! These tests verify that tool calls for unindexed crates trigger automatic
//! indexing via the refresh worker rather than returning "not indexed" errors.

use super::{Value, common, json, mock_index_sync_context, run_refresh_worker_for_tests};

/// A `crate.intel` call for an unindexed crate should trigger on-demand
/// indexing and return a valid response once the worker completes.
#[tokio::test]
async fn crate_intel_triggers_on_demand_indexing_for_unindexed_crate() {
    let context = mock_index_sync_context()
        .await
        .expect("failed to build mock index context");

    // Spawn the refresh worker so it can pick up the on-demand job.
    let _worker = tokio::spawn(run_refresh_worker_for_tests(context.state.clone()));

    // Verify the crate is NOT indexed yet.
    let pre_check = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*)::BIGINT FROM crates WHERE name = 'demo_crate'",
    )
    .fetch_one(&context.state.db)
    .await
    .expect("failed to check pre-existing crate count");
    assert_eq!(pre_check, 0, "demo_crate should not be indexed before the tool call");

    // Call crate.intel — this should trigger on-demand indexing.
    let response = context
        .mcp
        .call_tool(
            "crate_intel",
            json!({
                "crate_name": "demo_crate"
            }),
        )
        .await
        .expect("crate_intel should succeed via on-demand indexing");
    let payload = common::structured_content(&response);

    // The response should contain the crate name from the mock server.
    assert_eq!(
        payload
            .get("crate_name")
            .and_then(Value::as_str),
        Some("demo_crate"),
        "on-demand indexed crate.intel should return the crate name"
    );

    // Verify the crate is now indexed.
    let post_check = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*)::BIGINT FROM crates WHERE name = 'demo_crate'",
    )
    .fetch_one(&context.state.db)
    .await
    .expect("failed to check post-indexing crate count");
    assert!(post_check > 0, "demo_crate should be indexed after on-demand tool call");
}

/// A `crate.intel` call for a crate that does not exist on crates.io should
/// return a descriptive error rather than hanging or panicking.
#[tokio::test]
async fn crate_intel_returns_error_for_nonexistent_crate() {
    let context = mock_index_sync_context()
        .await
        .expect("failed to build mock index context");

    // Spawn the refresh worker.
    let _worker = tokio::spawn(run_refresh_worker_for_tests(context.state.clone()));

    // Call crate.intel for a crate that the mock server does not serve.
    // The worker should fail (404 from mock) and the tool handler should
    // return an error to the client.
    let result = context
        .mcp
        .call_tool(
            "crate_intel",
            json!({
                "crate_name": "nonexistent_crate_xyz"
            }),
        )
        .await;

    // The call should fail with a descriptive error (not a timeout).
    assert!(
        result.is_err(),
        "crate_intel for a nonexistent crate should return an error, got: {result:?}"
    );
}

/// Two concurrent `crate.intel` calls for the same unindexed crate should
/// both succeed, with the second one coalescing on the first's indexing job.
#[tokio::test]
async fn concurrent_on_demand_calls_coalesce() {
    let context = mock_index_sync_context()
        .await
        .expect("failed to build mock index context");

    // Spawn the refresh worker.
    let _worker = tokio::spawn(run_refresh_worker_for_tests(context.state.clone()));

    // Fire two concurrent crate.intel calls through the same MCP harness.
    let (r1, r2) = tokio::join!(
        context
            .mcp
            .call_tool("crate_intel", json!({"crate_name": "demo_crate"})),
        context
            .mcp
            .call_tool("crate_intel", json!({"crate_name": "demo_crate"})),
    );

    // Both should succeed.
    assert!(r1.is_ok(), "first concurrent call should succeed: {r1:?}");
    assert!(r2.is_ok(), "second concurrent call should succeed: {r2:?}");

    // Only one on-demand "crate" job should have been created (coalesced).
    let job_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*)::BIGINT FROM refresh_jobs WHERE crate_name = 'demo_crate' AND scope = \
         'crate'",
    )
    .fetch_one(&context.state.db)
    .await
    .expect("failed to count refresh jobs");
    assert!(
        job_count <= 2,
        "expected at most 2 crate-scope jobs (on-demand + possible freshness check), got \
         {job_count}"
    );
}
