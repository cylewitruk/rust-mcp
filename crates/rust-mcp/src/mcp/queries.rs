use crate::db::models::{CrateCoreRow, CrateVersionSelectionRow};
use crate::db::tools;
use crate::mcp::indexing::freshness::InteractionRefreshOutcome;
use crate::mcp::server::McpServer;

/// Outcome of looking up a crate, its latest version, and checking freshness.
pub(crate) struct CrateContext {
    pub crate_row: CrateCoreRow,
    pub latest_version: CrateVersionSelectionRow,
    pub freshness_outcome: InteractionRefreshOutcome,
}

/// Outcome of resolving a specific or latest version, with backfill tracking.
pub(crate) struct VersionResolution {
    pub selected_version: CrateVersionSelectionRow,
    pub refresh_enqueued: bool,
    pub refresh_job_id: Option<String>,
}

impl McpServer {
    /// Look up a crate by name, fetch its latest version, run the freshness
    /// check, and re-fetch the latest version if the check detected changes.
    ///
    /// This consolidates the crate-core-lookup + latest-version-lookup +
    /// ensure-freshness + conditional-re-fetch sequence used by most crate
    /// tools.
    pub(crate) async fn fetch_crate_context(
        &self,
        crate_name: &str,
    ) -> Result<CrateContext, String> {
        let crate_row = tools::fetch_crate_core_by_name(&self.state.db, crate_name)
            .await
            .map_err(|e| format!("crate lookup failed for {crate_name}: {e}"))?
            .ok_or_else(|| {
                format!("crate '{crate_name}' is not indexed locally; run index.sync_crates first")
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
    pub(crate) async fn resolve_version_or_latest(
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
}
