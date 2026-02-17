use std::net::SocketAddr;
use std::path::PathBuf;

use clap::{Parser, ValueEnum};

/// Canonical environment variable names used by server config.
pub(crate) mod env_vars {
    pub(crate) const MCP_TRANSPORT: &str = "MCP_TRANSPORT";
    pub(crate) const MCP_HTTP_BIND: &str = "MCP_HTTP_BIND";
    pub(crate) const MCP_SSE_KEEP_ALIVE_SECS: &str = "MCP_SSE_KEEP_ALIVE_SECS";
    pub(crate) const MCP_SSE_RETRY_MS: &str = "MCP_SSE_RETRY_MS";
    pub(crate) const DATABASE_URL: &str = "DATABASE_URL";
    pub(crate) const CRATES_IO_BASE_URL: &str = "CRATES_IO_BASE_URL";
    pub(crate) const CRATES_IO_USER_AGENT: &str = "CRATES_IO_USER_AGENT";
    pub(crate) const CRATES_IO_TIMEOUT_SECS: &str = "CRATES_IO_TIMEOUT_SECS";
    pub(crate) const CRATES_IO_MIN_INTERVAL_MS: &str = "CRATES_IO_MIN_INTERVAL_MS";
    pub(crate) const DOCS_RS_BASE_URL: &str = "DOCS_RS_BASE_URL";
    pub(crate) const DOCS_RS_MIN_INTERVAL_MS: &str = "DOCS_RS_MIN_INTERVAL_MS";
    pub(crate) const OSV_MIN_INTERVAL_MS: &str = "OSV_MIN_INTERVAL_MS";
    pub(crate) const DATABASE_MIN_CONNECTIONS: &str = "DATABASE_MIN_CONNECTIONS";
    pub(crate) const DATABASE_MAX_CONNECTIONS: &str = "DATABASE_MAX_CONNECTIONS";
    pub(crate) const MAX_CONCURRENT_REQUESTS: &str = "MAX_CONCURRENT_REQUESTS";
    pub(crate) const PROMETHEUS_BIND: &str = "PROMETHEUS_BIND";
    pub(crate) const AUTO_MIGRATE: &str = "AUTO_MIGRATE";
    pub(crate) const CARGO_REGISTRY_DIR: &str = "CARGO_REGISTRY_DIR";
    pub(crate) const MCP_DATA_DIR: &str = "MCP_DATA_DIR";
    pub(crate) const RUSTSEC_DB_DIR: &str = "RUSTSEC_DB_DIR";
    pub(crate) const RUSTDOC_JSON_DIR: &str = "RUSTDOC_JSON_DIR";
    pub(crate) const RUST_LOG: &str = "RUST_LOG";
    pub(crate) const LOG_FORMAT: &str = "LOG_FORMAT";
}

/// Reads an environment variable and returns `None` for unset/empty values.
pub fn optional_env_non_empty(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Runtime configuration for the server process.
#[derive(Debug, Clone, Parser)]
#[command(
    name = "rust-mcp",
    version,
    about = "Local-first Rust dependency intelligence MCP server"
)]
pub struct Config {
    /// MCP transport mode to use for serving clients.
    #[arg(long, env = env_vars::MCP_TRANSPORT, value_enum, default_value_t = TransportMode::Http)]
    pub mcp_transport: TransportMode,

    /// HTTP bind address for health/readiness and streamable MCP endpoints.
    #[arg(long, env = env_vars::MCP_HTTP_BIND, default_value = "127.0.0.1:43173")]
    pub http_bind: SocketAddr,

    /// SSE keep-alive interval in seconds for streamable MCP responses.
    #[arg(long, env = env_vars::MCP_SSE_KEEP_ALIVE_SECS, default_value_t = 15)]
    pub mcp_sse_keep_alive_secs: u64,

    /// SSE retry delay in milliseconds for reconnecting clients.
    #[arg(long, env = env_vars::MCP_SSE_RETRY_MS, default_value_t = 3000)]
    pub mcp_sse_retry_ms: u64,

