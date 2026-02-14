use std::sync::Arc;
use std::time::{Duration, Instant};

use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use tokio::sync::Mutex;
use tokio::time::sleep;

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
    /// Shared outbound rate limiters by remote source.
    pub outbound_rate_limiters: Arc<OutboundRateLimiters>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum OutboundSource {
    /// crates.io metadata and readme endpoints.
    CratesIo,
    /// docs.rs HTML page fetches.
    DocsRs,
    /// OSV vulnerability API.
    Osv,
}

/// Process-wide outbound request limiters for each remote data source.
#[derive(Debug)]
pub struct OutboundRateLimiters {
    crates_io: OutboundRateLimiter,
    docs_rs: OutboundRateLimiter,
    osv: OutboundRateLimiter,
}

#[derive(Debug)]
struct OutboundRateLimiter {
    min_interval: Duration,
    next_allowed_at: Mutex<Instant>,
}

impl OutboundRateLimiter {
    fn new(min_interval: Duration) -> Self {
        Self {
            min_interval,
            next_allowed_at: Mutex::new(Instant::now()),
        }
    }

    async fn acquire(&self) {
        let mut next_allowed_at = self
            .next_allowed_at
            .lock()
            .await;
        let now = Instant::now();
        if *next_allowed_at > now {
            sleep(*next_allowed_at - now).await;
        }
        *next_allowed_at = Instant::now() + self.min_interval;
    }
}

impl OutboundRateLimiters {
    /// Builds per-source outbound limiters from runtime configuration.
    pub(crate) fn new(config: &Config) -> Self {
        Self {
            crates_io: OutboundRateLimiter::new(Duration::from_millis(
                config.crates_io_min_interval_ms,
            )),
            docs_rs: OutboundRateLimiter::new(Duration::from_millis(
                config.docs_rs_min_interval_ms,
            )),
            osv: OutboundRateLimiter::new(Duration::from_millis(config.osv_min_interval_ms)),
        }
    }

    async fn acquire(&self, source: OutboundSource) {
        match source {
            OutboundSource::CratesIo => self.crates_io.acquire().await,
            OutboundSource::DocsRs => self.docs_rs.acquire().await,
            OutboundSource::Osv => self.osv.acquire().await,
        }
    }
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

        let outbound_rate_limiters = Arc::new(OutboundRateLimiters::new(&config));

        Ok(Self {
            config,
            db,
            http,
            outbound_rate_limiters,
        })
    }

    pub(crate) async fn acquire_outbound_slot(&self, source: OutboundSource) {
        self.outbound_rate_limiters
            .acquire(source)
            .await;
    }

    /// Applies all embedded SQL migrations.
    pub async fn run_migrations(&self) -> anyhow::Result<()> {
        sqlx::migrate!("../../migrations")
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
