//! Integration tests for proactive registry discovery (`run_registry_scan`).
//!
//! Each test builds a temporary cargo registry directory tree, points the
//! `AppState` config at it, runs a single scan, and then verifies the expected
//! number of `refresh_jobs` rows were created in the database.

use std::path::PathBuf;

use rust_mcp::mcp::run_registry_scan_for_tests;

use super::mock_index_sync_context;

// ──────────────────────────────────────────────────────────────────────────────
// Helpers
// ──────────────────────────────────────────────────────────────────────────────

/// RAII wrapper around a temporary registry root directory.
/// The directory is removed when the guard is dropped.
struct TempRegistry {
    root: PathBuf,
}

impl TempRegistry {
    fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos();
        let root = std::env::temp_dir().join(format!("rust-mcp-disc-inttest-{tag}-{nanos}"));
        Self { root }
    }

    /// Creates `{root}/src/crates-io-test/{name}-{version}/`.
    fn add_crate(&self, name: &str, version: &str) {
        let dir = self
            .root
            .join("src")
            .join("crates-io-test")
            .join(format!("{name}-{version}"));
        std::fs::create_dir_all(dir).expect("failed to create crate dir in temp registry");
    }
}

impl Drop for TempRegistry {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Seed a bare crate name into the `crates` table so the scan treats it as
/// already known.
async fn seed_known_crate(db: &sqlx::PgPool, name: &str) {
    sqlx::query("INSERT INTO crates (name) VALUES ($1) ON CONFLICT (name) DO NOTHING")
        .bind(name)
        .execute(db)
        .await
        .expect("failed to seed known crate into crates table");
}

/// Count pending `crate`-scope `refresh_jobs` rows.
async fn count_pending_crate_jobs(db: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*)::BIGINT
         FROM refresh_jobs
         WHERE scope = 'crate'
           AND status = 'pending'",
    )
    .fetch_one(db)
    .await
    .expect("failed to count pending crate refresh_jobs")
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

/// A registry containing two unknown crates should produce two pending
/// `crate`-scope refresh jobs.
#[tokio::test]
async fn registry_scan_enqueues_jobs_for_unindexed_crates() {
    let context = mock_index_sync_context()
        .await
        .expect("failed to build index sync context");

    let registry = TempRegistry::new("basic");
    registry.add_crate("alpha", "0.1.0");
    registry.add_crate("beta", "2.0.0");

    let mut state = context.state.clone();
    state
        .config
        .cargo_registry_dir = registry.root.clone();

    run_registry_scan_for_tests(&state).await;

    let job_count = count_pending_crate_jobs(&state.db).await;
    assert_eq!(job_count, 2, "expected one pending job per unindexed crate");
}

/// A crate already present in the `crates` table should be skipped; only the
/// truly unknown crate should be enqueued.
#[tokio::test]
async fn registry_scan_skips_already_indexed_crates() {
    let context = mock_index_sync_context()
        .await
        .expect("failed to build index sync context");

    let registry = TempRegistry::new("skip-known");
    registry.add_crate("alpha", "0.1.0");
    registry.add_crate("beta", "2.0.0");

    // Mark 'alpha' as already known so the scan should skip it.
    seed_known_crate(&context.state.db, "alpha").await;

    let mut state = context.state.clone();
    state
        .config
        .cargo_registry_dir = registry.root.clone();

    run_registry_scan_for_tests(&state).await;

    let job_count = count_pending_crate_jobs(&state.db).await;
    assert_eq!(job_count, 1, "only the unknown crate ('beta') should be enqueued");
}

/// When `registry_scan_batch_limit` is set to 1, at most one job should be
/// enqueued even if multiple unknown crates are discovered.
#[tokio::test]
async fn registry_scan_respects_batch_limit() {
    let context = mock_index_sync_context()
        .await
        .expect("failed to build index sync context");

    let registry = TempRegistry::new("batch");
    registry.add_crate("alpha", "0.1.0");
    registry.add_crate("beta", "2.0.0");
    registry.add_crate("gamma", "3.0.0");

    let mut state = context.state.clone();
    state
        .config
        .cargo_registry_dir = registry.root.clone();
    state
        .config
        .registry_scan_batch_limit = 1;

    run_registry_scan_for_tests(&state).await;

    let job_count = count_pending_crate_jobs(&state.db).await;
    assert_eq!(job_count, 1, "registry_scan_batch_limit=1 should cap enqueued jobs at 1");
}
