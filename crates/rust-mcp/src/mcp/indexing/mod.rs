pub(crate) mod coordinator;
pub(crate) mod discovery;
pub(crate) mod freshness;
pub(crate) mod handlers;
pub(crate) mod local_cache;
pub(crate) mod rustdoc_json;
pub(crate) mod security;
pub(crate) mod worker;

pub(crate) use discovery::run_registry_discovery;
#[cfg(feature = "integration-tests")]
pub(crate) use worker::run_startup_rustdoc_json_refresh_with_page_size;
pub(crate) use worker::{run_refresh_worker, run_startup_rustdoc_json_refresh};
