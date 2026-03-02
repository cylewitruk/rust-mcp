use metrics::{describe_counter, describe_gauge, describe_histogram};

use super::server::McpServer;
use crate::db::tools;

impl McpServer {
    /// Persists a tool invocation record to the database for metrics tracking.
    pub async fn record_tool_invocation(
        &self,
        tool_name: &str,
        success: bool,
        latency_ms: i64,
    ) -> Result<(), String> {
        tools::insert_tool_invocation(&self.state.db, tool_name, success, latency_ms)
            .await
            .map_err(|e| format!("failed to record tool invocation for {tool_name}: {e}"))?;

        Ok(())
    }
}

/// Registers HELP descriptions for all Prometheus metrics exported by the
/// application. Call once after `PrometheusBuilder::install()`.
pub fn register_metric_descriptions() {
    // -- Tool invocation metrics (server.rs) --
    describe_counter!(
        "rust_mcp_tool_invocations_total",
        "Total MCP tool calls, labeled by tool name and success/failure"
    );
    describe_histogram!(
        "rust_mcp_tool_latency_ms",
        "Tool call latency in milliseconds, labeled by tool name"
    );

    // -- Refresh worker gauges (worker.rs) --
    describe_gauge!(
        "rust_mcp_refresh_jobs_pending",
        "Number of refresh jobs waiting to be processed"
    );
    describe_gauge!("rust_mcp_refresh_jobs_running", "Number of refresh jobs currently executing");
    describe_gauge!("rust_mcp_refresh_jobs_failed", "Number of refresh jobs in failed state");
    describe_gauge!(
        "rust_mcp_refresh_jobs_background_ratio",
        "Ratio of background (discovery/enrichment) jobs to total pending jobs"
    );
    describe_gauge!(
        "rust_mcp_refresh_jobs_scope_pending",
        "Pending refresh jobs broken down by scope (crate, rustdoc_json, local_cache, etc.)"
    );

    // -- Refresh worker completion counters --
    describe_counter!(
        "rust_mcp_refresh_jobs_completed_total",
        "Total refresh jobs completed successfully, labeled by scope"
    );
    describe_counter!(
        "rust_mcp_refresh_jobs_errored_total",
        "Total refresh jobs that failed (terminal or retriable), labeled by scope"
    );

    // -- Indexing outcome counters --
    describe_counter!(
        "rust_mcp_crate_versions_synced_total",
        "Total crate versions whose metadata was synced from crates.io"
    );
    describe_counter!(
        "rust_mcp_dependencies_synced_total",
        "Total dependency edges synced from crates.io"
    );
    describe_counter!(
        "rust_mcp_rustdoc_symbols_written_total",
        "Total symbols extracted and written from rustdoc JSON"
    );
    describe_counter!(
        "rust_mcp_rustdoc_types_written_total",
        "Total type definitions extracted from rustdoc JSON"
    );
    describe_counter!(
        "rust_mcp_rustdoc_impls_written_total",
        "Total impl blocks extracted from rustdoc JSON"
    );
    describe_counter!(
        "rust_mcp_rustdoc_traits_written_total",
        "Total trait definitions extracted from rustdoc JSON"
    );
    describe_counter!(
        "rust_mcp_source_files_upserted_total",
        "Total source files upserted from local cargo registry cache"
    );
    describe_counter!(
        "rust_mcp_docs_pages_written_total",
        "Total documentation pages written from docs.rs"
    );
    describe_counter!(
        "rust_mcp_security_advisories_written_total",
        "Total security advisories written from OSV/RustSec"
    );

    // -- Discovery metrics (discovery.rs) --
    describe_counter!("rust_mcp_discovery_scans_total", "Total registry discovery scans performed");
    describe_counter!(
        "rust_mcp_discovery_jobs_enqueued_total",
        "Total refresh jobs enqueued by registry discovery"
    );
    describe_counter!(
        "rust_mcp_discovery_scan_errors_total",
        "Total errors encountered during registry discovery scans"
    );
    describe_histogram!(
        "rust_mcp_discovery_scan_duration_ms",
        "Duration of registry discovery scans in milliseconds"
    );
}
