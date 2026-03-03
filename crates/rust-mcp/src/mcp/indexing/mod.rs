/// Near-real-time registry cache watcher for `.crate` file detection.
pub mod cache_watcher;
/// On-demand and background indexing coordinator.
pub mod coordinator;
/// Proactive registry discovery and background crate scanning.
pub mod discovery;
/// Interaction-triggered freshness checks and TTL heuristics.
pub mod freshness;
/// `index.*` tool handlers (sync_crates, status, refresh).
pub mod handlers;
/// Local cargo registry source cache indexing.
pub mod local_cache;
/// Periodic enrichment maintenance (rustdoc JSON queue management).
pub mod maintenance;
/// Rustdoc JSON fetching, parsing, and symbol extraction.
pub mod rustdoc_json;
/// OSV and RustSec advisory synchronization.
pub mod security;
/// Durable refresh worker loop.
pub mod worker;

pub use cache_watcher::run_cache_watcher;
pub use discovery::{collect_local_versions_for_crate, run_registry_discovery};
pub use maintenance::run_enrichment_maintenance;
pub use worker::run_refresh_worker;
