use crate::db::models::{CrateCoreRow, CrateVersionSelectionRow};
use crate::db::tools;
use crate::mcp::indexing::coordinator::JobOutcome;
use crate::mcp::indexing::freshness::InteractionRefreshOutcome;
use crate::mcp::server::McpServer;

/// Outcome of looking up a crate, its latest version, and checking freshness.
pub struct CrateContext {
    /// Core crate metadata row.
    pub crate_row: CrateCoreRow,
    /// The latest non-yanked version of the crate.
    pub latest_version: CrateVersionSelectionRow,
    /// Result of the freshness check triggered by this interaction.
    pub freshness_outcome: InteractionRefreshOutcome,
}

/// Outcome of resolving a specific or latest version, with backfill tracking.
pub struct VersionResolution {
    /// The resolved crate version row.
    pub selected_version: CrateVersionSelectionRow,
    /// Whether a backfill refresh was enqueued for a missing version.
    pub refresh_enqueued: bool,
    /// The job ID of the enqueued backfill refresh, if any.
    pub refresh_job_id: Option<String>,
}

impl McpServer {
    /// Ensures the named crate is indexed locally. If it is not present in
    /// the database, enqueues a high-priority on-demand indexing job via the
    /// refresh worker and waits for completion (up to the coordinator
    /// timeout).
    async fn ensure_crate_indexed(&self, crate_name: &str) -> Result<(), String> {
        let exists = tools::fetch_crate_core_by_name(&self.state.db, crate_name)
            .await
            .map_err(|e| format!("crate existence check failed for {crate_name}: {e}"))?;

        if exists.is_some() {
            return Ok(());
        }

        tracing::info!(crate_name, "crate not indexed locally — triggering on-demand indexing");

        let job_id = self
            .state
            .indexing_coordinator
            .enqueue_on_demand(&self.state.db, crate_name, "crate")
            .await?;

        match self
            .state
            .indexing_coordinator
            .wait_for_job(job_id)
            .await
        {
            Ok(JobOutcome::Completed) => Ok(()),
            Ok(JobOutcome::Failed(msg)) => {
                Err(format!("on-demand indexing failed for '{crate_name}': {msg}"))
            }
            Ok(JobOutcome::Pending) => Err(format!(
                "unexpected pending state after waiting for on-demand indexing of '{crate_name}'"
            )),
            Err(timeout_msg) => Err(timeout_msg),
        }
    }

    /// Look up a crate by name, fetch its latest version, run the freshness
    /// check, and re-fetch the latest version if the check detected changes.
    ///
    /// If the crate is not yet indexed locally, triggers on-demand indexing
    /// via the refresh worker before proceeding.
    ///
    /// This consolidates the crate-core-lookup + latest-version-lookup +
    /// ensure-freshness + conditional-re-fetch sequence used by most crate
    /// tools.
    pub async fn fetch_crate_context(&self, crate_name: &str) -> Result<CrateContext, String> {
        // On-demand indexing: ensure the crate is present before querying.
        self.ensure_crate_indexed(crate_name)
            .await?;

        let crate_row = tools::fetch_crate_core_by_name(&self.state.db, crate_name)
            .await
            .map_err(|e| format!("crate lookup failed for {crate_name}: {e}"))?
            .ok_or_else(|| {
                format!("crate '{crate_name}' could not be indexed; it may not exist on crates.io")
            })?;

        let latest_version = tools::fetch_latest_crate_version(&self.state.db, crate_row.id)
            .await
            .map_err(|e| format!("latest version lookup failed for {crate_name}: {e}"))?
            .ok_or_else(|| {
                format!(
                    "crate '{}' has no indexed versions yet; run index.sync_crates first",
                    crate_row.name
                )
            })?;

        let freshness_outcome = self
            .ensure_freshness_for_interaction(
                crate_row.id,
                &crate_row.name,
                &latest_version.version,
            )
            .await?;

        let latest_version = if freshness_outcome.freshness_check_result == "changed" {
            tools::fetch_latest_crate_version(&self.state.db, crate_row.id)
                .await
                .map_err(|e| format!("latest version relookup failed for {crate_name}: {e}"))?
                .ok_or_else(|| {
                    format!(
                        "crate '{}' has no indexed versions yet; run index.sync_crates first",
                        crate_row.name
                    )
                })?
        } else {
            latest_version
        };

        Ok(CrateContext {
            crate_row,
            latest_version,
            freshness_outcome,
        })
    }

