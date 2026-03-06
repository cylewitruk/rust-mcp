//! On-demand and periodic GitHub repo metadata synchronization.
//!
//! Fetches repository metadata (stars, forks, issues, archived status, last
//! push, license, contributor count) and recent releases from the GitHub REST
//! API for indexed crates that have a `github.com` repository URL. Metadata is
//! stored in `github_repo_metadata` and `github_releases` and surfaced in
//! `crate_intel`.

use tracing::{debug, info, warn};

use crate::db::indexing::{
    fetch_crates_needing_github_metadata, update_git_probe_data, upsert_github_metadata,
    upsert_github_releases,
};
use crate::db::models::GitHubMetadataInsert;
use crate::integration::git_probe;
use crate::integration::github::{GitHubClient, parse_github_owner_repo};
use crate::mcp::server::McpServer;

/// Maximum number of releases to fetch per repository.
const RELEASES_PER_REPO: u8 = 10;

/// Outcome of a GitHub metadata sync pass.
#[derive(Debug, Default)]
pub struct GitHubSyncOutcome {
    /// Number of crates whose metadata was fetched.
    pub crates_processed: usize,
    /// Number of errors encountered.
    pub errors: Vec<String>,
}

impl McpServer {
    /// Syncs GitHub repo metadata for crates that need it.
    pub async fn sync_github_metadata(
        &self,
        limit: i64,
        max_age_secs: i64,
    ) -> Result<GitHubSyncOutcome, String> {
        let candidates = fetch_crates_needing_github_metadata(&self.state.db, max_age_secs, limit)
            .await
            .map_err(|e| format!("failed to fetch GitHub metadata candidates: {e}"))?;

        let mut outcome = GitHubSyncOutcome::default();
        let github = GitHubClient::new(&self.state);

        for candidate in candidates {
            let Some(repo_url) = candidate
                .repository_url
                .as_deref()
            else {
                continue;
            };

            let Some((owner, repo)) = parse_github_owner_repo(repo_url) else {
                debug!(
                    crate_name = %candidate.crate_name,
                    repository_url = %repo_url,
                    "skipping non-GitHub or unparseable repository URL"
                );
                continue;
            };

            match github
                .fetch_repo_metadata(&owner, &repo)
                .await
            {
                Ok(meta) => {
                    let pushed_at = meta.pushed_at.as_deref();

                    let license_spdx = meta
                        .license
                        .as_ref()
                        .and_then(|l| l.spdx_id.as_deref())
                        .filter(|s| *s != "NOASSERTION");

                    // Fetch contributor count (best-effort, separate request).
                    let contributor_count = github
                        .fetch_contributor_count(&owner, &repo)
                        .await
                        .unwrap_or(None)
                        .map(|c| c as i64);

                    let insert = GitHubMetadataInsert {
                        owner: &owner,
                        repo: &repo,
                        stargazers_count: meta.stargazers_count as i64,
                        forks_count: meta.forks_count as i64,
                        open_issues_count: meta.open_issues_count as i64,
                        archived: meta.archived,
                        pushed_at,
                        license_spdx,
                        contributor_count,
                    };

                    if let Err(e) =
                        upsert_github_metadata(&self.state.db, candidate.crate_id, &insert).await
                    {
                        outcome.errors.push(format!(
                            "failed to upsert GitHub metadata for {}: {e}",
                            candidate.crate_name
                        ));
                        continue;
                    }

                    // Fetch and store recent releases (best-effort).
                    match github
                        .fetch_releases(&owner, &repo, RELEASES_PER_REPO)
                        .await
                    {
                        Ok(releases) => {
                            let release_rows: Vec<_> = releases
                                .into_iter()
                                .filter(|r| !r.draft)
                                .map(|r| (r.tag_name, r.name, r.body, r.published_at, r.prerelease))
                                .collect();
                            if let Err(e) = upsert_github_releases(
                                &self.state.db,
                                candidate.crate_id,
                                &release_rows,
                            )
                            .await
                            {
                                warn!(
                                    crate_name = %candidate.crate_name,
                                    error = %e,
                                    "failed to upsert GitHub releases"
                                );
                            }
                        }
                        Err(e) => {
                            debug!(
                                crate_name = %candidate.crate_name,
                                error = %e,
                                "failed to fetch GitHub releases (non-fatal)"
                            );
                        }
                    }

                    // Git probe for commit liveness (best-effort, no API quota).
                    if self
                        .state
                        .config
                        .git_probe_enabled
                        && self
                            .state
                            .config
                            .git_probe_clone_depth
                            > 0
                    {
                        let clone_url = git_probe::github_clone_url(&owner, &repo);
                        match git_probe::probe_repo(
                            &clone_url,
                            &owner,
                            &repo,
                            &self.state.config.data_dir,
                            self.state
                                .config
                                .git_probe_clone_depth,
                            self.state
                                .config
                                .git_probe_timeout_secs,
                        )
                        .await
                        {
                            Ok(probe) => {
                                if let Err(e) = update_git_probe_data(
                                    &self.state.db,
                                    candidate.crate_id,
                                    probe
                                        .last_commit_at
                                        .as_deref(),
                                    probe
                                        .last_commit_message
                                        .as_deref(),
                                    probe.recent_commit_count,
                                )
                                .await
                                {
                                    warn!(
                                        crate_name = %candidate.crate_name,
                                        error = %e,
                                        "failed to persist git probe data"
                                    );
                                }
                            }
                            Err(e) => {
                                debug!(
                                    crate_name = %candidate.crate_name,
                                    error = %e,
                                    "git probe failed (non-fatal)"
                                );
                            }
                        }
                    }

                    debug!(
                        crate_name = %candidate.crate_name,
                        stars = meta.stargazers_count,
                        contributors = ?contributor_count,
                        "GitHub metadata synced"
                    );
                    outcome.crates_processed += 1;
                }
                Err(error) => {
                    outcome.errors.push(error);
                }
            }
        }

        Ok(outcome)
    }
}

/// Runs a periodic GitHub metadata sync loop.
pub async fn run_github_metadata_sync(state: crate::state::AppState) {
    use tokio::time::{Duration, sleep};

    // Use the security sync interval as the staleness threshold.
    let interval_secs = state
        .config
        .security_sync_interval_secs;
    let max_age_secs = if interval_secs == 0 { 86400 } else { interval_secs as i64 };
    let server = McpServer::new(state.clone());

    // Startup pass.
    match server
        .sync_github_metadata(50, max_age_secs)
        .await
    {
        Ok(outcome) => {
            if outcome.crates_processed > 0 || !outcome.errors.is_empty() {
                info!(
                    processed = outcome.crates_processed,
                    errors = outcome.errors.len(),
                    "startup GitHub metadata sync complete"
                );
            }
        }
        Err(error) => warn!(%error, "startup GitHub metadata sync failed"),
    }

    if interval_secs == 0 {
        return;
    }

    loop {
        sleep(Duration::from_secs(interval_secs)).await;

        match server
            .sync_github_metadata(50, max_age_secs)
            .await
        {
            Ok(outcome) => {
                if outcome.crates_processed > 0 || !outcome.errors.is_empty() {
                    info!(
                        processed = outcome.crates_processed,
                        errors = outcome.errors.len(),
                        "periodic GitHub metadata sync complete"
                    );
                }
            }
            Err(error) => warn!(%error, "periodic GitHub metadata sync failed"),
        }
    }
}
