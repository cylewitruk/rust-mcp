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
/// Rustdoc JSON fetching, parsing, and symbol extraction.
pub mod rustdoc_json;
/// OSV and RustSec advisory synchronization.
pub mod security;
/// Durable refresh worker loop.
pub mod worker;

pub use discovery::{collect_local_versions_for_crate, run_registry_discovery};
#[cfg(feature = "testing")]
pub use worker::run_startup_rustdoc_json_refresh_with_page_size;
pub use worker::{run_refresh_worker, run_startup_rustdoc_json_refresh};
