use std::{net::SocketAddr, path::PathBuf};

use clap::{Parser, ValueEnum};

#[derive(Debug, Clone, Parser)]
#[command(
    name = "rust-mcp",
    version,
    about = "Local-first Rust dependency intelligence MCP server"
)]
pub struct Config {
    #[arg(long, env = "MCP_TRANSPORT", value_enum, default_value_t = TransportMode::Http)]
    pub mcp_transport: TransportMode,

    #[arg(long, env = "MCP_HTTP_BIND", default_value = "127.0.0.1:43173")]
    pub http_bind: SocketAddr,

    #[arg(long, env = "MCP_SSE_KEEP_ALIVE_SECS", default_value_t = 15)]
    pub mcp_sse_keep_alive_secs: u64,

    #[arg(long, env = "MCP_SSE_RETRY_MS", default_value_t = 3000)]
    pub mcp_sse_retry_ms: u64,

    #[arg(
        long,
        env = "DATABASE_URL",
        default_value = "postgres://postgres:postgres@postgres:5432/rust_mcp"
    )]
    pub database_url: String,

    #[arg(long, env = "CRATES_IO_BASE_URL", default_value = "https://crates.io")]
    pub crates_io_base_url: String,

    #[arg(
        long,
        env = "CRATES_IO_USER_AGENT",
        default_value = "rust-mcp/0.1.0 (local dev machine)"
    )]
    pub crates_io_user_agent: String,

    #[arg(long, env = "CRATES_IO_TIMEOUT_SECS", default_value_t = 20)]
    pub crates_io_timeout_secs: u64,

    #[arg(long, env = "DATABASE_MIN_CONNECTIONS", default_value_t = 1)]
    pub database_min_connections: u32,

    #[arg(long, env = "DATABASE_MAX_CONNECTIONS", default_value_t = 10)]
    pub database_max_connections: u32,

    #[arg(long, env = "AUTO_MIGRATE", default_value_t = true)]
    pub auto_migrate: bool,

    #[arg(long, env = "CARGO_REGISTRY_DIR", default_value = "/cargo/registry")]
    pub cargo_registry_dir: PathBuf,

    #[arg(long, env = "MCP_DATA_DIR", default_value = "/var/lib/rust-mcp")]
    pub data_dir: PathBuf,

    #[arg(
        long,
        env = "RUST_LOG",
        default_value = "info,rust_mcp=debug,sqlx=warn"
    )]
    pub rust_log: String,

    #[arg(long, env = "LOG_FORMAT", value_enum, default_value_t = LogFormat::Pretty)]
    pub log_format: LogFormat,
}

impl Config {
    pub fn load() -> Self {
        Self::parse()
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, ValueEnum)]
pub enum TransportMode {
    Http,
    Stdio,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, ValueEnum)]
pub enum LogFormat {
    Pretty,
    Json,
}
