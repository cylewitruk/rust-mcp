//! Near-real-time registry cache watcher.
//!
//! Monitors `{CARGO_REGISTRY_DIR}/cache/{hash}/` directories for new `.crate`
//! files using mtime-based stat polling. When `cargo add` or `cargo update`
//! downloads a new crate, the file appears in the cache directory and this
//! watcher detects it within the configured polling interval (default 1s),
//! enqueuing a refresh job via the existing pipeline.
//!
//! On startup, the watcher populates an in-memory index of known `.crate`
//! filenames (without enqueuing anything — the discovery module handles
//! initial sync). After startup, only newly-appearing files trigger enqueues.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use metrics::{counter, histogram};
use serde_json::json;
use tokio::time::{Duration, sleep};
use tracing::{info, warn};

use crate::db::indexing::enqueue_or_get_refresh_job_id;
use crate::mcp::indexing::coordinator::PRIORITY_CACHE_WATCH;
use crate::state::AppState;

/// In-memory index: registry subdir path → (last mtime, known `.crate`
/// filenames).
pub type CacheIndex = HashMap<PathBuf, (SystemTime, HashSet<OsString>)>;

/// Summary of a single cache watcher poll cycle.
#[derive(Debug, Default)]
pub struct CacheWatchPollOutcome {
    /// Number of registry subdirs checked.
    pub subdirs_checked: usize,
    /// Number of new `.crate` files detected this cycle.
    pub new_files: usize,
    /// Number of refresh jobs enqueued this cycle.
    pub enqueued: usize,
    /// Number of filenames that could not be parsed.
    pub unparseable: usize,
}

// ──────────────────────────────────────────────────────────────────────────────
// Entry point
// ──────────────────────────────────────────────────────────────────────────────

