//! Integration tests for the registry cache watcher (`poll_cache_changes`).
//!
//! Each test builds a temporary cargo registry cache directory tree, points the
//! `AppState` config at it, runs a single poll cycle, and verifies the expected
//! number of refresh jobs were enqueued.

use std::path::PathBuf;

use rust_mcp::mcp::indexing::cache_watcher::CacheIndex;
use rust_mcp::mcp::run_cache_watch_poll_for_tests;

use super::mock_index_sync_context;

// ──────────────────────────────────────────────────────────────────────────────
// Helpers
// ──────────────────────────────────────────────────────────────────────────────

/// RAII wrapper around a temporary registry root directory.
struct TempCacheRegistry {
    root: PathBuf,
}

impl TempCacheRegistry {
    fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos();
        let root = std::env::temp_dir().join(format!("rust-mcp-cw-inttest-{tag}-{nanos}"));
        Self { root }
    }

    /// Returns the `cache/` directory path and ensures it exists.
    fn cache_dir(&self) -> PathBuf {
        let dir = self.root.join("cache");
        std::fs::create_dir_all(&dir).expect("failed to create cache dir");
        dir
    }

    /// Creates a registry cache subdir (e.g. `cache/crates-io-test/`) and
    /// returns its path.
    fn add_cache_subdir(&self, name: &str) -> PathBuf {
        let dir = self.cache_dir().join(name);
        std::fs::create_dir_all(&dir).expect("failed to create cache subdir");
        dir
    }

    /// Writes a fake `.crate` file into a cache subdir.
    fn add_crate_file(&self, subdir: &str, crate_name: &str, version: &str) {
        let dir = self.cache_dir().join(subdir);
        std::fs::create_dir_all(&dir).expect("failed to create cache subdir");
        let filename = format!("{crate_name}-{version}.crate");
        std::fs::write(dir.join(filename), b"fake crate bytes")
            .expect("failed to write .crate file");
    }
}

impl Drop for TempCacheRegistry {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
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

/// Get crate names from pending refresh jobs, ordered by name.
async fn pending_job_crate_names(db: &sqlx::PgPool) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT crate_name FROM refresh_jobs
         WHERE scope = 'crate' AND status = 'pending'
         ORDER BY crate_name",
    )
    .fetch_all(db)
    .await
    .expect("failed to query refresh_jobs")
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

/// A new `.crate` file appearing after the initial index build should be
/// detected and a refresh job enqueued.
#[tokio::test]
async fn cache_watcher_detects_new_crate_file_and_enqueues_job() {
    let context = mock_index_sync_context()
        .await
        .expect("failed to build index sync context");

    let registry = TempCacheRegistry::new("detect");
    let subdir_name = "crates-io-test";

    // Pre-populate with one existing .crate file.
    registry.add_crate_file(subdir_name, "serde", "1.0.193");

    let mut state = context.state.clone();
    state
        .config
        .cargo_registry_dir = registry.root.clone();

    // Build the initial index — this learns about the pre-existing file.
    let cache_root = state
        .config
        .cargo_registry_dir
        .join("cache");
    let mut index: CacheIndex =
        rust_mcp::mcp::indexing::cache_watcher::build_initial_index(&cache_root)
            .expect("failed to build initial index");

    assert_eq!(index.len(), 1, "should have one subdir in index");
    assert_eq!(
        index
            .values()
            .next()
            .unwrap()
            .1
            .len(),
        1,
        "should have one known file"
    );

    // No jobs should exist yet.
    assert_eq!(count_pending_crate_jobs(&state.db).await, 0);

    // Now add a new .crate file (simulating `cargo add tokio`).
    registry.add_crate_file(subdir_name, "tokio", "1.28.0");

    // Run a poll cycle.
    let outcome = run_cache_watch_poll_for_tests(&state, &mut index).await;

    assert_eq!(outcome.new_files, 1, "should detect exactly one new file");
    assert_eq!(outcome.enqueued, 1, "should enqueue one refresh job");
    assert_eq!(outcome.unparseable, 0, "no files should be unparseable");

    // Verify the job landed in the database.
    let job_count = count_pending_crate_jobs(&state.db).await;
    assert_eq!(job_count, 1, "expected one pending job in the database");

    let names = pending_job_crate_names(&state.db).await;
    assert_eq!(names, vec!["tokio"]);
}

