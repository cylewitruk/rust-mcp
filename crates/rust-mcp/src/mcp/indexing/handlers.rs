use rmcp::Json;
pub use rust_mcp_types::types::index::{
    IndexCoverage, IndexFailureByScope, IndexFreshness, IndexOperationalMetrics, IndexQueue,
    IndexRefreshRequest, IndexRefreshResponse, IndexRefreshResult, IndexRefreshScope,
    IndexRetryDistribution, IndexStatusRequest, IndexStatusResponse, IndexSyncCratesRequest,
    IndexSyncCratesResponse,
};
use serde_json::Value;
use tracing::{debug, info};

use crate::db::indexing::{
    enqueue_or_get_refresh_job_id, fetch_index_coverage_counts, fetch_index_failures_by_scope,
    fetch_index_freshness, fetch_index_operational_metrics_24h, fetch_index_queue_counts,
    fetch_recent_refresh_job_errors, fetch_refresh_eta_stats, fetch_refresh_job_retry_distribution,
    persist_crate_sync,
};
use crate::integration::crates_io::{
    CratesIoClient, CratesIoCrateDetailResponse, CratesIoSearchResponse,
};
use crate::mcp::models::ResponseFreshnessSource;
use crate::mcp::server::McpServer;
use crate::mcp::utils::{
    DEFAULT_SYNC_QUERY, dedupe_strings, normalize_optional, normalize_required, sync_page,
    sync_per_page,
};

/// Outcome of syncing a single crate's metadata from crates.io.
#[derive(Debug)]
pub struct SyncCrateOutcome {
    /// Number of crate versions that were persisted.
    pub versions_synced: usize,
    /// Number of dependency edges that were persisted.
    pub dependencies_synced: usize,
    /// The primary version selected for detailed indexing, if any.
    pub selected_version: Option<String>,
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

fn now_epoch_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default()
}

impl McpServer {
    fn pick_primary_version(detail: &CratesIoCrateDetailResponse) -> Option<String> {
        if let Some(max_version) = detail
            .krate
            .max_version
            .clone()
            && detail
                .versions
                .iter()
                .any(|v| v.num == max_version)
        {
            return Some(max_version);
        }

        detail
            .versions
            .iter()
            .find(|v| !v.yanked)
            .map(|v| v.num.clone())
            .or_else(|| {
                detail
                    .versions
                    .first()
                    .map(|v| v.num.clone())
            })
    }

    /// Fetches and persists a single crate's metadata, versions, and
    /// dependencies from crates.io.
    pub async fn sync_single_crate(
        &self,
        crate_name: &str,
        include_dependencies: bool,
    ) -> Result<SyncCrateOutcome, String> {
        info!(%crate_name, "syncing crate metadata from crates.io");

        let crates_io = CratesIoClient::new(&self.state);
        let detail = crates_io
            .fetch_crate_detail(crate_name)
            .await?;

        let selected_version = Self::pick_primary_version(&detail);
        debug!(
            %crate_name,
            selected_version = selected_version.as_deref().unwrap_or("none"),
            total_versions = detail.versions.len(),
            "selected primary version"
        );

        let readme = if let Some(version) = selected_version.as_deref() {
            crates_io
                .fetch_readme(crate_name, version)
                .await?
        } else {
            None
        };
        debug!(
            %crate_name,
            version = selected_version.as_deref().unwrap_or("none"),
            has_readme = readme.is_some(),
            "fetched readme"
        );

        let dependencies = if include_dependencies {
            if let Some(version) = selected_version.as_deref() {
                Some(
                    crates_io
                        .fetch_crate_dependencies(crate_name, version)
                        .await?,
                )
            } else {
                None
            }
        } else {
            None
        };

        let categories = dedupe_strings(
            detail
                .categories
                .iter()
                .map(|c| {
                    c.slug
                        .clone()
                        .or(c.category.clone())
                        .unwrap_or_else(|| c.id.clone())
                })
                .collect(),
        );

        let keywords = dedupe_strings(
            detail
                .keywords
                .iter()
                .map(|k| {
                    k.keyword
                        .clone()
                        .unwrap_or_else(|| k.id.clone())
                })
                .collect(),
        );

        let persisted = persist_crate_sync(
            &self.state.db,
            &detail,
            selected_version.as_deref(),
            readme.as_deref(),
            dependencies.as_deref(),
            &categories,
            &keywords,
        )
        .await
        .map_err(|e| format!("failed to persist sync for {crate_name}: {e}"))?;

        info!(
            %crate_name,
            selected_version = selected_version.as_deref().unwrap_or("none"),
            versions_synced = persisted.versions_synced,
            dependencies_synced = persisted.dependencies_synced,
            "crate sync persisted"
        );

        Ok(SyncCrateOutcome {
            versions_synced: persisted.versions_synced,
            dependencies_synced: persisted.dependencies_synced,
            selected_version,
        })
    }

