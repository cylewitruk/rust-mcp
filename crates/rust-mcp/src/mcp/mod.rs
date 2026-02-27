pub(crate) mod indexing;
pub(crate) mod metrics;
pub(crate) mod models;
pub(crate) mod queries;
pub(crate) mod query_cache;
pub(crate) mod server;
pub(crate) mod tools;
mod transport;
pub(crate) mod utils;

pub(crate) use indexing::{
    run_refresh_worker, run_registry_discovery, run_startup_rustdoc_json_refresh,
};
pub use transport::streamable_http_service;

#[cfg(feature = "integration-tests")]
/// Test-only wrapper that runs the refresh worker loop for integration tests.
pub async fn run_refresh_worker_for_tests(state: crate::state::AppState) {
    indexing::run_refresh_worker(state).await;
}

#[cfg(feature = "integration-tests")]
/// Test-only wrapper that runs startup rustdoc sync with a custom page size.
pub async fn run_startup_rustdoc_json_refresh_for_tests(
    state: crate::state::AppState,
    per_page: u32,
) {
    indexing::run_startup_rustdoc_json_refresh_with_page_size(state, per_page).await;
}

#[cfg(feature = "integration-tests")]
/// Test-only wrapper that runs a single registry discovery scan.
pub async fn run_registry_scan_for_tests(state: &crate::state::AppState) {
    indexing::discovery::run_registry_scan(state).await;
}
