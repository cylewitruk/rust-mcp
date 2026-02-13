use std::net::SocketAddr;
use std::path::PathBuf;

use clap::{Parser, ValueEnum};

/// Runtime configuration for the server process.
#[derive(Debug, Clone, Parser)]
#[command(
    name = "rust-mcp",
    version,
    about = "Local-first Rust dependency intelligence MCP server"
)]
pub struct Config {
    /// MCP transport mode to use for serving clients.
    #[arg(long, env = "MCP_TRANSPORT", value_enum, default_value_t = TransportMode::Http)]
    pub mcp_transport: TransportMode,

    /// HTTP bind address for health/readiness and streamable MCP endpoints.
    #[arg(long, env = "MCP_HTTP_BIND", default_value = "127.0.0.1:43173")]
    pub http_bind: SocketAddr,

    /// SSE keep-alive interval in seconds for streamable MCP responses.
    #[arg(long, env = "MCP_SSE_KEEP_ALIVE_SECS", default_value_t = 15)]
    pub mcp_sse_keep_alive_secs: u64,

    /// SSE retry delay in milliseconds for reconnecting clients.
    #[arg(long, env = "MCP_SSE_RETRY_MS", default_value_t = 3000)]
    pub mcp_sse_retry_ms: u64,

    /// PostgreSQL connection string.
    #[arg(
        long,
        env = "DATABASE_URL",
        default_value = "postgres://postgres:postgres@postgres:5432/rust_mcp"
    )]
    pub database_url: String,

    /// Base URL for crates.io API calls.
    #[arg(long, env = "CRATES_IO_BASE_URL", default_value = "https://crates.io")]
    pub crates_io_base_url: String,

    /// User-Agent sent to crates.io and other remote APIs.
    #[arg(
        long,
        env = "CRATES_IO_USER_AGENT",
        default_value = "rust-mcp/0.1.0 (local dev machine)"
    )]
    pub crates_io_user_agent: String,

    /// HTTP timeout for crates.io/OSV requests.
    #[arg(long, env = "CRATES_IO_TIMEOUT_SECS", default_value_t = 20)]
    pub crates_io_timeout_secs: u64,

    /// Minimum database connection pool size.
    #[arg(long, env = "DATABASE_MIN_CONNECTIONS", default_value_t = 1)]
    pub database_min_connections: u32,

    /// Maximum database connection pool size.
    #[arg(long, env = "DATABASE_MAX_CONNECTIONS", default_value_t = 10)]
    pub database_max_connections: u32,

    /// Whether to run SQL migrations during startup.
    #[arg(long, env = "AUTO_MIGRATE", default_value_t = true)]
    pub auto_migrate: bool,

    /// Mounted cargo registry directory path.
    #[arg(long, env = "CARGO_REGISTRY_DIR", default_value = "/cargo/registry")]
    pub cargo_registry_dir: PathBuf,

    /// Local data directory path used by the server.
    #[arg(long, env = "MCP_DATA_DIR", default_value = "/var/lib/rust-mcp")]
    pub data_dir: PathBuf,

    /// Tracing filter string (RUST_LOG style).
    #[arg(long, env = "RUST_LOG", default_value = "info,rust_mcp=debug,sqlx=warn")]
    pub rust_log: String,

    /// Output format for logs.
    #[arg(long, env = "LOG_FORMAT", value_enum, default_value_t = LogFormat::Pretty)]
    pub log_format: LogFormat,
}

impl Config {
    /// Loads configuration from CLI flags and environment variables.
    pub fn load() -> Self {
        Self::parse()
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
