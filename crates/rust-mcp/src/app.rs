use anyhow::{Context as _, Result};
use metrics_exporter_prometheus::PrometheusBuilder;
use tokio::net::TcpListener;
use tracing::{info, warn};

use crate::config::Config;
use crate::state::AppState;
use crate::{http, logging, mcp};

/// Runs the HTTP and MCP server lifecycle.
pub async fn run() -> Result<()> {
    let config = Config::load();
    logging::init(&config)?;

    let state = AppState::connect(config.clone())
        .await
        .context("failed to initialize shared application state")?;

    if config.auto_migrate {
        state
            .run_migrations()
            .await
            .context("failed to apply database migrations")?;
    } else {
        warn!("AUTO_MIGRATE=false, skipping migrations");
    }

    PrometheusBuilder::new()
        .with_http_listener(config.prometheus_bind)
        .install()
        .context("failed to install prometheus metrics exporter")?;

    info!(bind = %config.prometheus_bind, "prometheus exporter listening");

    let listener = TcpListener::bind(config.http_bind)
        .await
        .with_context(|| format!("failed to bind HTTP listener on {}", config.http_bind))?;

    info!(bind = %config.http_bind, transport = ?config.mcp_transport, "starting server");

    tokio::spawn(mcp::run_refresh_worker(state.clone()));
    tokio::spawn(mcp::run_startup_rustdoc_json_refresh(state.clone()));

    let app = http::router(state, config);
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("HTTP server exited unexpectedly")?;

    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{SignalKind, signal};

        let mut stream =
            signal(SignalKind::terminate()).expect("failed to install SIGTERM handler");
        stream.recv().await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }

    info!("shutdown signal received");
}
