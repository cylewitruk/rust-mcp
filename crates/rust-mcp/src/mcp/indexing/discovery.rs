//! Proactive registry discovery and background indexing.
//!
//! At startup, [`run_registry_discovery`] walks the mounted cargo registry
//! (`$CARGO_REGISTRY_DIR/src/`) and enqueues crate-scope refresh jobs for
//! every crate directory that is not yet present in the database. A
//! configurable pre-warm list is processed first so high-priority crates are
//! indexed before the general scan.
//!
//! After the startup scan the function optionally enters a periodic loop
//! (controlled by `REGISTRY_SCAN_INTERVAL_SECS`; 0 disables it) to pick up
//! newly-added registry entries over time.

use metrics::{counter, histogram};
use serde_json::json;
use tokio::time::{Duration, sleep};
use tracing::{info, warn};

use crate::db::indexing::{enqueue_or_get_refresh_job_id, fetch_known_crate_names};
use crate::mcp::indexing::coordinator::{PRIORITY_DISCOVERY, PRIORITY_PRE_WARM};
use crate::state::AppState;

/// Summary of a single registry discovery scan run.
#[derive(Debug, Default)]
pub(crate) struct DiscoveryScanOutcome {
    /// Total `{name}-{version}` directories found in the registry.
    pub discovered: usize,
    /// New crate-scope jobs enqueued this run.
    pub enqueued: usize,
    /// Crates already present in the database (skipped).
    pub already_known: usize,
    /// Directory names that could not be parsed as `{name}-{version}`.
    pub unparseable: usize,
}

// ──────────────────────────────────────────────────────────────────────────────
// Entry points
// ──────────────────────────────────────────────────────────────────────────────