    /// PostgreSQL connection string (unix socket by default in Docker).
    #[arg(
        long,
        env = env_vars::DATABASE_URL,
        default_value = "postgres://postgres@%2Frun%2Fpostgresql/rust_mcp"
    )]
    pub database_url: String,

    /// Base URL for crates.io API calls.
    #[arg(long, env = env_vars::CRATES_IO_BASE_URL, default_value = "https://crates.io")]
    pub crates_io_base_url: String,

    /// User-Agent sent to crates.io and other remote APIs.
    #[arg(
        long,
        env = env_vars::CRATES_IO_USER_AGENT,
        default_value = "rust-mcp/0.1.0 (local dev machine)"
    )]
    pub crates_io_user_agent: String,

    /// HTTP timeout for crates.io/OSV requests.
    #[arg(long, env = env_vars::CRATES_IO_TIMEOUT_SECS, default_value_t = 20)]
    pub crates_io_timeout_secs: u64,

    /// Minimum delay between outbound crates.io requests (per process) in
    /// milliseconds.
    #[arg(long, env = env_vars::CRATES_IO_MIN_INTERVAL_MS, default_value_t = 100)]
    pub crates_io_min_interval_ms: u64,

    /// Base URL for docs.rs page fetches.
    #[arg(long, env = env_vars::DOCS_RS_BASE_URL, default_value = "https://docs.rs")]
    pub docs_rs_base_url: String,

    /// Minimum delay between outbound docs.rs requests (per process) in
    /// milliseconds.
    #[arg(long, env = env_vars::DOCS_RS_MIN_INTERVAL_MS, default_value_t = 120)]
    pub docs_rs_min_interval_ms: u64,

    /// Minimum delay between outbound OSV requests (per process) in
    /// milliseconds.
    #[arg(long, env = env_vars::OSV_MIN_INTERVAL_MS, default_value_t = 150)]
    pub osv_min_interval_ms: u64,

    /// Minimum database connection pool size.
    #[arg(long, env = env_vars::DATABASE_MIN_CONNECTIONS, default_value_t = 1)]
    pub database_min_connections: u32,

    /// Maximum database connection pool size.
    #[arg(long, env = env_vars::DATABASE_MAX_CONNECTIONS, default_value_t = 10)]
    pub database_max_connections: u32,

    /// Maximum number of concurrent inbound HTTP requests.
    #[arg(long, env = env_vars::MAX_CONCURRENT_REQUESTS, default_value_t = 128)]
    pub max_concurrent_requests: u32,

    /// Bind address for the standalone Prometheus metrics exporter.
    #[arg(long, env = env_vars::PROMETHEUS_BIND, default_value = "0.0.0.0:9090")]
    pub prometheus_bind: SocketAddr,

    /// Whether to run SQL migrations during startup.
    #[arg(long, env = env_vars::AUTO_MIGRATE, default_value_t = true)]
    pub auto_migrate: bool,

    /// Mounted cargo registry directory path.
    #[arg(long, env = env_vars::CARGO_REGISTRY_DIR, default_value = "/cargo/registry")]
    pub cargo_registry_dir: PathBuf,

    /// Local data directory path used by the server.
    #[arg(long, env = env_vars::MCP_DATA_DIR, default_value = "/var/lib/rust-mcp")]
    pub data_dir: PathBuf,

    /// Optional local path to a checked-out RustSec advisory-db repository.
    #[arg(long, env = env_vars::RUSTSEC_DB_DIR)]
    pub rustsec_db_dir: Option<PathBuf>,

    /// Optional local directory containing pre-generated rustdoc JSON files.
    #[arg(long, env = env_vars::RUSTDOC_JSON_DIR)]
    pub rustdoc_json_dir: Option<PathBuf>,

    /// Tracing filter string (RUST_LOG style).
    #[arg(long, env = env_vars::RUST_LOG, default_value = "info,rust_mcp=debug,sqlx=warn")]
    pub rust_log: String,

    /// Output format for logs.
    #[arg(long, env = env_vars::LOG_FORMAT, value_enum, default_value_t = LogFormat::Pretty)]
    pub log_format: LogFormat,
}

impl Config {
    /// Loads configuration from CLI flags and environment variables.
    pub fn load() -> Self {
        Self::parse()
    }
}

#[cfg(any(test, feature = "testing"))]
impl Config {
    /// Loads configuration from environment variables and built-in defaults,
    /// without reading process CLI arguments.
    pub fn load_from_env() -> Self {
        Self::parse_from(["rust-mcp"])
    }
}

/// MCP transport mode.
#[derive(Debug, Clone, Copy, Eq, PartialEq, ValueEnum)]
pub enum TransportMode {
    /// Serve MCP over streamable HTTP.
    Http,
    /// Use stdio transport.
    Stdio,
}

/// Log output formatting mode.
#[derive(Debug, Clone, Copy, Eq, PartialEq, ValueEnum)]
pub enum LogFormat {
    /// Human-readable compact logs.
    Pretty,
    /// Structured JSON logs.
    Json,
}
