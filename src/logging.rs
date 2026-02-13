use anyhow::Result;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

use crate::config::{Config, LogFormat};

pub fn init(config: &Config) -> Result<()> {
    let build_filter =
        || EnvFilter::try_new(config.rust_log.as_str()).unwrap_or_else(|_| EnvFilter::new("info"));

    match config.log_format {
        LogFormat::Pretty => tracing_subscriber::registry()
            .with(build_filter())
            .with(fmt::layer().compact())
            .try_init()?,
        LogFormat::Json => tracing_subscriber::registry()
            .with(build_filter())
            .with(fmt::layer().json())
            .try_init()?,
    }

    Ok(())
}
