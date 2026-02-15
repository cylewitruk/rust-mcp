use rmcp::{Json, schemars};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::mcp::models::{
    ConfidenceAssessment, ConfidenceLevel, CrateCoreRow, CrateVersionSelectionRow,
    ResponseFreshnessSource,
};
use crate::mcp::server::McpServer;
use crate::mcp::utils::{normalize_optional, normalize_required, re_exports_limit};

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CrateReExportsRequest {
    pub crate_name: String,
    pub version: Option<String>,
    pub path_prefix: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateReExportsResponse {
    pub crate_name: String,
    pub selected_version: String,
    pub latest_version: String,
    pub path_prefix: Option<String>,
    pub limit: u32,
    pub count: usize,
    pub re_exports: Vec<CrateReExportEntry>,
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
pub struct CrateReExportEntry {
    pub canonical_path: String,
    pub original_definition_path: String,
    pub kind: String,
    pub visibility: String,
    pub shortest_public_path: bool,
    pub source_path: String,
    pub source_line: u32,
}

#[derive(Debug, Clone, FromRow)]
pub(crate) struct ReExportSourceRow {
    pub(crate) path: String,
    pub(crate) content: String,
}

#[derive(Debug, Clone, FromRow)]
pub(crate) struct ReExportSymbolKindRow {
    pub(crate) kind: String,
    pub(crate) visibility: Option<String>,
}

#[derive(Debug)]
struct ParsedReExport {
    target_path: String,
    exported_name: String,
    source_line: u32,
}

fn module_prefix_from_path(crate_name: &str, path: &str) -> String {
    let path = path.trim_start_matches("./");
    if path == "src/lib.rs" {
        return crate_name.to_string();
    }

    let mut parts = path
        .trim_start_matches("src/")
        .split('/')
        .collect::<Vec<_>>();

    if parts.is_empty() {
        return crate_name.to_string();
    }

    if parts.last() == Some(&"mod.rs") {
        let _ = parts.pop();
    } else if let Some(last) = parts.last_mut()
        && last.ends_with(".rs")
    {
        *last = &last[..last.len().saturating_sub(3)];
    }

    let filtered = parts
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();

    if filtered.is_empty() {
        crate_name.to_string()
    } else {
        format!("{}::{}", crate_name, filtered.join("::"))
    }
}

fn split_alias(entry: &str) -> Option<(String, String)> {
    let trimmed = entry.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Some((left, right)) = trimmed.split_once(" as ") {
        let target = left.trim();
        let alias = right.trim();
        if !target.is_empty() && !alias.is_empty() {
            return Some((target.to_string(), alias.to_string()));
        }
    }

    let exported_name = trimmed
        .split("::")
        .last()
        .unwrap_or(trimmed)
        .trim()
        .to_string();

    Some((trimmed.to_string(), exported_name))
}

fn parse_pub_use_statement(statement: &str, line_no: u32) -> Vec<ParsedReExport> {
    let mut normalized = statement.trim();
    if let Some((left, _)) = normalized.split_once("//") {
        normalized = left.trim();
    }

    if !normalized.starts_with("pub use ") {
        return Vec::new();
    }

    let mut body = normalized
        .trim_start_matches("pub use ")
        .trim();
    body = body
        .trim_end_matches(';')
        .trim();

    if body.is_empty() {
        return Vec::new();
    }

    if let Some(open_brace) = body.find('{')
        && let Some(close_brace) = body.rfind('}')
        && close_brace > open_brace
    {
        let prefix = body[..open_brace]
            .trim()
            .trim_end_matches("::")
            .trim();
        let inner = body[open_brace + 1..close_brace].trim();

        return inner
            .split(',')
            .filter_map(split_alias)
            .map(|(target, exported_name)| ParsedReExport {
                target_path: if prefix.is_empty() { target } else { format!("{prefix}::{target}") },
                exported_name,
                source_line: line_no,
            })
            .collect();
    }

    split_alias(body)
        .map(|(target_path, exported_name)| ParsedReExport {
            target_path,
            exported_name,
            source_line: line_no,
        })
        .into_iter()
        .collect()
}

fn normalize_target_path(crate_name: &str, target_path: &str) -> String {
    let trimmed = target_path.trim();
    if let Some(rest) = trimmed.strip_prefix("crate::") {
        format!("{crate_name}::{rest}")
    } else if let Some(rest) = trimmed.strip_prefix("self::") {
        format!("{crate_name}::{rest}")
    } else {
        trimmed.to_string()
    }
}

impl McpServer {
    pub(crate) async fn handle_crate_re_exports(
        &self,
        request: CrateReExportsRequest,
    ) -> Result<Json<CrateReExportsResponse>, String> {
        let crate_name = normalize_required(request.crate_name, "crate_name")?;
        let requested_version = normalize_optional(request.version);
        let path_prefix = normalize_optional(request.path_prefix);
        let limit = re_exports_limit(request.limit);

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

        let sources = sqlx::query_as::<_, ReExportSourceRow>(
            "SELECT path, content
             FROM source_files
             WHERE crate_version_id = $1
               AND (path = 'src/lib.rs' OR path LIKE 'src/%/mod.rs')
             ORDER BY CASE WHEN path = 'src/lib.rs' THEN 0 ELSE 1 END, path ASC",
        )
        .bind(selected_version.id)
        .fetch_all(&self.state.db)
        .await
        .map_err(|e| format!("crate.re_exports source query failed: {e}"))?;

        let mut re_exports = Vec::<CrateReExportEntry>::new();
        for source in sources {
            let module_prefix = module_prefix_from_path(&crate_row.name, &source.path);
            for (line_idx, line) in source
                .content
                .lines()
                .enumerate()
            {
                let parsed = parse_pub_use_statement(line, (line_idx + 1) as u32);
                for entry in parsed {
                    let canonical_path = format!("{}::{}", module_prefix, entry.exported_name);
                    if let Some(prefix) = path_prefix.as_deref()
                        && !canonical_path.starts_with(prefix)
                    {
                        continue;
                    }

                    let normalized_target =
                        normalize_target_path(&crate_row.name, &entry.target_path);

                    let symbol = sqlx::query_as::<_, ReExportSymbolKindRow>(
                        "SELECT
                            s.kind,
                            s.visibility
                         FROM symbols s
                         WHERE s.crate_version_id = $1
                           AND LOWER(s.name) = LOWER($2)
                         ORDER BY CASE WHEN s.visibility = 'public' THEN 0 ELSE 1 END, \
                         s.start_line ASC
                         LIMIT 1",
                    )
                    .bind(selected_version.id)
                    .bind(&entry.exported_name)
                    .fetch_optional(&self.state.db)
                    .await
                    .map_err(|e| format!("crate.re_exports symbol lookup failed: {e}"))?;

                    let kind = symbol
                        .as_ref()
                        .map(|row| row.kind.clone())
                        .unwrap_or_else(|| "unknown".to_string());
                    let visibility = symbol
                        .and_then(|row| row.visibility)
                        .unwrap_or_else(|| "public".to_string());

                    re_exports.push(CrateReExportEntry {
                        canonical_path,
                        original_definition_path: normalized_target,
                        kind,
                        visibility,
                        shortest_public_path: true,
                        source_path: source.path.clone(),
                        source_line: entry.source_line,
                    });

                    if re_exports.len() >= limit as usize {
                        break;
                    }
                }

                if re_exports.len() >= limit as usize {
                    break;
                }
            }

            if re_exports.len() >= limit as usize {
                break;
            }
        }

        let confidence_assessment = if re_exports.is_empty() {
            ConfidenceAssessment {
                level: ConfidenceLevel::Low,
                reason: "no public re-exports were detected in indexed module roots".to_string(),
            }
        } else if re_exports
            .iter()
            .any(|entry| entry.kind == "unknown")
        {
            ConfidenceAssessment {
                level: ConfidenceLevel::Medium,
                reason: "some re-export entries could not be mapped to indexed symbol kinds"
                    .to_string(),
            }
        } else {
            ConfidenceAssessment {
                level: ConfidenceLevel::High,
                reason: "re-export paths and symbol kinds were resolved from indexed source"
                    .to_string(),
            }
        };

        let freshness_check_result = freshness_outcome
            .freshness_check_result
            .clone();

        Ok(Json(CrateReExportsResponse {
            crate_name: crate_row.name,
            selected_version: selected_version.version,
            latest_version: latest_version.version,
            path_prefix,
            limit,
            count: re_exports.len(),
            re_exports,
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
                "crate.api".to_string(),
                "symbol.search".to_string(),
                "source.read".to_string(),
            ],
            provenance: "local_postgres_index(source_files, symbols)".to_string(),
        }))
    }
}