/// Long-running background task.
///
/// Runs a startup scan immediately, then sleeps for
/// `config.registry_scan_interval_secs` between subsequent scans. If the
/// interval is 0 the function exits after the startup scan.
pub(crate) async fn run_registry_discovery(state: AppState) {
    let outcome = run_registry_scan(&state).await;
    info!(
        discovered = outcome.discovered,
        enqueued = outcome.enqueued,
        already_known = outcome.already_known,
        unparseable = outcome.unparseable,
        "startup registry discovery scan complete"
    );

    let interval_secs = state
        .config
        .registry_scan_interval_secs;
    if interval_secs == 0 {
        info!("REGISTRY_SCAN_INTERVAL_SECS=0; periodic registry discovery disabled");
        return;
    }

    loop {
        sleep(Duration::from_secs(interval_secs)).await;
        let outcome = run_registry_scan(&state).await;
        info!(
            discovered = outcome.discovered,
            enqueued = outcome.enqueued,
            already_known = outcome.already_known,
            unparseable = outcome.unparseable,
            "periodic registry discovery scan complete"
        );
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Core scan logic
// ──────────────────────────────────────────────────────────────────────────────

/// Perform one full scan of the cargo registry and enqueue crate jobs for
/// any crate names not yet present in the database.
pub(crate) async fn run_registry_scan(state: &AppState) -> DiscoveryScanOutcome {
    let scan_started = std::time::Instant::now();
    let mut outcome = DiscoveryScanOutcome::default();

    // ── 1. Walk the registry ──────────────────────────────────────────────────
    let src_root = state
        .config
        .cargo_registry_dir
        .join("src");
    let discovered_names = match collect_crate_names_from_registry(&src_root, &mut outcome) {
        Ok(names) => names,
        Err(error) => {
            warn!(%error, src_root = %src_root.display(), "registry discovery scan skipped");
            counter!("rust_mcp_discovery_scan_errors_total").increment(1);
            return outcome;
        }
    };

    // ── 2. Load known crate names from DB ────────────────────────────────────
    let known_names = match fetch_known_crate_names(&state.db).await {
        Ok(names) => names,
        Err(error) => {
            warn!(%error, "registry discovery scan: failed to fetch known crate names");
            counter!("rust_mcp_discovery_scan_errors_total").increment(1);
            return outcome;
        }
    };

    // ── 3. Enqueue pre-warm crates first ─────────────────────────────────────
    let pre_warm: Vec<String> = state
        .config
        .pre_warm_crates
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_lowercase)
        .collect();

    for crate_name in &pre_warm {
        if let Err(error) = enqueue_or_get_refresh_job_id(
            &state.db,
            crate_name,
            "crate",
            PRIORITY_PRE_WARM,
            false,
            json!({"trigger": "registry_discovery", "pre_warm": true}),
        )
        .await
        {
            warn!(%error, %crate_name, "failed to enqueue pre-warm crate job");
        } else {
            outcome.enqueued += 1;
        }
    }

    // ── 4. Enqueue unknown crates from the general scan ───────────────────────
    let batch_limit = state
        .config
        .registry_scan_batch_limit as usize;
    let unknown: Vec<&String> = discovered_names
        .iter()
        .filter(|name| !known_names.contains(*name))
        .collect();

    outcome.already_known = outcome
        .discovered
        .saturating_sub(unknown.len() + outcome.unparseable);

    let to_enqueue =
        if batch_limit > 0 { &unknown[..unknown.len().min(batch_limit)] } else { &unknown[..] };

    for crate_name in to_enqueue {
        if let Err(error) = enqueue_or_get_refresh_job_id(
            &state.db,
            crate_name,
            "crate",
            PRIORITY_DISCOVERY,
            false,
            json!({"trigger": "registry_discovery"}),
        )
        .await
        {
            warn!(%error, %crate_name, "failed to enqueue discovery crate job");
        } else {
            outcome.enqueued += 1;
        }
    }

    // ── 5. Wake the worker if we enqueued anything ───────────────────────────
    if outcome.enqueued > 0 {
        state
            .indexing_coordinator
            .notify_worker();
    }

    // ── 6. Emit Prometheus metrics ───────────────────────────────────────────
    let elapsed_ms = scan_started
        .elapsed()
        .as_millis() as f64;
    histogram!("rust_mcp_discovery_scan_duration_ms").record(elapsed_ms);
    counter!("rust_mcp_discovery_scans_total").increment(1);
    counter!("rust_mcp_discovery_jobs_enqueued_total").increment(outcome.enqueued as u64);

    outcome
}

// ──────────────────────────────────────────────────────────────────────────────
// Helpers
// ──────────────────────────────────────────────────────────────────────────────

/// Walk `src_root/{registry}/{name}-{version}/` and collect the unique set of
/// crate names. Updates `outcome.discovered` and `outcome.unparseable`.
fn collect_crate_names_from_registry(
    src_root: &std::path::Path,
    outcome: &mut DiscoveryScanOutcome,
) -> Result<Vec<String>, String> {
    let mut names: std::collections::HashSet<String> = std::collections::HashSet::new();

    let registries = std::fs::read_dir(src_root)
        .map_err(|e| format!("cannot read registry src dir `{}`: {e}", src_root.display()))?;

    for registry_entry in registries {
        let registry_path = match registry_entry {
            Ok(e) => e.path(),
            Err(error) => {
                warn!(%error, "failed to read registry dir entry; skipping");
                continue;
            }
        };

        if !registry_path.is_dir() {
            continue;
        }

        let crate_dirs = match std::fs::read_dir(&registry_path) {
            Ok(d) => d,
            Err(error) => {
                warn!(%error, path = %registry_path.display(), "failed to read registry sub-dir; skipping");
                continue;
            }
        };

        for crate_entry in crate_dirs {
            let crate_path = match crate_entry {
                Ok(e) => e.path(),
                Err(error) => {
                    warn!(%error, "failed to read crate dir entry; skipping");
                    continue;
                }
            };

            if !crate_path.is_dir() {
                continue;
            }

            let dir_name = crate_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");

            outcome.discovered += 1;

            match parse_registry_dir_name(dir_name) {
                Some((crate_name, _version)) => {
                    names.insert(crate_name);
                }
                None => {
                    outcome.unparseable += 1;
                }
            }
        }
    }

    Ok(names.into_iter().collect())
}

/// Parse a cargo registry directory name of the form `{name}-{semver}` into
/// its component parts. Iterates split points from right to correctly handle
/// crate names containing hyphens (e.g. `serde-derive-1.0.0`).
fn parse_registry_dir_name(dir_name: &str) -> Option<(String, String)> {
    let bytes = dir_name.as_bytes();
    for i in (1..dir_name.len()).rev() {
        if bytes[i - 1] == b'-' {
            let potential_version = &dir_name[i..];
            if semver::Version::parse(potential_version).is_ok() {
                return Some((dir_name[..i - 1].to_string(), potential_version.to_string()));
            }
        }
    }
    None
}

// ──────────────────────────────────────────────────────────────────────────────
// Unit tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::{DiscoveryScanOutcome, collect_crate_names_from_registry, parse_registry_dir_name};

    // ── parse_registry_dir_name ───────────────────────────────────────────────

    #[test]
    fn parses_simple_crate() {
        let (name, version) = parse_registry_dir_name("serde-1.0.193").unwrap();
        assert_eq!(name, "serde");
        assert_eq!(version, "1.0.193");
    }

    #[test]
    fn parses_hyphenated_crate() {
        let (name, version) = parse_registry_dir_name("serde-derive-1.0.193").unwrap();
        assert_eq!(name, "serde-derive");
        assert_eq!(version, "1.0.193");
    }

    #[test]
    fn parses_multi_hyphen_crate() {
        let (name, version) = parse_registry_dir_name("tokio-util-0.7.10").unwrap();
        assert_eq!(name, "tokio-util");
        assert_eq!(version, "0.7.10");
    }

    #[test]
    fn parses_pre_release_version() {
        let (name, version) = parse_registry_dir_name("foo-bar-1.0.0-alpha.1").unwrap();
        assert_eq!(name, "foo-bar");
        assert_eq!(version, "1.0.0-alpha.1");
    }

    #[test]
    fn returns_none_for_no_version() {
        assert!(parse_registry_dir_name("just-a-name").is_none());
        assert!(parse_registry_dir_name("noversion").is_none());
    }

    // ── collect_crate_names_from_registry ────────────────────────────────────

    /// Build a deterministic temp path for a test, returning a cleanup guard.
    fn temp_registry_root(tag: &str) -> (std::path::PathBuf, TempCleanup) {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos();
        let root = std::env::temp_dir().join(format!("rust-mcp-disc-{tag}-{nanos}"));
        (root.clone(), TempCleanup(root))
    }

    struct TempCleanup(std::path::PathBuf);
    impl Drop for TempCleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn collect_names_discovers_dirs_and_counts_unparseable() {
        let (root, _cleanup) = temp_registry_root("basic");
        // src_root/crates-io-test/{name}-{version}/
        let registry = root.join("crates-io-test");
        std::fs::create_dir_all(registry.join("serde-1.0.193")).unwrap();
        std::fs::create_dir_all(registry.join("tokio-util-0.7.10")).unwrap();
        std::fs::create_dir_all(registry.join("not-a-valid-version")).unwrap();
        // A file (not a dir) — should be ignored.
        std::fs::write(registry.join("README.md"), b"readme").unwrap();

        let mut outcome = DiscoveryScanOutcome::default();
        let names = collect_crate_names_from_registry(&root, &mut outcome).unwrap();

        // 3 directories counted (the file is skipped).
        assert_eq!(outcome.discovered, 3, "should count 3 dirs, not the file");
        assert_eq!(outcome.unparseable, 1, "not-a-valid-version is unparseable");
        assert!(names.contains(&"serde".to_string()), "serde should be discovered");
        assert!(names.contains(&"tokio-util".to_string()), "tokio-util should be discovered");
        assert!(
            !names.contains(&"not-a-valid-version".to_string()),
            "unparseable dir should not appear in names"
        );
    }

    #[test]
    fn collect_names_deduplicates_multiple_versions_of_same_crate() {
        let (root, _cleanup) = temp_registry_root("dedup");
        let registry = root.join("crates-io-test");
        std::fs::create_dir_all(registry.join("serde-1.0.0")).unwrap();
        std::fs::create_dir_all(registry.join("serde-1.1.0")).unwrap();

        let mut outcome = DiscoveryScanOutcome::default();
        let names = collect_crate_names_from_registry(&root, &mut outcome).unwrap();

        assert_eq!(outcome.discovered, 2, "two version dirs should be counted");
        assert_eq!(names.len(), 1, "two versions of serde should deduplicate to one unique name");
        assert_eq!(outcome.unparseable, 0);
    }

    #[test]
    fn collect_names_walks_multiple_registry_subdirs() {
        let (root, _cleanup) = temp_registry_root("multi-registry");
        // Simulate two registry mirrors (both under src_root).
        std::fs::create_dir_all(
            root.join("crates-io-abc")
                .join("serde-1.0.0"),
        )
        .unwrap();
        std::fs::create_dir_all(
            root.join("crates-io-xyz")
                .join("tokio-1.28.0"),
        )
        .unwrap();

        let mut outcome = DiscoveryScanOutcome::default();
        let names = collect_crate_names_from_registry(&root, &mut outcome).unwrap();

        assert_eq!(outcome.discovered, 2);
        assert!(names.contains(&"serde".to_string()));
        assert!(names.contains(&"tokio".to_string()));
    }

    #[test]
    fn collect_names_returns_error_for_missing_dir() {
        let mut outcome = DiscoveryScanOutcome::default();
        let result = collect_crate_names_from_registry(
            &std::path::PathBuf::from("/nonexistent/path/rust-mcp-test-xyz"),
            &mut outcome,
        );
        assert!(result.is_err(), "should return Err for a missing directory");
    }

    #[test]
    fn collect_names_returns_empty_for_empty_registry() {
        let (root, _cleanup) = temp_registry_root("empty");
        // Create the src_root but no registry subdirs inside it.
        std::fs::create_dir_all(&root).unwrap();

        let mut outcome = DiscoveryScanOutcome::default();
        let names = collect_crate_names_from_registry(&root, &mut outcome).unwrap();

        assert!(names.is_empty());
        assert_eq!(outcome.discovered, 0);
        assert_eq!(outcome.unparseable, 0);
    }
}