    /// Enqueues a refresh job for the given crate and scope at the specified
    /// priority.
    pub async fn enqueue_refresh_job(
        &self,
        crate_name: &str,
        scope: &str,
        priority: i32,
        include_dependencies: bool,
        payload: Value,
    ) -> Result<String, String> {
        let job_id = enqueue_or_get_refresh_job_id(
            &self.state.db,
            crate_name,
            scope,
            priority,
            include_dependencies,
            payload,
        )
        .await
        .map_err(|e| format!("failed to enqueue refresh job for {crate_name}: {e}"))?;

        Ok(format!("refresh-job-{job_id}"))
    }

    /// Handles the `index.sync_crates` tool call.
    pub async fn handle_index_sync_crates(
        &self,
        request: IndexSyncCratesRequest,
    ) -> Result<Json<IndexSyncCratesResponse>, String> {
        let query =
            normalize_optional(request.query).unwrap_or_else(|| DEFAULT_SYNC_QUERY.to_string());
        let page = sync_page(request.page);
        let per_page = sync_per_page(request.per_page);
        let include_dependencies = request
            .include_dependencies
            .unwrap_or(true);

        let params = vec![
            ("q", query.clone()),
            ("page", page.to_string()),
            ("per_page", per_page.to_string()),
        ];
        let crates_io = CratesIoClient::new(&self.state);
        let search_response: CratesIoSearchResponse = crates_io
            .search_crates(&params)
            .await?;

        let mut synced_crates = 0_usize;
        let mut synced_versions = 0_usize;
        let mut synced_dependencies = 0_usize;
        let mut selected_versions = Vec::new();
        let mut errors = Vec::new();

        for item in search_response.crates {
            let crate_name = if item.id.trim().is_empty() {
                item.name.trim().to_string()
            } else {
                item.id.trim().to_string()
            };

            if crate_name.is_empty() {
                continue;
            }

            match self
                .sync_single_crate(&crate_name, include_dependencies)
                .await
            {
                Ok(outcome) => {
                    synced_crates += 1;
                    synced_versions += outcome.versions_synced;
                    synced_dependencies += outcome.dependencies_synced;
                    if let Some(version) = outcome.selected_version {
                        selected_versions.push(format!("{crate_name}@{version}"));
                    }
                }
                Err(error) => {
                    errors.push(format!("{crate_name}: {error}"));
                }
            }
        }

        Ok(Json(IndexSyncCratesResponse {
            query,
            page,
            per_page,
            total_candidates: search_response.meta.total,
            synced_crates,
            synced_versions,
            synced_dependencies,
            selected_versions,
            errors,
            freshness: vec![
                ResponseFreshnessSource {
                    source: "crates.io".to_string(),
                    status: "refreshed".to_string(),
                    checked_at: None,
                },
                ResponseFreshnessSource {
                    source: "local_postgres_index".to_string(),
                    status: "updated".to_string(),
                    checked_at: None,
                },
            ],
            provenance: "crates.io + local_postgres_index".to_string(),
        }))
    }

