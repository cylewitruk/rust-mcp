use rmcp::Json;
pub use rust_mcp_types::types::krate::{
    CrateImportPathMatch, CrateImportPathRequest, CrateImportPathResponse,
};
use sqlx::FromRow;

use crate::db::models::{CrateCoreRow, CrateVersionSelectionRow};
use crate::mcp::models::{ConfidenceAssessment, ConfidenceLevel, ResponseFreshnessSource};
use crate::mcp::server::McpServer;
use crate::mcp::utils::{import_path_limit, normalize_optional, normalize_required};

#[derive(Debug, Clone, FromRow)]
struct ImportPathRow {
    symbol_name: String,
    kind: String,
    canonical_path: Option<String>,
    definition_path: Option<String>,
    source_path: String,
    start_line: i32,
    end_line: i32,
    index_source: String,
}

fn normalized_import_path(crate_name: &str, row: &ImportPathRow) -> String {
    row.canonical_path
        .clone()
        .filter(|path| !path.trim().is_empty())
        .unwrap_or_else(|| format!("{crate_name}::{}", row.symbol_name))
}

impl McpServer {
    pub(crate) async fn handle_crate_import_path(
        &self,
        request: CrateImportPathRequest,
    ) -> Result<Json<CrateImportPathResponse>, String> {
        let crate_name = normalize_required(request.crate_name, "crate_name")?;
        let symbol_name = normalize_required(request.symbol_name, "symbol_name")?;
        let requested_version = normalize_optional(request.version);
        let kind = normalize_optional(request.kind);
        let limit = import_path_limit(request.limit);

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

        let symbol_rows = sqlx::query_as::<_, ImportPathRow>(
            "SELECT
                s.name AS symbol_name,
                s.kind,
                s.canonical_path,
                s.definition_path,
                sf.path AS source_path,
                s.start_line,
                s.end_line,
                s.index_source
             FROM symbols s
             JOIN source_files sf ON sf.id = s.source_file_id
             WHERE s.crate_version_id = $1
               AND (s.visibility = 'public' OR s.visibility IS NULL)
               AND (
                    LOWER(s.name) = LOWER($2)
                    OR LOWER(COALESCE(s.canonical_path, '')) = LOWER($2)
                    OR LOWER(COALESCE(s.canonical_path, '')) LIKE ('%::' || LOWER($2))
               )
               AND ($3::TEXT IS NULL OR LOWER(s.kind) = LOWER($3))
             ORDER BY
                CASE
                    WHEN LOWER(s.name) = LOWER($2) THEN 0
                    WHEN LOWER(COALESCE(s.canonical_path, '')) = LOWER($2) THEN 1
                    WHEN LOWER(COALESCE(s.canonical_path, '')) LIKE ('%::' || LOWER($2)) THEN 2
                    ELSE 3
                END,
                CASE WHEN s.canonical_path IS NULL THEN 1 ELSE 0 END,
                CASE WHEN s.index_source = 'rustdoc_json' THEN 0 ELSE 1 END,
                s.start_line ASC,
                s.id ASC
             LIMIT $4",
        )
        .bind(selected_version.id)
        .bind(&symbol_name)
        .bind(kind.as_deref())
        .bind(i64::from(limit))
        .fetch_all(&self.state.db)
        .await
        .map_err(|e| format!("crate.import_path query failed: {e}"))?;

        let matches = symbol_rows
            .iter()
            .map(|row| CrateImportPathMatch {
                symbol_name: row.symbol_name.clone(),
                kind: row.kind.clone(),
                import_path: normalized_import_path(&crate_row.name, row),
                definition_path: row.definition_path.clone(),
                source_path: row.source_path.clone(),
                start_line: row.start_line.max(1) as u32,
                end_line: row.end_line.max(1) as u32,
                index_source: row.index_source.clone(),
                exact_name_match: row
                    .symbol_name
                    .eq_ignore_ascii_case(&symbol_name),
            })
            .collect::<Vec<_>>();

        let best_import_path = matches
            .first()
            .map(|entry| entry.import_path.clone());

        let confidence_assessment = if matches.is_empty() {
            ConfidenceAssessment {
                level: ConfidenceLevel::Low,
                reason: "no matching public symbols were found in indexed local cache data"
                    .to_string(),
            }
        } else if matches
            .first()
            .is_some_and(|entry| entry.index_source == "rustdoc_json" && entry.exact_name_match)
        {
            ConfidenceAssessment {
                level: ConfidenceLevel::High,
                reason: "best import path resolved from rustdoc canonical symbol data".to_string(),
            }
        } else {
            ConfidenceAssessment {
                level: ConfidenceLevel::Medium,
                reason: "import path resolved from indexed symbols without rustdoc canonical \
                         priority"
                    .to_string(),
            }
        };

        let freshness_check_result = freshness_outcome
            .freshness_check_result
            .clone();

        Ok(Json(CrateImportPathResponse {
            crate_name: crate_row.name,
            selected_version: selected_version.version,
            latest_version: latest_version.version,
            symbol_name,
            kind,
            limit,
            count: matches.len(),
            best_import_path,
            matches,
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
                "crate.re_exports".to_string(),
                "crate.type_info".to_string(),
                "source.search".to_string(),
            ],
            provenance: "local_postgres_index(symbols, source_files)".to_string(),
        }))
    }
}
