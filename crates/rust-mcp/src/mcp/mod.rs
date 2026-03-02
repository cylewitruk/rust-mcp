/// Background indexing pipeline and refresh worker.
pub mod indexing;
/// DB-backed tool invocation metrics.
pub mod metrics;
/// Shared response model types.
pub mod models;
/// Common crate/version resolution queries.
pub mod queries;
/// Short-lived query result memoization.
pub mod query_cache;
/// Ripgrep wrapper for structured source-file content search.
pub mod ripgrep;
/// MCP server handler and tool registration.
pub mod server;
/// MCP tool handler implementations.
pub mod tools;
/// Parameter validation and pagination utilities.
pub mod utils;

mod transport;

pub use indexing::{run_enrichment_maintenance, run_refresh_worker, run_registry_discovery};
pub use transport::streamable_http_service;

#[cfg(feature = "testing")]
/// Test-only wrapper that runs the refresh worker loop for integration tests.
pub async fn run_refresh_worker_for_tests(state: crate::state::AppState) {
    indexing::run_refresh_worker(state).await;
}

#[cfg(feature = "testing")]
/// Test-only wrapper that runs a single enrichment maintenance scan.
pub async fn run_enrichment_maintenance_scan_for_tests(
    state: &crate::state::AppState,
) -> indexing::maintenance::EnrichmentMaintenanceOutcome {
    indexing::maintenance::run_enrichment_maintenance_scan(state).await
}

#[cfg(feature = "testing")]
/// Test-only wrapper that runs a single registry discovery scan.
pub async fn run_registry_scan_for_tests(state: &crate::state::AppState) {
    indexing::discovery::run_registry_scan(state).await;
}

#[cfg(feature = "testing")]
/// Test-only wrapper that runs a registry scan and returns the outcome.
pub async fn run_registry_scan_with_outcome_for_tests(
    state: &crate::state::AppState,
) -> indexing::discovery::DiscoveryScanOutcome {
    indexing::discovery::run_registry_scan(state).await
}
