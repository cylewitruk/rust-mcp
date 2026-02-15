use rmcp::{Json, schemars};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::mcp::models::{
    ConfidenceAssessment, ConfidenceLevel, CrateCoreRow, CrateVersionSelectionRow,
    ResponseFreshnessSource,
};
use crate::mcp::server::McpServer;
use crate::mcp::utils::{normalize_optional, normalize_required, usage_patterns_limit};

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CrateUsagePatternsRequest {
    pub crate_name: String,
    pub symbol_name: String,
    pub version: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateUsagePatternsResponse {
    pub crate_name: String,
    pub selected_version: String,
    pub latest_version: String,
    pub symbol_name: String,
    pub limit: u32,
    pub count: usize,
    pub patterns: Vec<CrateUsagePattern>,
    pub freshness_check_performed: bool,
    pub freshness_check_result: String,
    pub refresh_enqueued: bool,
    pub refresh_job_id: Option<String>,
    pub freshness: Vec<ResponseFreshnessSource>,
    pub confidence: String,
    pub confidence_assessment: ConfidenceAssessment,
    pub next_best_calls: Vec<String>,
    pub provenance: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateUsagePattern {
    pub dependent_crate: String,
    pub dependent_version: String,
    pub dependent_downloads: i64,
    pub path: String,
    pub line_start: u32,
    pub line_end: u32,
    pub snippet: String,
}

#[derive(Debug, Clone, FromRow)]
pub(crate) struct CrateUsageSourceRow {
    pub(crate) dependent_crate: String,
    pub(crate) dependent_version: String,
    pub(crate) dependent_downloads: i64,
    pub(crate) path: String,
    pub(crate) content: String,
}

fn extract_usage_snippet(content: &str, symbol_name: &str) -> (u32, u32, String) {
    let lines = content
        .lines()
        .collect::<Vec<_>>();
    if lines.is_empty() {
        return (1, 1, String::new());
    }

    let needle = symbol_name.to_ascii_lowercase();
    let line_index = lines
        .iter()
        .position(|line| {
            line.to_ascii_lowercase()
                .contains(&needle)
        })
        .unwrap_or(0);

    let line_number = (line_index + 1) as u32;
    let start = line_index.saturating_sub(1);
    let end = (line_index + 1).min(lines.len() - 1);

    let snippet = lines[start..=end]
        .join("\n")
        .chars()
        .take(300)
        .collect::<String>();

    (line_number, line_number, snippet)
}

impl McpServer {
    pub(crate) async fn handle_crate_usage_patterns(
        &self,
        request: CrateUsagePatternsRequest,
    ) -> Result<Json<CrateUsagePatternsResponse>, String> {
        let crate_name = normalize_required(request.crate_name, "crate_name")?;
        let symbol_name = normalize_required(request.symbol_name, "symbol_name")?;
        let requested_version = normalize_optional(request.version);
        let limit = usage_patterns_limit(request.limit);

        let crate_row = sqlx::query_as::<_, CrateCoreRow>(
            "SELECT
                id,
                name,
                description,
                repository_url,
                docs_url,
                homepage_url,
                categories,
                keywords,
                updated_at::TEXT AS updated_at
             FROM crates
             WHERE name = $1",
        )
        .bind(&crate_name)
        .fetch_optional(&self.state.db)
        .await
        .map_err(|e| format!("crate lookup failed for {crate_name}: {e}"))?
        .ok_or_else(|| {
            format!("crate '{crate_name}' is not indexed locally; run index.sync_crates first")
        })?;

        let latest_version = sqlx::query_as::<_, CrateVersionSelectionRow>(
            "SELECT
                id,
                version,
                rust_version,
                published_at::TEXT AS published_at,
                readme
             FROM crate_versions
             WHERE crate_id = $1
             ORDER BY published_at DESC NULLS LAST, id DESC
             LIMIT 1",
        )
        .bind(crate_row.id)
        .fetch_optional(&self.state.db)
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
            sqlx::query_as::<_, CrateVersionSelectionRow>(
                "SELECT
                    id,
                    version,
                    rust_version,
                    published_at::TEXT AS published_at,
                    readme
                 FROM crate_versions
                 WHERE crate_id = $1
                 ORDER BY published_at DESC NULLS LAST, id DESC
                 LIMIT 1",
            )
            .bind(crate_row.id)
            .fetch_optional(&self.state.db)
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

        let mut refresh_enqueued = freshness_outcome.refresh_enqueued;
        let mut refresh_job_id = freshness_outcome
            .refresh_job_id
            .clone();

        let selected_version = if let Some(version) = requested_version {
            let selected = sqlx::query_as::<_, CrateVersionSelectionRow>(
                "SELECT
                    id,
                    version,
                    rust_version,
                    published_at::TEXT AS published_at,
                    readme
                 FROM crate_versions
                 WHERE crate_id = $1 AND version = $2
                 LIMIT 1",
            )
            .bind(crate_row.id)
            .bind(&version)
            .fetch_optional(&self.state.db)
            .await
            .map_err(|e| {
                format!("selected version lookup failed for {}@{}: {e}", crate_row.name, version)
            })?;

            if let Some(selected) = selected {
                selected
            } else {
                let queued_job_id = self
                    .backfill_missing_requested_version(&crate_row.name)
                    .await?;
                if let Some(job_id) = queued_job_id {
                    refresh_enqueued = true;
                    refresh_job_id = Some(job_id);
                }

                sqlx::query_as::<_, CrateVersionSelectionRow>(
                    "SELECT
                        id,
                        version,
                        rust_version,
                        published_at::TEXT AS published_at,
                        readme
                     FROM crate_versions
                     WHERE crate_id = $1 AND version = $2
                     LIMIT 1",
                )
                .bind(crate_row.id)
                .bind(&version)
                .fetch_optional(&self.state.db)
                .await
                .map_err(|e| {
                    format!(
                        "selected version lookup failed after backfill for {}@{}: {e}",
                        crate_row.name, version
                    )
                })?
                .ok_or_else(|| {
                    format!(
                        "version '{}' for crate '{}' is not indexed locally (refresh attempted)",
                        version, crate_row.name
                    )
                })?
            }
        } else {
            latest_version.clone()
        };

        let symbol_filter = format!("%{}%", symbol_name);

        let rows = sqlx::query_as::<_, CrateUsageSourceRow>(
            "WITH dependents AS (
                SELECT DISTINCT ON (dc.id)
                    dc.id AS dependent_crate_id,
                    dc.name AS dependent_crate_name,
                    dcv.id AS dependent_version_id,
                    dcv.version AS dependent_version,
                    dcv.total_downloads AS dependent_downloads
                FROM dependency_edges de
                JOIN crate_versions dcv ON dcv.id = de.from_version_id
                JOIN crates dc ON dc.id = dcv.crate_id
                WHERE de.to_crate_id = $1
                ORDER BY dc.id, dcv.published_at DESC NULLS LAST, dcv.id DESC
            )
            SELECT
                d.dependent_crate_name AS dependent_crate,
                d.dependent_version,
                d.dependent_downloads,
                sf.path,
                sf.content
            FROM dependents d
            JOIN source_files sf ON sf.crate_version_id = d.dependent_version_id
            WHERE sf.content ILIKE $2
            ORDER BY d.dependent_downloads DESC, d.dependent_crate_name ASC, sf.path ASC
            LIMIT $3",
        )
        .bind(crate_row.id)
        .bind(&symbol_filter)
        .bind(i64::from(limit))
        .fetch_all(&self.state.db)
        .await
        .map_err(|e| format!("crate.usage_patterns query failed: {e}"))?;

        let patterns = rows
            .into_iter()
            .map(|row| {
                let (line_start, line_end, snippet) =
                    extract_usage_snippet(&row.content, &symbol_name);
                CrateUsagePattern {
                    dependent_crate: row.dependent_crate,
                    dependent_version: row.dependent_version,
                    dependent_downloads: row.dependent_downloads,
                    path: row.path,
                    line_start,
                    line_end,
                    snippet,
                }
            })
            .collect::<Vec<_>>();

        let confidence_assessment = if patterns.is_empty() {
            ConfidenceAssessment {
                level: ConfidenceLevel::Low,
                reason: "no dependent source snippets matched the requested symbol in local index"
                    .to_string(),
            }
        } else {
            ConfidenceAssessment {
                level: ConfidenceLevel::High,
                reason: "usage snippets were resolved from indexed dependent crate source files"
                    .to_string(),
            }
        };

        let freshness_check_result = freshness_outcome
            .freshness_check_result
            .clone();

        Ok(Json(CrateUsagePatternsResponse {
            crate_name: crate_row.name,
            selected_version: selected_version.version,
            latest_version: latest_version.version,
            symbol_name,
            limit,
            count: patterns.len(),
            patterns,
            freshness_check_performed: freshness_outcome.freshness_check_performed,
            freshness_check_result: freshness_check_result.clone(),
            refresh_enqueued,
            refresh_job_id,
            freshness: vec![
                ResponseFreshnessSource {
                    source: "local_postgres_index".to_string(),
                    status: "fresh".to_string(),
                    checked_at: crate_row.updated_at,
                },
                ResponseFreshnessSource {
                    source: "crates.io".to_string(),
                    status: freshness_check_result,
                    checked_at: None,
                },
            ],
            confidence: confidence_assessment
                .level
                .as_str()
                .to_string(),
            confidence_assessment,
            next_best_calls: vec![
                "source.read".to_string(),
                "crate.type_info".to_string(),
                "crate.api".to_string(),
            ],
            provenance: "local_postgres_index(dependency_edges, crate_versions, source_files)"
                .to_string(),
        }))
    }
}
