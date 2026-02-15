use std::collections::BTreeSet;

use rmcp::{Json, schemars};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::mcp::models::{
    ConfidenceAssessment, ConfidenceLevel, CrateCoreRow, CrateVersionSelectionRow,
    ResponseFreshnessSource,
};
use crate::mcp::server::McpServer;
use crate::mcp::utils::{
    crate_api_limit, normalize_optional, normalize_required, path_glob_to_like,
};

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CrateApiRequest {
    pub crate_name: String,
    pub version: Option<String>,
    pub path_glob: Option<String>,
    pub kinds: Option<Vec<String>>,
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateApiResponse {
    pub crate_name: String,
    pub selected_version: String,
    pub latest_version: String,
    pub path_glob: Option<String>,
    pub kinds: Vec<String>,
    pub limit: u32,
    pub count: usize,
    pub symbols: Vec<CrateApiSymbol>,
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
pub struct CrateApiSymbol {
    pub name: String,
    pub kind: String,
    pub signature: Option<String>,
    pub visibility: Option<String>,
    pub source_path: String,
    pub start_line: i32,
    pub end_line: i32,
    pub index_source: String,
}

#[derive(Debug, FromRow)]
pub(crate) struct ApiSurfaceRow {
    pub(crate) name: String,
    pub(crate) kind: String,
    pub(crate) signature: Option<String>,
    pub(crate) visibility: Option<String>,
    pub(crate) source_path: String,
    pub(crate) start_line: i32,
    pub(crate) end_line: i32,
    pub(crate) index_source: String,
}

fn normalize_kind_filters(input: Option<Vec<String>>) -> Vec<String> {
    let mut out = input
        .unwrap_or_default()
        .into_iter()
        .map(|value| {
            value
                .trim()
                .to_ascii_lowercase()
        })
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    out.sort();
    out.dedup();
    out
}

fn allowed_kind_filters(values: Vec<String>) -> Vec<String> {
    let allowed =
        ["function", "struct", "enum", "trait", "type_alias", "const", "static", "module"]
            .into_iter()
            .map(ToString::to_string)
            .collect::<BTreeSet<_>>();

    values
        .into_iter()
        .filter(|value| allowed.contains(value))
        .collect()
}

impl McpServer {
    pub(crate) async fn handle_crate_api(
        &self,
        request: CrateApiRequest,
    ) -> Result<Json<CrateApiResponse>, String> {
        let crate_name = normalize_required(request.crate_name, "crate_name")?;
        let requested_version = normalize_optional(request.version);
        let path_glob = normalize_optional(request.path_glob);
        let kinds = allowed_kind_filters(normalize_kind_filters(request.kinds));
        let limit = crate_api_limit(request.limit);

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

        let has_rustdoc_symbols = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*)::BIGINT
             FROM symbols
             WHERE crate_version_id = $1
               AND visibility = 'public'
               AND index_source = 'rustdoc_json'",
        )
        .bind(selected_version.id)
        .fetch_one(&self.state.db)
        .await
        .map_err(|e| format!("crate.api rustdoc source check failed: {e}"))?
            > 0;
        let preferred_source = if has_rustdoc_symbols { Some("rustdoc_json") } else { None };

        let rows = if let Some(path_filter) = path_glob.as_deref() {
            sqlx::query_as::<_, ApiSurfaceRow>(
                "SELECT
                    s.name,
                    s.kind,
                    s.signature,
                    s.visibility,
                    sf.path AS source_path,
                    s.start_line,
                    s.end_line,
                    s.index_source
                 FROM symbols s
                 JOIN source_files sf ON sf.id = s.source_file_id
                 WHERE s.crate_version_id = $1
                   AND s.visibility = 'public'
                   AND ($2::TEXT[] IS NULL OR s.kind = ANY($2))
                                     AND ($3::TEXT IS NULL OR s.index_source = $3)
                                     AND sf.path ILIKE $4 ESCAPE '\\'
                 ORDER BY
                    s.kind ASC,
                    s.name ASC,
                    sf.path ASC,
                    s.start_line ASC,
                    s.end_line ASC
                                 LIMIT $5",
            )
            .bind(selected_version.id)
            .bind(if kinds.is_empty() { None } else { Some(kinds.as_slice()) })
            .bind(preferred_source)
            .bind(path_glob_to_like(path_filter))
            .bind(i64::from(limit))
            .fetch_all(&self.state.db)
            .await
            .map_err(|e| format!("crate.api query failed: {e}"))?
        } else {
            sqlx::query_as::<_, ApiSurfaceRow>(
                "SELECT
                    s.name,
                    s.kind,
                    s.signature,
                    s.visibility,
                    sf.path AS source_path,
                    s.start_line,
                    s.end_line,
                    s.index_source
                 FROM symbols s
                 JOIN source_files sf ON sf.id = s.source_file_id
                 WHERE s.crate_version_id = $1
                   AND s.visibility = 'public'
                   AND ($2::TEXT[] IS NULL OR s.kind = ANY($2))
                                     AND ($3::TEXT IS NULL OR s.index_source = $3)
                 ORDER BY
                    s.kind ASC,
                    s.name ASC,
                    sf.path ASC,
                    s.start_line ASC,
                    s.end_line ASC
                                 LIMIT $4",
            )
            .bind(selected_version.id)
            .bind(if kinds.is_empty() { None } else { Some(kinds.as_slice()) })
            .bind(preferred_source)
            .bind(i64::from(limit))
            .fetch_all(&self.state.db)
            .await
            .map_err(|e| format!("crate.api query failed: {e}"))?
        };

        let symbols = rows
            .into_iter()
            .map(|row| CrateApiSymbol {
                name: row.name,
                kind: row.kind,
                signature: row.signature,
                visibility: row.visibility,
                source_path: row.source_path,
                start_line: row.start_line,
                end_line: row.end_line,
                index_source: row.index_source,
            })
            .collect::<Vec<_>>();

        let confidence_assessment = if symbols.is_empty() {
            ConfidenceAssessment {
                level: ConfidenceLevel::Low,
                reason: "no public symbols matched the selected version/filter constraints"
                    .to_string(),
            }
        } else if symbols
            .iter()
            .any(|symbol| symbol.signature.is_none())
        {
            ConfidenceAssessment {
                level: ConfidenceLevel::Medium,
                reason: "some public symbols are missing extracted signatures".to_string(),
            }
        } else {
            ConfidenceAssessment {
                level: ConfidenceLevel::High,
                reason: if has_rustdoc_symbols {
                    "public symbol surface resolved from rustdoc_json indexed symbols".to_string()
                } else {
                    "public symbol surface resolved from indexed symbols".to_string()
                },
            }
        };

        let freshness_check_result = freshness_outcome
            .freshness_check_result
            .clone();

        Ok(Json(CrateApiResponse {
            crate_name: crate_row.name,
            selected_version: selected_version.version,
            latest_version: latest_version.version,
            path_glob,
            kinds,
            limit,
            count: symbols.len(),
            symbols,
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
                "crate.api_diff".to_string(),
                "symbol.search".to_string(),
                "source.read".to_string(),
            ],
            provenance: "local_postgres_index".to_string(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::{allowed_kind_filters, normalize_kind_filters};

    #[test]
    fn normalize_kind_filters_dedupes_and_lowers() {
        let values = normalize_kind_filters(Some(vec![
            " Function ".to_string(),
            "trait".to_string(),
            "function".to_string(),
        ]));
        assert_eq!(values, vec!["function".to_string(), "trait".to_string()]);
    }

    #[test]
    fn allowed_kind_filters_removes_unknown_values() {
        let values = allowed_kind_filters(vec![
            "function".to_string(),
            "macro".to_string(),
            "module".to_string(),
        ]);
        assert_eq!(values, vec!["function".to_string(), "module".to_string()]);
    }
}