/// When the cache directory hasn't changed since the last poll, no jobs should
/// be enqueued and the cycle should be effectively a no-op.
#[tokio::test]
async fn cache_watcher_skips_unchanged_subdirs() {
    let context = mock_index_sync_context()
        .await
        .expect("failed to build index sync context");

    let registry = TempCacheRegistry::new("unchanged");
    registry.add_crate_file("crates-io-test", "serde", "1.0.193");

    let mut state = context.state.clone();
    state
        .config
        .cargo_registry_dir = registry.root.clone();

    let cache_root = state
        .config
        .cargo_registry_dir
        .join("cache");
    let mut index: CacheIndex =
        rust_mcp::mcp::indexing::cache_watcher::build_initial_index(&cache_root)
            .expect("failed to build initial index");

    // Poll with no filesystem changes.
    let outcome = run_cache_watch_poll_for_tests(&state, &mut index).await;

    assert_eq!(outcome.new_files, 0, "no new files should be detected");
    assert_eq!(outcome.enqueued, 0, "no jobs should be enqueued");
    assert_eq!(count_pending_crate_jobs(&state.db).await, 0);
}

/// Multiple new `.crate` files for different crates should each produce a
/// separate refresh job.
#[tokio::test]
async fn cache_watcher_enqueues_multiple_crates() {
    let context = mock_index_sync_context()
        .await
        .expect("failed to build index sync context");

    let registry = TempCacheRegistry::new("multi");
    let subdir_name = "crates-io-test";

    // Start with an empty cache subdir.
    registry.add_cache_subdir(subdir_name);

    let mut state = context.state.clone();
    state
        .config
        .cargo_registry_dir = registry.root.clone();

    let cache_root = state
        .config
        .cargo_registry_dir
        .join("cache");
    let mut index: CacheIndex =
        rust_mcp::mcp::indexing::cache_watcher::build_initial_index(&cache_root)
            .expect("failed to build initial index");

    // Add three crates at once.
    registry.add_crate_file(subdir_name, "alpha", "0.1.0");
    registry.add_crate_file(subdir_name, "beta", "2.0.0");
    registry.add_crate_file(subdir_name, "gamma", "3.0.0");

    let outcome = run_cache_watch_poll_for_tests(&state, &mut index).await;

    assert_eq!(outcome.new_files, 3, "should detect three new files");
    assert_eq!(outcome.enqueued, 3, "should enqueue three refresh jobs");

    let names = pending_job_crate_names(&state.db).await;
    assert_eq!(names, vec!["alpha", "beta", "gamma"]);
}

/// A newly-appearing cache subdir (new registry source) should be discovered
/// and its `.crate` files enqueued.
#[tokio::test]
async fn cache_watcher_discovers_new_subdir() {
    let context = mock_index_sync_context()
        .await
        .expect("failed to build index sync context");

    let registry = TempCacheRegistry::new("newsubdir");
    // Create the cache root with one empty subdir.
    registry.add_cache_subdir("crates-io-existing");

    let mut state = context.state.clone();
    state
        .config
        .cargo_registry_dir = registry.root.clone();

    let cache_root = state
        .config
        .cargo_registry_dir
        .join("cache");
    let mut index: CacheIndex =
        rust_mcp::mcp::indexing::cache_watcher::build_initial_index(&cache_root)
            .expect("failed to build initial index");

    assert_eq!(index.len(), 1, "should have one subdir initially");

    // A new registry subdir appears with a .crate file.
    registry.add_crate_file("crates-io-new-registry", "hyper", "1.0.0");

    let outcome = run_cache_watch_poll_for_tests(&state, &mut index).await;

    assert_eq!(outcome.new_files, 1, "should detect file in new subdir");
    assert_eq!(outcome.enqueued, 1, "should enqueue job for new crate");
    assert_eq!(index.len(), 2, "index should now track two subdirs");

    let names = pending_job_crate_names(&state.db).await;
    assert_eq!(names, vec!["hyper"]);
}
