//! Lightweight git-based repository probing for commit liveness data.
//!
//! Performs shallow bare clones to extract recent commit history without
//! consuming GitHub API rate limits. Clones are placed in a temporary
//! subdirectory under the configured data dir and cleaned up after extraction.

use std::path::Path;
use std::time::Duration;

use tokio::process::Command;
use tracing::{debug, warn};

/// Extracted commit liveness data from a shallow git clone.
#[derive(Debug, Clone, Default)]
pub struct GitProbeResult {
    /// ISO 8601 timestamp of the most recent commit on the default branch.
    pub last_commit_at: Option<String>,
    /// Subject line of the most recent commit.
    pub last_commit_message: Option<String>,
    /// Number of commits within the last 90 days.
    pub recent_commit_count: i64,
}

/// Probes a git repository URL for commit liveness data.
///
/// Performs a `--bare --depth=N` clone into a temporary directory under
/// `base_dir`, extracts commit metadata via `git log`, then cleans up.
///
/// Returns `Ok(GitProbeResult)` on success; errors are non-fatal and callers
/// should treat them as best-effort.
pub async fn probe_repo(
    repo_url: &str,
    owner: &str,
    repo: &str,
    base_dir: &Path,
    clone_depth: u32,
    timeout_secs: u64,
) -> Result<GitProbeResult, String> {
    // Validate URL to prevent command injection: must start with https://
    // and contain only safe characters.
    if !repo_url.starts_with("https://") {
        return Err(format!("refusing to clone non-HTTPS URL: {repo_url}"));
    }
    if repo_url.contains([';', '|', '&', '$', '`']) {
        return Err(format!("refusing to clone URL with shell metacharacters: {repo_url}"));
    }

    let probe_dir = base_dir.join("git-probes");
    tokio::fs::create_dir_all(&probe_dir)
        .await
        .map_err(|e| format!("failed to create git-probes dir: {e}"))?;

    let clone_dir = probe_dir.join(format!("{owner}__{repo}"));

    // Clean up any stale clone from a previous run.
    if clone_dir.exists() {
        let _ = tokio::fs::remove_dir_all(&clone_dir).await;
    }

    let timeout = Duration::from_secs(timeout_secs);

    // Clone: bare, shallow, single-branch (default branch only).
    let clone_result = run_git_command(
        &[
            "clone",
            "--bare",
            "--depth",
            &clone_depth.to_string(),
            "--single-branch",
            repo_url,
            clone_dir
                .to_str()
                .unwrap_or(""),
        ],
        None,
        timeout,
    )
    .await;

    if let Err(e) = clone_result {
        let _ = tokio::fs::remove_dir_all(&clone_dir).await;
        return Err(format!("git clone failed for {owner}/{repo}: {e}"));
    }

    let result = extract_commit_data(&clone_dir).await;

    // Always clean up.
    if let Err(e) = tokio::fs::remove_dir_all(&clone_dir).await {
        warn!(
            %owner, %repo,
            error = %e,
            "failed to clean up git probe directory"
        );
    }

    result.map_err(|e| format!("git log extraction failed for {owner}/{repo}: {e}"))
}

/// Extracts commit data from a bare clone directory.
async fn extract_commit_data(clone_dir: &Path) -> Result<GitProbeResult, String> {
    let timeout = Duration::from_secs(30);

    // Get the most recent commit: date and subject.
    let latest_output =
        run_git_command(&["log", "--format=%aI%n%s", "-1"], Some(clone_dir), timeout).await?;

    let mut lines = latest_output.lines();
    let last_commit_at = lines
        .next()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let last_commit_message = lines
        .next()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    // Count commits in the last 90 days.
    let count_output = run_git_command(
        &["rev-list", "--count", "--since=90 days ago", "HEAD"],
        Some(clone_dir),
        timeout,
    )
    .await?;

    let recent_commit_count = count_output
        .trim()
        .parse::<i64>()
        .unwrap_or(0);

    debug!(
        ?last_commit_at,
        ?last_commit_message,
        recent_commit_count,
        "git probe extracted commit data"
    );

    Ok(GitProbeResult {
        last_commit_at,
        last_commit_message,
        recent_commit_count,
    })
}

/// Runs a git command with the given arguments and optional working directory.
async fn run_git_command(
    args: &[&str],
    work_dir: Option<&Path>,
    timeout: Duration,
) -> Result<String, String> {
    let mut cmd = Command::new("git");
    cmd.args(args);
    // Prevent git from prompting for credentials or using terminal features.
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    cmd.env("GIT_ASKPASS", "true");

    if let Some(dir) = work_dir {
        cmd.current_dir(dir);
    }

    let output = tokio::time::timeout(timeout, cmd.output())
        .await
        .map_err(|_| "git command timed out".to_string())?
        .map_err(|e| format!("failed to spawn git: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "git {} exited with {}: {}",
            args.first().unwrap_or(&""),
            output.status,
            stderr.trim()
        ));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Returns a clone URL for a GitHub repository.
pub fn github_clone_url(owner: &str, repo: &str) -> String {
    format!("https://github.com/{owner}/{repo}.git")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_https_url() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(probe_repo(
            "http://github.com/foo/bar.git",
            "foo",
            "bar",
            Path::new("/tmp"),
            10,
            30,
        ));
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .contains("non-HTTPS")
        );
    }

    #[test]
    fn rejects_url_with_shell_metacharacters() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(probe_repo(
            "https://github.com/foo/bar;rm -rf /",
            "foo",
            "bar",
            Path::new("/tmp"),
            10,
            30,
        ));
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .contains("metacharacters")
        );
    }

    #[test]
    fn github_clone_url_formats_correctly() {
        assert_eq!(github_clone_url("serde-rs", "serde"), "https://github.com/serde-rs/serde.git");
    }
}
