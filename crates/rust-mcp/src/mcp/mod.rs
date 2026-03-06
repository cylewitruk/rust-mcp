/// Background indexing pipeline and refresh worker.
pub mod indexing;
/// DB-backed tool invocation metrics.
pub mod metrics;
/// Shared response model types.
pub mod models;
/// Per-tool-call progress reporting and request context.
pub mod progress;
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

/// Database-backed MCP session manager.
pub mod session;
mod transport;

pub use indexing::{
    run_cache_watcher, run_enrichment_maintenance, run_refresh_worker, run_registry_discovery,
    run_security_sync,
};
pub use session::run_session_reaper;
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

#[cfg(feature = "testing")]
/// Test-only wrapper that runs a single security sync scan.
pub async fn run_security_sync_scan_for_tests(
    state: &crate::state::AppState,
) -> indexing::security_sync::SecuritySyncRunOutcome {
    indexing::security_sync::run_security_sync_scan(state, None).await
}

#[cfg(feature = "testing")]
/// Test-only wrapper that runs a single cache watcher poll cycle.
pub async fn run_cache_watch_poll_for_tests(
    state: &crate::state::AppState,
    index: &mut indexing::cache_watcher::CacheIndex,
) -> indexing::cache_watcher::CacheWatchPollOutcome {
    let cache_root = state
        .config
        .cargo_registry_dir
        .join("cache");
    indexing::cache_watcher::poll_cache_changes(state, &cache_root, index).await
}
