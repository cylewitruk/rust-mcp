use std::future::Future;

use anyhow::{Context as _, Result};
use metrics_exporter_prometheus::PrometheusBuilder;
use tokio::net::TcpListener;
use tracing::{info, warn};

use crate::config::Config;
use crate::state::AppState;
use crate::{contracts, http, logging, mcp};

/// Runs the HTTP and MCP server lifecycle.
pub async fn run() -> Result<()> {
    let config = Config::load();
    logging::init(&config)?;

    if let Some(schema_export_dir) = config
        .schema_export_dir
        .as_deref()
    {
        let export =
            contracts::export_tool_schema_artifacts(schema_export_dir).with_context(|| {
                format!(
                    "failed to export tool schema artifacts into `{}`",
                    schema_export_dir.display()
                )
            })?;
        info!(
            output_dir = %schema_export_dir.display(),
            aggregate_file = %export.aggregate_file.display(),
            per_tool_files = export.per_tool_files.len(),
            "exported tool schema artifacts"
        );
    }

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

    let prometheus_handle = PrometheusBuilder::new()
        .install_recorder()
        .context("failed to install prometheus metrics recorder")?;

    mcp::metrics::register_metric_descriptions();

    let listener = TcpListener::bind(config.http_bind)
        .await
        .with_context(|| format!("failed to bind HTTP listener on {}", config.http_bind))?;

    info!(bind = %config.http_bind, "starting server");

    tokio::spawn(mcp::run_refresh_worker(state.clone()));
    tokio::spawn(mcp::run_enrichment_maintenance(state.clone()));
    tokio::spawn(mcp::run_registry_discovery(state.clone()));
    tokio::spawn(mcp::run_cache_watcher(state.clone()));

    let shutdown = make_shutdown_signal().context("failed to install shutdown signal handlers")?;

    let app = http::router(state, config, prometheus_handle);
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await
        .context("HTTP server exited unexpectedly")?;

    Ok(())
}

/// Installs OS signal handlers and returns a future that resolves on the first
/// shutdown signal received.
///
/// SIGTERM stream creation is fallible and is eagerly installed so that any
/// failure surfaces as a startup error rather than a panic inside the serve
/// loop. Ctrl+C is handled through an async future; errors from it are logged
/// as warnings rather than propagated, since the OS error can only be observed
/// after the future is polled.
fn make_shutdown_signal() -> std::io::Result<impl Future<Output = ()> + Send + 'static> {
    #[cfg(unix)]
    let mut sigterm = {
        use tokio::signal::unix::{SignalKind, signal};
        signal(SignalKind::terminate())?
    };

    Ok(async move {
        let ctrl_c = async {
            if let Err(error) = tokio::signal::ctrl_c().await {
                warn!(%error, "Ctrl+C signal error; Ctrl+C will not trigger graceful shutdown");
            }
        };

        #[cfg(unix)]
        let terminate = async move { sigterm.recv().await };

        #[cfg(not(unix))]
        let terminate = std::future::pending::<()>();

        tokio::select! {
            () = ctrl_c => {}
            _ = terminate => {}
        }

        info!("shutdown signal received");
    })
}