    /// Resolve a specific requested version or fall back to the latest.
    /// If the requested version is missing, triggers a backfill and retries.
    ///
    /// Returns the resolved version row plus whether a refresh was enqueued.
    pub async fn resolve_version_or_latest(
        &self,
        ctx: &CrateContext,
        requested_version: Option<&str>,
    ) -> Result<VersionResolution, String> {
        let mut refresh_enqueued = ctx
            .freshness_outcome
            .refresh_enqueued;
        let mut refresh_job_id = ctx
            .freshness_outcome
            .refresh_job_id
            .clone();

        let selected_version = if let Some(version) = requested_version {
            let selected =
                tools::fetch_crate_version_by_name(&self.state.db, ctx.crate_row.id, version)
                    .await
                    .map_err(|e| {
                        format!(
                            "selected version lookup failed for {}@{}: {e}",
                            ctx.crate_row.name, version
                        )
                    })?;

            if let Some(selected) = selected {
                selected
            } else {
                let queued_job_id = self
                    .backfill_missing_requested_version(&ctx.crate_row.name)
                    .await?;
                if let Some(job_id) = queued_job_id {
                    refresh_enqueued = true;
                    refresh_job_id = Some(job_id);
                }

                tools::fetch_crate_version_by_name(&self.state.db, ctx.crate_row.id, version)
                    .await
                    .map_err(|e| {
                        format!(
                            "selected version lookup failed after backfill for {}@{}: {e}",
                            ctx.crate_row.name, version
                        )
                    })?
                    .ok_or_else(|| {
                        format!(
                            "version '{}' for crate '{}' is not indexed locally (refresh \
                             attempted)",
                            version, ctx.crate_row.name
                        )
                    })?
            }
        } else {
            ctx.latest_version.clone()
        };

        Ok(VersionResolution {
            selected_version,
            refresh_enqueued,
            refresh_job_id,
        })
    }

    /// Best-effort on-demand source file indexing for a crate version.
    ///
    /// If source files are already indexed, returns immediately. Otherwise
    /// enqueues a `local_cache` scope job via the coordinator and waits.
    /// Failures are logged but do **not** propagate — the caller proceeds
    /// with whatever data is available.
    pub async fn ensure_source_indexed(
        &self,
        crate_name: &str,
        crate_version_id: i64,
    ) -> Result<(), String> {
        let has_source = tools::has_source_files_for_version(&self.state.db, crate_version_id)
            .await
            .map_err(|e| format!("source file existence check failed for {crate_name}: {e}"))?;

        if has_source {
            return Ok(());
        }

        tracing::info!(
            crate_name,
            crate_version_id,
            "no source files indexed — triggering on-demand local_cache indexing"
        );

        let job_id = self
            .state
            .indexing_coordinator
            .enqueue_on_demand(&self.state.db, crate_name, "local_cache")
            .await?;

        match self
            .state
            .indexing_coordinator
            .wait_for_job(job_id)
            .await
        {
            Ok(JobOutcome::Completed) => {}
            Ok(JobOutcome::Failed(msg)) => {
                tracing::warn!(
                    crate_name,
                    %msg,
                    "on-demand source indexing failed (best-effort)"
                );
            }
            Ok(JobOutcome::Pending) => {
                tracing::warn!(
                    crate_name,
                    "unexpected pending state after on-demand source indexing wait"
                );
            }
            Err(timeout_msg) => {
                tracing::warn!(crate_name, %timeout_msg, "on-demand source indexing timed out");
            }
        }

        Ok(())
    }

    /// Best-effort on-demand rustdoc JSON indexing for a crate version.
    ///
    /// If rustdoc-backed symbols are already indexed, returns immediately.
    /// Otherwise enqueues a `rustdoc_json` scope job via the coordinator and
    /// waits. Failures are logged but do **not** propagate — the caller
    /// proceeds with syn-only data.
    pub async fn ensure_rustdoc_indexed(
        &self,
        crate_name: &str,
        crate_version_id: i64,
    ) -> Result<(), String> {
        let has_rustdoc = tools::has_rustdoc_symbols_for_version(&self.state.db, crate_version_id)
            .await
            .map_err(|e| format!("rustdoc symbol existence check failed for {crate_name}: {e}"))?;

        if has_rustdoc {
            return Ok(());
        }

        tracing::info!(
            crate_name,
            crate_version_id,
            "no rustdoc symbols indexed — triggering on-demand rustdoc_json indexing"
        );

        let job_id = self
            .state
            .indexing_coordinator
            .enqueue_on_demand(&self.state.db, crate_name, "rustdoc_json")
            .await?;

        match self
            .state
            .indexing_coordinator
            .wait_for_job(job_id)
            .await
        {
            Ok(JobOutcome::Completed) => {}
            Ok(JobOutcome::Failed(msg)) => {
                tracing::warn!(
                    crate_name,
                    %msg,
                    "on-demand rustdoc indexing failed (best-effort)"
                );
            }
            Ok(JobOutcome::Pending) => {
                tracing::warn!(
                    crate_name,
                    "unexpected pending state after on-demand rustdoc indexing wait"
                );
            }
            Err(timeout_msg) => {
                tracing::warn!(crate_name, %timeout_msg, "on-demand rustdoc indexing timed out");
            }
        }

        Ok(())
    }
}