/// Long-running background task that watches the cargo registry cache for new
/// `.crate` files.
///
/// Populates the in-memory cache index on startup (without enqueuing), then
/// polls at `config.registry_cache_watch_interval_ms`. If the interval is 0,
/// the function exits immediately (disabled).
pub async fn run_cache_watcher(state: AppState) {
    let interval_ms = state
        .config
        .registry_cache_watch_interval_ms;
    if interval_ms == 0 {
        info!("REGISTRY_CACHE_WATCH_INTERVAL_MS=0; cache watcher disabled");
        return;
    }

    let cache_root = state
        .config
        .cargo_registry_dir
        .join("cache");

    let mut index = match build_initial_index(&cache_root) {
        Ok(idx) => {
            let total_files: usize = idx
                .values()
                .map(|(_, set)| set.len())
                .sum();
            info!(
                subdirs = idx.len(),
                known_files = total_files,
                "cache watcher: initial index populated"
            );
            idx
        }
        Err(error) => {
            warn!(
                %error,
                "cache watcher: failed to read cache root; starting with empty index"
            );
            CacheIndex::new()
        }
    };

    let interval = Duration::from_millis(interval_ms);
    loop {
        sleep(interval).await;
        let outcome = poll_cache_changes(&state, &cache_root, &mut index).await;
        if outcome.new_files > 0 {
            info!(
                new_files = outcome.new_files,
                enqueued = outcome.enqueued,
                unparseable = outcome.unparseable,
                "cache watcher: detected new crate files"
            );
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Index building
// ──────────────────────────────────────────────────────────────────────────────

/// Walk `cache_root/{hash}/` and collect all existing `.crate` filenames.
pub fn build_initial_index(cache_root: &Path) -> Result<CacheIndex, String> {
    let mut index = CacheIndex::new();

    let subdirs = std::fs::read_dir(cache_root)
        .map_err(|e| format!("cannot read cache root `{}`: {e}", cache_root.display()))?;

    for entry in subdirs {
        let entry = match entry {
            Ok(e) => e,
            Err(error) => {
                warn!(%error, "cache watcher: failed to read cache subdir entry");
                continue;
            }
        };

        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let (mtime, filenames) = snapshot_subdir(&path);
        index.insert(path, (mtime, filenames));
    }

    Ok(index)
}

/// Read a single registry subdir and return (mtime, set of `.crate` filenames).
fn snapshot_subdir(subdir: &Path) -> (SystemTime, HashSet<OsString>) {
    let mtime = std::fs::metadata(subdir)
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);

    let mut filenames = HashSet::new();
    if let Ok(entries) = std::fs::read_dir(subdir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            if name
                .to_string_lossy()
                .ends_with(".crate")
            {
                filenames.insert(name);
            }
        }
    }

    (mtime, filenames)
}

// ──────────────────────────────────────────────────────────────────────────────
// Poll cycle
// ──────────────────────────────────────────────────────────────────────────────

/// Check each tracked subdir for mtime changes, plus discover new subdirs.
pub async fn poll_cache_changes(
    state: &AppState,
    cache_root: &Path,
    index: &mut CacheIndex,
) -> CacheWatchPollOutcome {
    let poll_started = std::time::Instant::now();
    let mut outcome = CacheWatchPollOutcome::default();

    // Discover any new subdirs that appeared since last poll.
    if let Ok(subdirs) = std::fs::read_dir(cache_root) {
        for entry in subdirs.flatten() {
            let path = entry.path();
            if path.is_dir() && !index.contains_key(&path) {
                let (mtime, filenames) = snapshot_subdir(&path);
                let new_files: HashSet<OsString> = filenames.clone();
                index.insert(path, (mtime, filenames));
                enqueue_new_files(state, &new_files, &mut outcome).await;
            }
        }
    }

    // Check mtime of known subdirs.
    let subdir_paths: Vec<PathBuf> = index
        .keys()
        .cloned()
        .collect();
    for subdir in &subdir_paths {
        outcome.subdirs_checked += 1;

        let current_mtime = match std::fs::metadata(subdir).and_then(|m| m.modified()) {
            Ok(t) => t,
            Err(_) => continue,
        };

        let (stored_mtime, known) = index.get(subdir).unwrap();
        if current_mtime == *stored_mtime {
            continue;
        }

        // mtime changed — do a full read_dir and diff.
        let (new_mtime, current_files) = snapshot_subdir(subdir);
        let new_files: HashSet<OsString> = current_files
            .difference(known)
            .cloned()
            .collect();

        if !new_files.is_empty() {
            enqueue_new_files(state, &new_files, &mut outcome).await;
        }

        index.insert(subdir.clone(), (new_mtime, current_files));
    }

    let elapsed_ms = poll_started
        .elapsed()
        .as_millis() as f64;
    histogram!("rust_mcp_cache_watch_poll_duration_ms").record(elapsed_ms);
    if outcome.enqueued > 0 {
        counter!("rust_mcp_cache_watch_jobs_enqueued_total").increment(outcome.enqueued as u64);
    }

    outcome
}

// ──────────────────────────────────────────────────────────────────────────────
// Filename parsing + enqueue
// ──────────────────────────────────────────────────────────────────────────────

/// Parse a `.crate` filename like `serde-1.0.193.crate` into `(name, version)`.
fn parse_crate_filename(filename: &str) -> Option<(String, String)> {
    let stem = filename.strip_suffix(".crate")?;
    super::discovery::parse_registry_dir_name(stem)
}

/// Enqueue refresh jobs for newly detected `.crate` files.
async fn enqueue_new_files(
    state: &AppState,
    new_files: &HashSet<OsString>,
    outcome: &mut CacheWatchPollOutcome,
) {
    let mut crate_names: HashMap<String, Vec<String>> = HashMap::new();
    for filename in new_files {
        outcome.new_files += 1;
        let Some(filename_str) = filename.to_str() else {
            outcome.unparseable += 1;
            continue;
        };
        match parse_crate_filename(filename_str) {
            Some((name, version)) => {
                crate_names
                    .entry(name)
                    .or_default()
                    .push(version);
            }
            None => {
                outcome.unparseable += 1;
            }
        }
    }

    for (crate_name, versions) in &crate_names {
        match enqueue_or_get_refresh_job_id(
            &state.db,
            crate_name,
            "crate",
            PRIORITY_CACHE_WATCH,
            false,
            json!({
                "trigger": "cache_watcher",
                "detected_versions": versions,
            }),
        )
        .await
        {
            Ok(_) => {
                outcome.enqueued += 1;
            }
            Err(error) => {
                warn!(%error, %crate_name, "cache watcher: failed to enqueue job");
            }
        }
    }

    if outcome.enqueued > 0 {
        state
            .indexing_coordinator
            .notify_worker();
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Unit tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_temp_dir(tag: &str) -> (PathBuf, TempCleanup) {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos();
        let root = std::env::temp_dir().join(format!("rust-mcp-cachewatch-{tag}-{nanos}"));
        std::fs::create_dir_all(&root).unwrap();
        (root.clone(), TempCleanup(root))
    }

    struct TempCleanup(PathBuf);
    impl Drop for TempCleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn parse_simple_crate_filename() {
        let (name, version) = parse_crate_filename("serde-1.0.193.crate").unwrap();
        assert_eq!(name, "serde");
        assert_eq!(version, "1.0.193");
    }

    #[test]
    fn parse_hyphenated_crate_filename() {
        let (name, version) = parse_crate_filename("serde-derive-1.0.193.crate").unwrap();
        assert_eq!(name, "serde-derive");
        assert_eq!(version, "1.0.193");
    }

    #[test]
    fn parse_build_metadata_crate_filename() {
        let (name, version) = parse_crate_filename("toml_parser-1.0.6+spec-1.1.0.crate").unwrap();
        assert_eq!(name, "toml_parser");
        assert_eq!(version, "1.0.6+spec-1.1.0");
    }

    #[test]
    fn parse_returns_none_for_non_crate() {
        assert!(parse_crate_filename("README.md").is_none());
        assert!(parse_crate_filename("serde-1.0.193").is_none());
        assert!(parse_crate_filename(".crate").is_none());
    }

    #[test]
    fn initial_index_captures_existing_files() {
        let (root, _cleanup) = make_temp_dir("initial");
        let subdir = root.join("crates-io-1234abcd");
        std::fs::create_dir_all(&subdir).unwrap();
        std::fs::write(subdir.join("serde-1.0.193.crate"), b"fake").unwrap();
        std::fs::write(subdir.join("tokio-1.28.0.crate"), b"fake").unwrap();

        let index = build_initial_index(&root).unwrap();
        assert_eq!(index.len(), 1);
        let (_, filenames) = index.values().next().unwrap();
        assert_eq!(filenames.len(), 2);
    }

    #[test]
    fn snapshot_subdir_filters_non_crate_files() {
        let (root, _cleanup) = make_temp_dir("filter");
        let subdir = root.join("test-registry");
        std::fs::create_dir_all(&subdir).unwrap();
        std::fs::write(subdir.join("serde-1.0.193.crate"), b"fake").unwrap();
        std::fs::write(subdir.join("README.md"), b"readme").unwrap();
        std::fs::write(subdir.join(".cargo-lock"), b"lock").unwrap();

        let (_, filenames) = snapshot_subdir(&subdir);
        assert_eq!(filenames.len(), 1);
    }

    #[test]
    fn initial_index_returns_error_for_missing_dir() {
        let result = build_initial_index(Path::new("/nonexistent/path/rust-mcp-cachewatch-test"));
        assert!(result.is_err());
    }

    #[test]
    fn initial_index_skips_non_directory_entries() {
        let (root, _cleanup) = make_temp_dir("skipfiles");
        std::fs::write(root.join("some-file.txt"), b"not a dir").unwrap();
        let subdir = root.join("crates-io-abc");
        std::fs::create_dir_all(&subdir).unwrap();
        std::fs::write(subdir.join("itoa-1.0.1.crate"), b"fake").unwrap();

        let index = build_initial_index(&root).unwrap();
        assert_eq!(index.len(), 1);
    }
}