    /// Handles the `index.status` tool call.
    pub async fn handle_index_status(
        &self,
        _request: IndexStatusRequest,
    ) -> Result<Json<IndexStatusResponse>, String> {
        let coverage = fetch_index_coverage_counts(&self.state.db)
            .await
            .map_err(|e| format!("index.status failed to load coverage: {e}"))?;
        let queue = fetch_index_queue_counts(&self.state.db)
            .await
            .map_err(|e| format!("index.status failed to load queue counters: {e}"))?;
        let retry_distribution = fetch_refresh_job_retry_distribution(&self.state.db)
            .await
            .map_err(|e| format!("index.status failed to compute retry distribution: {e}"))?;
        let failures_by_scope = fetch_index_failures_by_scope(&self.state.db)
            .await
            .map_err(|e| format!("index.status failed to compute failure-by-scope: {e}"))?
            .into_iter()
            .map(|row| IndexFailureByScope {
                scope: row.scope,
                failed_jobs: row.failed_jobs,
            })
            .collect::<Vec<_>>();
        let last_errors = fetch_recent_refresh_job_errors(&self.state.db, 10)
            .await
            .map_err(|e| format!("index.status failed to fetch job errors: {e}"))?;
        let freshness = fetch_index_freshness(&self.state.db)
            .await
            .map_err(|e| format!("index.status failed to load freshness: {e}"))?;
        let operational = fetch_index_operational_metrics_24h(&self.state.db)
            .await
            .map_err(|e| format!("index.status failed to load operational metrics: {e}"))?;

        Ok(Json(IndexStatusResponse {
            freshness: IndexFreshness {
                crates_updated_at: freshness.crates_updated_at,
                source_indexed_at: freshness.source_indexed_at,
                symbols_indexed_at: freshness.symbols_indexed_at,
                docs_indexed_at: freshness.docs_indexed_at,
                advisories_updated_at: freshness.advisories_updated_at,
            },
            coverage: IndexCoverage {
                crates: coverage.crates,
                crate_versions: coverage.crate_versions,
                dependency_edges: coverage.dependency_edges,
                advisory_matches: coverage.advisory_matches,
                source_files: coverage.source_files,
                symbols: coverage.symbols,
                docs_pages: coverage.docs_pages,
            },
            operational_metrics: IndexOperationalMetrics {
                window: "24h".to_string(),
                query_count: operational.query_count,
                average_latency_ms: operational.average_latency_ms,
                error_rate: operational.error_rate,
                cache_hit_rate: operational.cache_hit_rate,
                index_lag_seconds: operational.index_lag_seconds,
            },
            queue: IndexQueue {
                pending_jobs: queue.pending_jobs,
                delayed_jobs: queue.delayed_jobs,
                retrying_jobs: queue.retrying_jobs,
                running_jobs: queue.running_jobs,
                failed_jobs: queue.failed_jobs,
            },
            retry_distribution: IndexRetryDistribution {
                inflight_attempt_1: retry_distribution.inflight_attempt_1,
                inflight_attempt_2: retry_distribution.inflight_attempt_2,
                inflight_attempt_3_plus: retry_distribution.inflight_attempt_3_plus,
                failed_attempt_1: retry_distribution.failed_attempt_1,
                failed_attempt_2: retry_distribution.failed_attempt_2,
                failed_attempt_3_plus: retry_distribution.failed_attempt_3_plus,
            },
            failures_by_scope,
            last_errors,
            provenance: "local_postgres_index".to_string(),
        }))
    }

    async fn estimated_refresh_duration_seconds(
        &self,
        scope: IndexRefreshScope,
    ) -> Result<Option<u32>, String> {
        let scope_key = match scope {
            IndexRefreshScope::Crate => "crate",
            IndexRefreshScope::All => "all",
            IndexRefreshScope::Security => "security",
            IndexRefreshScope::Docs => "docs",
            IndexRefreshScope::LocalCache => "local_cache",
            IndexRefreshScope::RustdocJson => "rustdoc_json",
        };

        let (sample_count, average_seconds) = fetch_refresh_eta_stats(&self.state.db, scope_key)
            .await
            .map_err(|e| format!("index.refresh ETA estimate failed for scope {scope_key}: {e}"))?;

        if sample_count < 3 {
            return Ok(None);
        }

        let Some(average_seconds) = average_seconds else {
            return Ok(None);
        };

        let clamped = average_seconds
            .round()
            .clamp(1.0, 86_400.0);
        Ok(Some(clamped as u32))
    }

