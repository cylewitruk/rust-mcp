use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use tokio::sync::Mutex;
use tokio::time::sleep;

use crate::config::Config;
use crate::db::tools;
use crate::mcp::indexing::coordinator::IndexingCoordinator;

/// Builds a User-Agent string following SEP-1329 conventions.
///
/// Format: `rust-mcp/<version> os/<os>[#<release>] lang/rust`
///
/// Examples:
/// - `rust-mcp/0.1.0 os/linux#6.1.0 lang/rust`
/// - `rust-mcp/0.1.0 os/darwin#24.5.0 lang/rust`
fn user_agent() -> String {
    let os = std::env::consts::OS;
    let version = env!("CARGO_PKG_VERSION");

    match os_release() {
        Some(release) => format!("rust-mcp/{version} os/{os}#{release} lang/rust"),
        None => format!("rust-mcp/{version} os/{os} lang/rust"),
    }
}

/// Returns the OS kernel release string via `uname -r`, or `None` if
/// unavailable.
fn os_release() -> Option<String> {
    std::process::Command::new("uname")
        .arg("-r")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
}

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
    /// Bridges tool handlers waiting for on-demand indexing with the
    /// background refresh worker.
    pub indexing_coordinator: Arc<IndexingCoordinator>,
}

/// Identifies a remote data source for outbound rate limiting.
#[derive(Debug, Clone, Copy)]
pub enum OutboundSource {
    /// crates.io metadata and readme endpoints.
    CratesIo,
    /// docs.rs HTML page fetches.
    DocsRs,
    /// OSV vulnerability API.
    Osv,
    /// GitHub REST API (repo metadata, GHSA advisories).
    GitHub,
}

/// Process-wide outbound request limiters for each remote data source.
#[derive(Debug)]
pub struct OutboundRateLimiters {
    crates_io: OutboundRateLimiter,
    docs_rs: OutboundRateLimiter,
    osv: OutboundRateLimiter,
    github: OutboundRateLimiter,
}

/// Two-tier outbound rate limiter.
///
/// **Tier 1 (burst)**: enforces a minimum interval between consecutive
/// requests — e.g. "at most 1 request per second".
///
/// **Tier 2 (window)**: enforces a maximum number of requests within a
/// rolling time window — e.g. "at most 100 requests per 3 minutes". This
/// prevents sustained high throughput from overwhelming upstream services
/// even when the burst interval is satisfied.
///
/// Both tiers must pass before a request is allowed through.
#[derive(Debug)]
pub struct OutboundRateLimiter {
    /// Minimum gap between consecutive requests.
    burst_interval: Duration,
    /// Earliest `Instant` at which the next request is allowed (burst gate).
    next_allowed_at: Mutex<Instant>,
    /// Maximum number of requests allowed within `window_duration`.
    window_max: u32,
    /// Duration of the rolling window.
    window_duration: Duration,
    /// Timestamps of recent requests within the window.
    window_timestamps: Mutex<VecDeque<Instant>>,
}

impl OutboundRateLimiter {
    /// Creates a new two-tier rate limiter.
    ///
    /// If `window_max` is 0 the window gate is effectively disabled (burst
    /// only). If `burst_interval` is zero the burst gate is effectively
    /// disabled (window only).
    pub fn new(burst_interval: Duration, window_max: u32, window_duration: Duration) -> Self {
        Self {
            burst_interval,
            next_allowed_at: Mutex::new(Instant::now()),
            window_max,
            window_duration,
            window_timestamps: Mutex::new(VecDeque::new()),
        }
    }

    /// Waits until both rate-limit tiers allow a request, then records
    /// the request timestamp.
    pub async fn acquire(&self) {
        // Tier 1: burst gate.
        {
            let mut next = self
                .next_allowed_at
                .lock()
                .await;
            let now = Instant::now();
            if *next > now {
                sleep(*next - now).await;
            }
            *next = Instant::now() + self.burst_interval;
        }

        // Tier 2: window gate.
        if self.window_max > 0 {
            let mut timestamps = self
                .window_timestamps
                .lock()
                .await;
            let now = Instant::now();
            let cutoff = now
                .checked_sub(self.window_duration)
                .unwrap_or(now);

            // Evict entries outside the window.
            while timestamps
                .front()
                .is_some_and(|&ts| ts < cutoff)
            {
                timestamps.pop_front();
            }

            // If the window is full, wait until the oldest entry expires.
            if timestamps.len() >= self.window_max as usize
                && let Some(&oldest) = timestamps.front()
            {
                let wait_until = oldest + self.window_duration;
                let now = Instant::now();
                if wait_until > now {
                    // Release the lock while sleeping so other tasks
                    // aren't blocked unnecessarily.
                    drop(timestamps);
                    sleep(wait_until - now).await;
                    timestamps = self
                        .window_timestamps
                        .lock()
                        .await;
                    // Re-evict after waking — time has passed.
                    let now = Instant::now();
                    let cutoff = now
                        .checked_sub(self.window_duration)
                        .unwrap_or(now);
                    while timestamps
                        .front()
                        .is_some_and(|&ts| ts < cutoff)
                    {
                        timestamps.pop_front();
                    }
                }
            }

            timestamps.push_back(Instant::now());
        }
    }
}

