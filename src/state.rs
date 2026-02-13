use std::time::Duration;

use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

use crate::config::Config;

/// Shared application state used by HTTP and MCP handlers.
#[derive(Debug, Clone)]
pub struct AppState {
    /// Loaded runtime configuration.
    pub config: Config,
    /// PostgreSQL connection pool.
    pub db: PgPool,
    /// Shared outbound HTTP client.
    pub http: reqwest::Client,
}

impl AppState {
    /// Creates a new shared application state, connecting database and HTTP
    /// client.
    pub async fn connect(config: Config) -> anyhow::Result<Self> {
        let db = PgPoolOptions::new()
            .min_connections(config.database_min_connections)
            .max_connections(config.database_max_connections)
            .acquire_timeout(Duration::from_secs(10))
            .connect(&config.database_url)
            .await?;

        let http = reqwest::Client::builder()
            .user_agent(
                config
                    .crates_io_user_agent
                    .clone(),
            )
            .timeout(Duration::from_secs(config.crates_io_timeout_secs))
            .build()?;

        Ok(Self { config, db, http })
    }

    /// Applies all embedded SQL migrations.
    pub async fn run_migrations(&self) -> anyhow::Result<()> {
        sqlx::migrate!("./migrations")
            .run(&self.db)
            .await?;
        Ok(())
    }

    /// Executes a lightweight readiness check against the database.
    pub async fn readiness_check(&self) -> Result<(), sqlx::Error> {
        sqlx::query_scalar::<_, i64>("SELECT 1::BIGINT")
            .fetch_one(&self.db)
            .await?;
        Ok(())
    }
}
