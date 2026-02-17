//! Shared environment variable names and helpers for test harnesses.

/// Canonical environment variable names used by test harnesses.
pub mod vars {
    /// Optional Postgres image tag override for testcontainers.
    pub const RUST_MCP_TEST_POSTGRES_TAG: &str = "RUST_MCP_TEST_POSTGRES_TAG";
    /// Optional rust-mcp image tag override for e2e test container builds.
    pub const RUST_MCP_TEST_IMAGE_TAG: &str = "RUST_MCP_TEST_IMAGE_TAG";
    /// Runtime outbound firewall flag used by the rust-mcp container.
    pub const OUTBOUND_FIREWALL: &str = "OUTBOUND_FIREWALL";
    /// HTTP bind address for MCP endpoints.
    pub const MCP_HTTP_BIND: &str = "MCP_HTTP_BIND";
    /// Prometheus bind address.
    pub const PROMETHEUS_BIND: &str = "PROMETHEUS_BIND";
    /// Database connection URL.
    pub const DATABASE_URL: &str = "DATABASE_URL";
    /// Log filter string.
    pub const RUST_LOG: &str = "RUST_LOG";
}

/// Default environment variable values used by test harnesses.
pub mod defaults {
    /// Default Postgres image tag for local testcontainers.
    pub const POSTGRES_IMAGE_TAG: &str = "18-alpine";
    /// Disables outbound firewalling inside the rust-mcp container.
    pub const OUTBOUND_FIREWALL_DISABLED: &str = "false";
    /// Container bind address for MCP endpoints.
    pub const MCP_HTTP_BIND: &str = "0.0.0.0:43173";
    /// Container bind address for Prometheus metrics.
    pub const PROMETHEUS_BIND: &str = "0.0.0.0:9090";
    /// Container-local unix-socket Postgres URL.
    pub const DATABASE_URL: &str = "postgres://postgres@%2Frun%2Fpostgresql/rust_mcp";
    /// Default test container log level.
    pub const RUST_LOG: &str = "warn";
}

/// Reads an environment variable and returns `None` for unset/empty values.
pub fn optional_env_non_empty(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}