impl OutboundRateLimiters {
    /// Builds per-source outbound limiters from runtime configuration.
    pub fn new(config: &Config) -> Self {
        Self {
            crates_io: OutboundRateLimiter::new(
                Duration::from_millis(config.crates_io_min_interval_ms),
                config.crates_io_window_max_requests,
                Duration::from_secs(config.crates_io_window_duration_secs),
            ),
            docs_rs: OutboundRateLimiter::new(
                Duration::from_millis(config.docs_rs_min_interval_ms),
                config.docs_rs_window_max_requests,
                Duration::from_secs(config.docs_rs_window_duration_secs),
            ),
            osv: OutboundRateLimiter::new(
                Duration::from_millis(config.osv_min_interval_ms),
                config.osv_window_max_requests,
                Duration::from_secs(config.osv_window_duration_secs),
            ),
            github: OutboundRateLimiter::new(
                Duration::from_millis(config.github_min_interval_ms),
                config.github_window_max_requests,
                Duration::from_secs(config.github_window_duration_secs),
            ),
        }
    }

    async fn acquire(&self, source: OutboundSource) {
        match source {
            OutboundSource::CratesIo => self.crates_io.acquire().await,
            OutboundSource::DocsRs => self.docs_rs.acquire().await,
            OutboundSource::Osv => self.osv.acquire().await,
            OutboundSource::GitHub => self.github.acquire().await,
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
            .user_agent(user_agent())
            .timeout(Duration::from_secs(config.crates_io_timeout_secs))
            .build()?;

        let outbound_rate_limiters = Arc::new(OutboundRateLimiters::new(&config));
        let indexing_coordinator = Arc::new(IndexingCoordinator::new());

        Ok(Self {
            config,
            db,
            http,
            outbound_rate_limiters,
            indexing_coordinator,
        })
    }

    /// Waits until the per-source rate limit allows the next outbound request.
    pub async fn acquire_outbound_slot(&self, source: OutboundSource) {
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
        tools::run_readiness_probe(&self.db).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn burst_only_limiter_allows_immediate_first_request() {
        let limiter = OutboundRateLimiter::new(
            Duration::from_millis(50),
            0, // window disabled
            Duration::from_secs(1),
        );
        let start = Instant::now();
        limiter.acquire().await;
        // First acquire should be nearly instant.
        assert!(start.elapsed() < Duration::from_millis(30));
    }

    #[tokio::test]
    async fn burst_limiter_enforces_minimum_interval() {
        let limiter = OutboundRateLimiter::new(
            Duration::from_millis(50),
            0, // window disabled
            Duration::from_secs(1),
        );
        limiter.acquire().await;
        let between = Instant::now();
        limiter.acquire().await;
        // Second acquire should wait at least 50ms.
        assert!(between.elapsed() >= Duration::from_millis(45));
    }

    #[tokio::test]
    async fn window_limiter_blocks_after_max_requests() {
        // Window: max 3 requests per 200ms, no burst interval.
        let limiter = OutboundRateLimiter::new(Duration::ZERO, 3, Duration::from_millis(200));

        // First 3 should be fast.
        for _ in 0..3 {
            limiter.acquire().await;
        }

        // 4th should block until the window slides.
        let before = Instant::now();
        limiter.acquire().await;
        let waited = before.elapsed();
        // Should have waited ~200ms for the oldest timestamp to expire.
        assert!(waited >= Duration::from_millis(150), "expected ~200ms wait, got {:?}", waited);
    }

    #[tokio::test]
    async fn window_disabled_when_max_is_zero() {
        let limiter = OutboundRateLimiter::new(
            Duration::from_millis(1),
            0, // disabled
            Duration::from_secs(60),
        );
        // Should complete quickly — window gate is not enforced.
        let start = Instant::now();
        for _ in 0..10 {
            limiter.acquire().await;
        }
        // With 1ms burst interval and 10 requests, total should be < 100ms.
        assert!(start.elapsed() < Duration::from_millis(100));
    }

    #[tokio::test]
    async fn both_tiers_combine() {
        // Burst: 30ms, Window: max 2 per 200ms.
        let limiter =
            OutboundRateLimiter::new(Duration::from_millis(30), 2, Duration::from_millis(200));

        let start = Instant::now();
        // First 2 requests: limited by burst (30ms each).
        limiter.acquire().await;
        limiter.acquire().await;
        // 3rd request: window is full (2/2), must wait ~200ms from first.
        limiter.acquire().await;
        let total = start.elapsed();
        // Should be at least 200ms (window gate) not just 60ms (burst only).
        assert!(total >= Duration::from_millis(180), "expected >= 200ms total, got {:?}", total);
    }
}
