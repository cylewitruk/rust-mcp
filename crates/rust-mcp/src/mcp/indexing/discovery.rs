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

use std::collections::HashMap;
use std::path::Path;

use metrics::{counter, histogram};
use serde_json::json;
use tokio::time::{Duration, sleep};
use tracing::{info, warn};

use crate::db::indexing::{enqueue_or_get_refresh_job_id, fetch_known_crate_names};
use crate::mcp::indexing::coordinator::{PRIORITY_DISCOVERY, PRIORITY_PRE_WARM};
use crate::state::AppState;

/// Summary of a single registry discovery scan run.
#[derive(Debug, Default)]
pub struct DiscoveryScanOutcome {
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
pub async fn run_registry_discovery(state: AppState) {
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
pub async fn run_registry_scan(state: &AppState) -> DiscoveryScanOutcome {
    let scan_started = std::time::Instant::now();
    let mut outcome = DiscoveryScanOutcome::default();

    // ── 1. Walk the registry ──────────────────────────────────────────────────
    let src_root = state
        .config
        .cargo_registry_dir
        .join("src");
    let discovered_map = match collect_crate_versions_from_registry(&src_root, &mut outcome) {
        Ok(map) => map,
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
        let local_versions = discovered_map
            .get(crate_name.as_str())
            .cloned()
            .unwrap_or_default();
        if let Err(error) = enqueue_or_get_refresh_job_id(
            &state.db,
            crate_name,
            "crate",
            PRIORITY_PRE_WARM,
            false,
            json!({"trigger": "registry_discovery", "pre_warm": true, "local_versions": local_versions}),
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
    let unknown: Vec<&String> = discovered_map
        .keys()
        .filter(|name| !known_names.contains(*name))
        .collect();

    outcome.already_known = discovered_map
        .len()
        .saturating_sub(unknown.len());

    let to_enqueue =
        if batch_limit > 0 { &unknown[..unknown.len().min(batch_limit)] } else { &unknown[..] };

    for crate_name in to_enqueue {
        let local_versions = discovered_map
            .get(crate_name.as_str())
            .cloned()
            .unwrap_or_default();
        if let Err(error) = enqueue_or_get_refresh_job_id(
            &state.db,
            crate_name,
            "crate",
            PRIORITY_DISCOVERY,
            false,
            json!({"trigger": "registry_discovery", "local_versions": local_versions}),
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

/// Walk `src_root/{registry}/{name}-{version}/` and collect a mapping of
/// crate names to their locally-present version strings.
/// Updates `outcome.discovered` and `outcome.unparseable`.
fn collect_crate_versions_from_registry(
    src_root: &Path,
    outcome: &mut DiscoveryScanOutcome,
) -> Result<HashMap<String, Vec<String>>, String> {
    let mut map: HashMap<String, Vec<String>> = HashMap::new();

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
                Some((crate_name, version)) => {
                    map.entry(crate_name)
                        .or_default()
                        .push(version);
                }
                None => {
                    outcome.unparseable += 1;
                }
            }
        }
    }

    Ok(map)
}

/// Collects locally-present version strings for a single crate by walking
/// `src_root/{registry}/{name}-{version}/`.
///
/// Used by the worker as a fallback when the job payload does not include
/// `local_versions` (e.g. freshness-triggered or on-demand jobs).
pub fn collect_local_versions_for_crate(src_root: &Path, crate_name: &str) -> Vec<String> {
    let mut versions = Vec::new();

    let Ok(registries) = std::fs::read_dir(src_root) else {
        return versions;
    };

    for registry_entry in registries.flatten() {
        let registry_path = registry_entry.path();
        if !registry_path.is_dir() {
            continue;
        }

        let Ok(crate_dirs) = std::fs::read_dir(&registry_path) else {
            continue;
        };

        for crate_entry in crate_dirs.flatten() {
            let path = crate_entry.path();
            if !path.is_dir() {
                continue;
            }

            let Some(dir_name) = path
                .file_name()
                .and_then(|n| n.to_str())
            else {
                continue;
            };

            if let Some((name, version)) = parse_registry_dir_name(dir_name)
                && name == crate_name
            {
                versions.push(version);
            }
        }
    }

    versions
}

/// Parse a cargo registry directory name of the form `{name}-{semver}` into
/// its component parts. Iterates split points from right to correctly handle
/// crate names containing hyphens (e.g. `serde-derive-1.0.0`).
pub fn parse_registry_dir_name(dir_name: &str) -> Option<(String, String)> {
    let bytes = dir_name.as_bytes();
    for i in (1..dir_name.len()).rev() {
        if bytes[i - 1] == b'-' {
            let potential_name = &dir_name[..i - 1];
            let potential_version = &dir_name[i..];
            // Crate names may only contain [a-zA-Z0-9_-]. If the candidate
            // name contains '.' or '+' the split landed inside semver build
            // metadata (e.g. `toml_parser-1.0.6+spec-1.1.0`); keep scanning.
            if potential_name.contains('.') || potential_name.contains('+') {
                continue;
            }
            if semver::Version::parse(potential_version).is_ok() {
                return Some((potential_name.to_string(), potential_version.to_string()));
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
    use super::{
        DiscoveryScanOutcome, collect_crate_versions_from_registry,
        collect_local_versions_for_crate, parse_registry_dir_name,
    };

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
    fn parses_version_with_build_metadata_containing_hyphens() {
        let (name, version) = parse_registry_dir_name("toml_parser-1.0.6+spec-1.1.0").unwrap();
        assert_eq!(name, "toml_parser");
        assert_eq!(version, "1.0.6+spec-1.1.0");
    }

    #[test]
    fn parses_hyphenated_crate_with_build_metadata() {
        let (name, version) = parse_registry_dir_name("toml-datetime-0.7.5+spec-1.1.0").unwrap();
        assert_eq!(name, "toml-datetime");
        assert_eq!(version, "0.7.5+spec-1.1.0");
    }

    #[test]
    fn returns_none_for_no_version() {
        assert!(parse_registry_dir_name("just-a-name").is_none());
        assert!(parse_registry_dir_name("noversion").is_none());
    }

    // ── collect_crate_versions_from_registry ─────────────────────────────────

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
    fn collect_discovers_dirs_and_counts_unparseable() {
        let (root, _cleanup) = temp_registry_root("basic");
        let registry = root.join("crates-io-test");
        std::fs::create_dir_all(registry.join("serde-1.0.193")).unwrap();
        std::fs::create_dir_all(registry.join("tokio-util-0.7.10")).unwrap();
        std::fs::create_dir_all(registry.join("not-a-valid-version")).unwrap();
        std::fs::write(registry.join("README.md"), b"readme").unwrap();

        let mut outcome = DiscoveryScanOutcome::default();
        let map = collect_crate_versions_from_registry(&root, &mut outcome).unwrap();

        assert_eq!(outcome.discovered, 3, "should count 3 dirs, not the file");
        assert_eq!(outcome.unparseable, 1, "not-a-valid-version is unparseable");
        assert!(map.contains_key("serde"), "serde should be discovered");
        assert_eq!(map["serde"], vec!["1.0.193"]);
        assert!(map.contains_key("tokio-util"), "tokio-util should be discovered");
        assert!(!map.contains_key("not-a-valid-version"));
    }

    #[test]
    fn collect_groups_multiple_versions_of_same_crate() {
        let (root, _cleanup) = temp_registry_root("dedup");
        let registry = root.join("crates-io-test");
        std::fs::create_dir_all(registry.join("serde-1.0.0")).unwrap();
        std::fs::create_dir_all(registry.join("serde-1.1.0")).unwrap();

        let mut outcome = DiscoveryScanOutcome::default();
        let map = collect_crate_versions_from_registry(&root, &mut outcome).unwrap();

        assert_eq!(outcome.discovered, 2, "two version dirs should be counted");
        assert_eq!(map.len(), 1, "two versions of serde should group to one key");
        let mut versions = map["serde"].clone();
        versions.sort();
        assert_eq!(versions, vec!["1.0.0", "1.1.0"]);
        assert_eq!(outcome.unparseable, 0);
    }

    #[test]
    fn collect_walks_multiple_registry_subdirs() {
        let (root, _cleanup) = temp_registry_root("multi-registry");
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
        let map = collect_crate_versions_from_registry(&root, &mut outcome).unwrap();

        assert_eq!(outcome.discovered, 2);
        assert!(map.contains_key("serde"));
        assert!(map.contains_key("tokio"));
    }

    #[test]
    fn collect_returns_error_for_missing_dir() {
        let mut outcome = DiscoveryScanOutcome::default();
        let result = collect_crate_versions_from_registry(
            &std::path::PathBuf::from("/nonexistent/path/rust-mcp-test-xyz"),
            &mut outcome,
        );
        assert!(result.is_err(), "should return Err for a missing directory");
    }

    #[test]
    fn collect_returns_empty_for_empty_registry() {
        let (root, _cleanup) = temp_registry_root("empty");
        std::fs::create_dir_all(&root).unwrap();

        let mut outcome = DiscoveryScanOutcome::default();
        let map = collect_crate_versions_from_registry(&root, &mut outcome).unwrap();

        assert!(map.is_empty());
        assert_eq!(outcome.discovered, 0);
        assert_eq!(outcome.unparseable, 0);
    }

    // ── collect_local_versions_for_crate ─────────────────────────────────────

    #[test]
    fn single_crate_collects_matching_versions() {
        let (root, _cleanup) = temp_registry_root("single-crate");
        let registry = root.join("crates-io-test");
        std::fs::create_dir_all(registry.join("serde-1.0.0")).unwrap();
        std::fs::create_dir_all(registry.join("serde-1.1.0")).unwrap();
        std::fs::create_dir_all(registry.join("tokio-1.28.0")).unwrap();

        let mut versions = collect_local_versions_for_crate(&root, "serde");
        versions.sort();
        assert_eq!(versions, vec!["1.0.0", "1.1.0"]);
    }

    #[test]
    fn single_crate_returns_empty_for_unknown() {
        let (root, _cleanup) = temp_registry_root("single-crate-miss");
        let registry = root.join("crates-io-test");
        std::fs::create_dir_all(registry.join("serde-1.0.0")).unwrap();

        let versions = collect_local_versions_for_crate(&root, "tokio");
        assert!(versions.is_empty());
    }

    #[test]
    fn single_crate_returns_empty_for_missing_dir() {
        let versions = collect_local_versions_for_crate(
            &std::path::PathBuf::from("/nonexistent/path/rust-mcp-test-xyz"),
            "serde",
        );
        assert!(versions.is_empty());
    }
}