    /// Handles the `index.refresh` tool call.
    pub async fn handle_index_refresh(
        &self,
        request: IndexRefreshRequest,
    ) -> Result<Json<IndexRefreshResponse>, String> {
        let scope = request
            .scope
            .unwrap_or(IndexRefreshScope::Crate);
        let started_at_epoch_ms = now_epoch_millis();
        let job_id = format!("job-{started_at_epoch_ms}");
        let estimated_seconds = self
            .estimated_refresh_duration_seconds(scope)
            .await?;

        match scope {
            IndexRefreshScope::Crate => {
                let crate_name = normalize_required(
                    normalize_optional(request.crate_name).unwrap_or_default(),
                    "crate_name",
                )?;

                match self
                    .sync_single_crate(
                        &crate_name,
                        request
                            .include_dependencies
                            .unwrap_or(true),
                    )
                    .await
                {
                    Ok(outcome) => Ok(Json(IndexRefreshResponse {
                        job_id,
                        scope,
                        accepted: true,
                        status: "completed".to_string(),
                        message: format!("refreshed {crate_name}"),
                        estimated_seconds,
                        estimated_seconds_remaining: Some(0),
                        started_at_epoch_ms,
                        finished_at_epoch_ms: Some(now_epoch_millis()),
                        result: Some(IndexRefreshResult {
                            synced_crates: 1,
                            synced_versions: outcome.versions_synced,
                            synced_dependencies: outcome.dependencies_synced,
                            selected_versions: outcome
                                .selected_version
                                .into_iter()
                                .map(|v| format!("{crate_name}@{v}"))
                                .collect(),
                            errors: Vec::new(),
                            synced_types: None,
                            synced_impls: None,
                            synced_traits: None,
                        }),
                        freshness: vec![
                            ResponseFreshnessSource {
                                source: "crates.io".to_string(),
                                status: "refreshed".to_string(),
                                checked_at: None,
                            },
                            ResponseFreshnessSource {
                                source: "local_postgres_index".to_string(),
                                status: "updated".to_string(),
                                checked_at: None,
                            },
                        ],
                        provenance: "crates.io + local_postgres_index".to_string(),
                    })),
                    Err(error) => Ok(Json(IndexRefreshResponse {
                        job_id,
                        scope,
                        accepted: true,
                        status: "failed".to_string(),
                        message: format!("refresh failed for {crate_name}"),
                        estimated_seconds,
                        estimated_seconds_remaining: Some(0),
                        started_at_epoch_ms,
                        finished_at_epoch_ms: Some(now_epoch_millis()),
                        result: Some(IndexRefreshResult {
                            synced_crates: 0,
                            synced_versions: 0,
                            synced_dependencies: 0,
                            selected_versions: Vec::new(),
                            errors: vec![error],
                            synced_types: None,
                            synced_impls: None,
                            synced_traits: None,
                        }),
                        freshness: vec![
                            ResponseFreshnessSource {
                                source: "crates.io".to_string(),
                                status: "failed".to_string(),
                                checked_at: None,
                            },
                            ResponseFreshnessSource {
                                source: "local_postgres_index".to_string(),
                                status: "unchanged".to_string(),
                                checked_at: None,
                            },
                        ],
                        provenance: "crates.io + local_postgres_index".to_string(),
                    })),
                }
            }
            IndexRefreshScope::All => {
                let Json(sync_response) = self
                    .handle_index_sync_crates(IndexSyncCratesRequest {
                        query: request.query,
                        page: request.page,
                        per_page: request.per_page,
                        include_dependencies: request.include_dependencies,
                    })
                    .await?;

                Ok(Json(IndexRefreshResponse {
                    job_id,
                    scope,
                    accepted: true,
                    status: "completed".to_string(),
                    message: "completed sync over search page".to_string(),
                    estimated_seconds,
                    estimated_seconds_remaining: Some(0),
                    started_at_epoch_ms,
                    finished_at_epoch_ms: Some(now_epoch_millis()),
                    result: Some(IndexRefreshResult {
                        synced_crates: sync_response.synced_crates,
                        synced_versions: sync_response.synced_versions,
                        synced_dependencies: sync_response.synced_dependencies,
                        selected_versions: sync_response.selected_versions,
                        errors: sync_response.errors,
                        synced_types: None,
                        synced_impls: None,
                        synced_traits: None,
                    }),
                    freshness: sync_response.freshness,
                    provenance: sync_response.provenance,
                }))
            }
            IndexRefreshScope::Security => {
                let page = sync_page(request.page);
                let per_page = sync_per_page(request.per_page);
                let offset = page.saturating_sub(1) * per_page;
                let mut outcome = self
                    .sync_osv_security(per_page, offset)
                    .await?;
                let rustsec_outcome = self
                    .sync_rustsec_db_security(per_page, offset)
                    .await?;
                let rustsec_enabled = self
                    .state
                    .config
                    .rustsec_db_dir
                    .is_some();
                outcome.merge(rustsec_outcome);

                Ok(Json(IndexRefreshResponse {
                    job_id,
                    scope,
                    accepted: true,
                    status: if outcome.errors.is_empty() {
                        "completed".to_string()
                    } else {
                        "completed_with_errors".to_string()
                    },
                    message: format!(
                        "security sync processed {} crates and wrote {} advisory matches",
                        outcome.crates_processed, outcome.advisories_written
                    ),
                    estimated_seconds,
                    estimated_seconds_remaining: Some(0),
                    started_at_epoch_ms,
                    finished_at_epoch_ms: Some(now_epoch_millis()),
                    result: Some(IndexRefreshResult {
                        synced_crates: outcome.crates_processed,
                        synced_versions: outcome.advisories_written,
                        synced_dependencies: 0,
                        selected_versions: outcome.touched_crates,
                        errors: outcome.errors,
                        synced_types: None,
                        synced_impls: None,
                        synced_traits: None,
                    }),
                    freshness: vec![
                        ResponseFreshnessSource {
                            source: "osv.dev".to_string(),
                            status: "refreshed".to_string(),
                            checked_at: None,
                        },
                        ResponseFreshnessSource {
                            source: "rustsec-db".to_string(),
                            status: if rustsec_enabled {
                                "refreshed".to_string()
                            } else {
                                "skipped".to_string()
                            },
                            checked_at: None,
                        },
                        ResponseFreshnessSource {
                            source: "local_postgres_index".to_string(),
                            status: "updated".to_string(),
                            checked_at: None,
                        },
                    ],
                    provenance: if rustsec_enabled {
                        "osv.dev + rustsec-db + local_postgres_index".to_string()
                    } else {
                        "osv.dev + local_postgres_index".to_string()
                    },
                }))
            }
            IndexRefreshScope::LocalCache => {
                let outcome = self
                    .sync_local_source_cache(
                        request.crate_name,
                        request.query,
                        request.page,
                        request.per_page,
                    )
                    .await?;

                Ok(Json(IndexRefreshResponse {
                    job_id,
                    scope,
                    accepted: true,
                    status: if outcome.errors.is_empty() {
                        "completed".to_string()
                    } else {
                        "completed_with_errors".to_string()
                    },
                    message: format!(
                        "scanned {} crate versions ({} files), upserted {}, pruned {}",
                        outcome.scanned_versions,
                        outcome.scanned_files,
                        outcome.upserted_files,
                        outcome.deleted_files
                    ),
                    estimated_seconds,
                    estimated_seconds_remaining: Some(0),
                    started_at_epoch_ms,
                    finished_at_epoch_ms: Some(now_epoch_millis()),
                    result: Some(IndexRefreshResult {
                        synced_crates: outcome.scanned_versions,
                        synced_versions: outcome.upserted_files,
                        synced_dependencies: outcome.deleted_files,
                        selected_versions: outcome.touched_versions,
                        errors: outcome.errors,
                        synced_types: None,
                        synced_impls: None,
                        synced_traits: None,
                    }),
                    freshness: vec![
                        ResponseFreshnessSource {
                            source: "cargo_registry".to_string(),
                            status: "scanned".to_string(),
                            checked_at: None,
                        },
                        ResponseFreshnessSource {
                            source: "local_postgres_index".to_string(),
                            status: "updated".to_string(),
                            checked_at: None,
                        },
                    ],
                    provenance: "cargo_registry + local_postgres_index".to_string(),
                }))
            }
            IndexRefreshScope::Docs => {
                let outcome = self
                    .sync_docs_pages(request.crate_name, request.page, request.per_page)
                    .await?;

                Ok(Json(IndexRefreshResponse {
                    job_id,
                    scope,
                    accepted: true,
                    status: if outcome.errors.is_empty() {
                        "completed".to_string()
                    } else {
                        "completed_with_errors".to_string()
                    },
                    message: format!(
                        "docs sync processed {} crate versions and wrote {} docs pages",
                        outcome.versions_processed, outcome.pages_written
                    ),
                    estimated_seconds,
                    estimated_seconds_remaining: Some(0),
                    started_at_epoch_ms,
                    finished_at_epoch_ms: Some(now_epoch_millis()),
                    result: Some(IndexRefreshResult {
                        synced_crates: outcome.versions_processed,
                        synced_versions: outcome.pages_written,
                        synced_dependencies: 0,
                        selected_versions: outcome.touched_versions,
                        errors: outcome.errors,
                        synced_types: None,
                        synced_impls: None,
                        synced_traits: None,
                    }),
                    freshness: vec![
                        ResponseFreshnessSource {
                            source: "docs.rs".to_string(),
                            status: "refreshed".to_string(),
                            checked_at: None,
                        },
                        ResponseFreshnessSource {
                            source: "local_postgres_index".to_string(),
                            status: "updated".to_string(),
                            checked_at: None,
                        },
                    ],
                    provenance: "docs.rs + local_postgres_index".to_string(),
                }))
            }
            IndexRefreshScope::RustdocJson => {
                let outcome = self
                    .sync_rustdoc_json_cache(request.crate_name, request.page, request.per_page)
                    .await?;

                Ok(Json(IndexRefreshResponse {
                    job_id,
                    scope,
                    accepted: true,
                    status: if outcome.errors.is_empty() {
                        "completed".to_string()
                    } else {
                        "completed_with_errors".to_string()
                    },
                    message: format!(
                        "rustdoc JSON sync scanned {} files, synced {} crate versions, wrote {} \
                         symbols, {} types, {} impls, {} traits",
                        outcome.scanned_files,
                        outcome.synced_versions,
                        outcome.symbols_written,
                        outcome.types_written,
                        outcome.impls_written,
                        outcome.traits_written,
                    ),
                    estimated_seconds,
                    estimated_seconds_remaining: Some(0),
                    started_at_epoch_ms,
                    finished_at_epoch_ms: Some(now_epoch_millis()),
                    result: Some(IndexRefreshResult {
                        synced_crates: outcome.scanned_files,
                        synced_versions: outcome.synced_versions,
                        synced_dependencies: outcome.symbols_written,
                        selected_versions: outcome.touched_versions,
                        errors: outcome.errors,
                        synced_types: Some(outcome.types_written),
                        synced_impls: Some(outcome.impls_written),
                        synced_traits: Some(outcome.traits_written),
                    }),
                    freshness: vec![
                        ResponseFreshnessSource {
                            source: "rustdoc_json".to_string(),
                            status: "scanned".to_string(),
                            checked_at: None,
                        },
                        ResponseFreshnessSource {
                            source: "local_postgres_index".to_string(),
                            status: "updated".to_string(),
                            checked_at: None,
                        },
                    ],
                    provenance: "rustdoc_json + local_postgres_index".to_string(),
                }))
            }
        }
    }
}
