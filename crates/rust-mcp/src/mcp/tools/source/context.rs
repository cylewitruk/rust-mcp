use rmcp::{Json, schemars};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::mcp::models::{
    ConfidenceAssessment, ConfidenceLevel, CrateCoreRow, CrateVersionSelectionRow,
    ResponseFreshnessSource, SourceReadRow,
};
use crate::mcp::server::McpServer;
use crate::mcp::utils::{normalize_optional, normalize_required};

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SourceContextRequest {
    pub crate_name: String,
    pub version: Option<String>,
    pub path: String,
    pub line: Option<u32>,
    pub symbol_name: Option<String>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct SourceContextResponse {
    pub crate_name: String,
    pub selected_version: String,
    pub latest_version: String,
    pub path: String,
    pub line: u32,
    pub symbol_name: Option<String>,
    pub module_path: String,
    pub imports_in_scope: Vec<String>,
    pub containing_impl: Option<SourceContextImplBlock>,
    pub surrounding_types: Vec<SourceContextTypeContext>,
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
pub struct SourceContextImplBlock {
    pub type_name: String,
    pub type_name_display: Option<String>,
    pub trait_name: Option<String>,
    pub trait_name_display: Option<String>,
    pub impl_kind: String,
    pub source_line: i32,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct SourceContextTypeContext {
    pub type_name: String,
    pub kind: String,
    pub source_line: i32,
}

#[derive(Debug, Clone, FromRow)]
pub(crate) struct SourceContextLineLookupRow {
    pub(crate) start_line: i32,
}

#[derive(Debug, Clone, FromRow)]
pub(crate) struct SourceContextImplLookupRow {
    pub(crate) type_name: String,
    pub(crate) type_name_display: Option<String>,
    pub(crate) trait_name: Option<String>,
    pub(crate) trait_name_display: Option<String>,
    pub(crate) impl_kind: String,
    pub(crate) start_line: i32,
}

#[derive(Debug, Clone, FromRow)]
pub(crate) struct SourceContextTypeLookupRow {
    pub(crate) type_name: String,
    pub(crate) kind: String,
    pub(crate) start_line: i32,
}

fn module_path_from_source_path(crate_name: &str, path: &str) -> String {
    let normalized = path.trim_start_matches("./");
    if normalized == "src/lib.rs" || normalized == "src/main.rs" {
        return crate_name.to_string();
    }

    let mut parts = normalized
        .trim_start_matches("src/")
        .split('/')
        .collect::<Vec<_>>();

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

fn collect_imports_in_scope(content: &str, line: u32) -> Vec<String> {
    let mut imports = Vec::<String>::new();
    let mut buffer = String::new();
    let mut collecting = false;

    for (line_index, raw_line) in content.lines().enumerate() {
        let current_line = (line_index + 1) as u32;
        if current_line > line {
            break;
        }

        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if collecting {
            if !buffer.is_empty() {
                buffer.push(' ');
            }
            buffer.push_str(trimmed);

            if trimmed.ends_with(';') {
                imports.push(buffer.trim().to_string());
                buffer.clear();
                collecting = false;
            }
            continue;
        }

        if trimmed.starts_with("use ") || trimmed.starts_with("pub use ") {
            buffer.push_str(trimmed);
            if trimmed.ends_with(';') {
                imports.push(buffer.trim().to_string());
                buffer.clear();
            } else {
                collecting = true;
            }
        }
    }

    imports.sort();
    imports.dedup();
    imports
}

impl McpServer {
    pub(crate) async fn handle_source_context(
        &self,
        request: SourceContextRequest,
    ) -> Result<Json<SourceContextResponse>, String> {
        let crate_name = normalize_required(request.crate_name, "crate_name")?;
        let path = normalize_required(request.path, "path")?;
        let requested_version = normalize_optional(request.version);
        let symbol_name = normalize_optional(request.symbol_name);

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

        let row = sqlx::query_as::<_, SourceReadRow>(
            "SELECT
                c.name AS crate_name,
                cv.version,
                sf.path,
                sf.content
             FROM source_files sf
             JOIN crate_versions cv ON cv.id = sf.crate_version_id
             JOIN crates c ON c.id = cv.crate_id
             WHERE c.name = $1 AND cv.id = $2 AND sf.path = $3
             LIMIT 1",
        )
        .bind(&crate_row.name)
        .bind(selected_version.id)
        .bind(&path)
        .fetch_optional(&self.state.db)
        .await
        .map_err(|e| {
            format!(
                "source.context lookup failed for {}@{}:{}: {e}",
                crate_row.name, selected_version.version, path
            )
        })?
        .ok_or_else(|| {
            format!(
                "source file not found for {}@{}:{}",
                crate_row.name, selected_version.version, path
            )
        })?;

        let total_lines = row
            .content
            .lines()
            .count()
            .max(1) as u32;

        let resolved_line = if let Some(line) = request.line {
            line.clamp(1, total_lines)
        } else if let Some(ref symbol) = symbol_name {
            let symbol_line = sqlx::query_as::<_, SourceContextLineLookupRow>(
                "SELECT s.start_line
                 FROM symbols s
                 JOIN source_files sf ON sf.id = s.source_file_id
                 WHERE s.crate_version_id = $1
                   AND sf.path = $2
                   AND s.name = $3
                 ORDER BY s.start_line ASC
                 LIMIT 1",
            )
            .bind(selected_version.id)
            .bind(&path)
            .bind(symbol)
            .fetch_optional(&self.state.db)
            .await
            .map_err(|e| {
                format!(
                    "source.context symbol lookup failed for {}@{}:{}:{}: {e}",
                    crate_row.name, selected_version.version, path, symbol
                )
            })?;

            symbol_line
                .map(|value| value.start_line.max(1) as u32)
                .ok_or_else(|| {
                    format!(
                        "symbol '{}' not found in {}@{}:{}",
                        symbol, crate_row.name, selected_version.version, path
                    )
                })?
                .clamp(1, total_lines)
        } else {
            return Err("source.context requires either line or symbol_name".to_string());
        };

        let imports_in_scope = collect_imports_in_scope(&row.content, resolved_line);
        let module_path = module_path_from_source_path(&crate_row.name, &path);

        let containing_impl = sqlx::query_as::<_, SourceContextImplLookupRow>(
            "SELECT
                ci.type_name,
                ci.type_name_display,
                ci.trait_name,
                ci.trait_name_display,
                ci.impl_kind,
                ci.start_line
             FROM crate_impls ci
             JOIN source_files sf ON sf.id = ci.source_file_id
             WHERE ci.crate_version_id = $1
               AND sf.path = $2
               AND ci.start_line <= $3
             ORDER BY ci.start_line DESC
             LIMIT 1",
        )
        .bind(selected_version.id)
        .bind(&path)
        .bind(resolved_line as i32)
        .fetch_optional(&self.state.db)
        .await
        .map_err(|e| format!("source.context impl lookup failed: {e}"))?
        .filter(|row| ((resolved_line as i32) - row.start_line).abs() <= 200)
        .map(|row| SourceContextImplBlock {
            type_name: row.type_name,
            type_name_display: row.type_name_display,
            trait_name: row.trait_name,
            trait_name_display: row.trait_name_display,
            impl_kind: row.impl_kind,
            source_line: row.start_line,
        });

        let surrounding_type_rows = sqlx::query_as::<_, SourceContextTypeLookupRow>(
            "SELECT
                ct.type_name,
                ct.kind,
                ct.start_line
             FROM crate_types ct
             JOIN source_files sf ON sf.id = ct.source_file_id
             WHERE ct.crate_version_id = $1
               AND sf.path = $2
             ORDER BY ABS(ct.start_line - $3), ct.start_line ASC
             LIMIT 5",
        )
        .bind(selected_version.id)
        .bind(&path)
        .bind(resolved_line as i32)
        .fetch_all(&self.state.db)
        .await
        .map_err(|e| format!("source.context type lookup failed: {e}"))?;

        let surrounding_types = surrounding_type_rows
            .into_iter()
            .map(|row| SourceContextTypeContext {
                type_name: row.type_name,
                kind: row.kind,
                source_line: row.start_line,
            })
            .collect::<Vec<_>>();

        let freshness_check_result = freshness_outcome
            .freshness_check_result
            .clone();
        let confidence_assessment = if imports_in_scope.is_empty()
            && containing_impl.is_none()
            && surrounding_types.is_empty()
        {
            ConfidenceAssessment {
                level: ConfidenceLevel::Low,
                reason: "indexed source context is limited for this location".to_string(),
            }
        } else {
            ConfidenceAssessment {
                level: ConfidenceLevel::Medium,
                reason: "module/import/type context derived from indexed source and syn metadata"
                    .to_string(),
            }
        };

        Ok(Json(SourceContextResponse {
            crate_name: crate_row.name,
            selected_version: selected_version.version,
            latest_version: latest_version.version,
            path,
            line: resolved_line,
            symbol_name,
            module_path,
            imports_in_scope,
            containing_impl,
            surrounding_types,
            freshness_check_performed: freshness_outcome.freshness_check_performed,
            freshness_check_result: freshness_check_result.clone(),
            refresh_enqueued,
            refresh_job_id,
            freshness: vec![ResponseFreshnessSource {
                source: "crates.io".to_string(),
                status: freshness_check_result,
                checked_at: None,
            }],
            confidence: confidence_assessment
                .level
                .as_str()
                .to_string(),
            confidence_assessment,
            next_best_calls: vec![
                "source.read".to_string(),
                "symbol.search".to_string(),
                "crate.type_info".to_string(),
            ],
            provenance: "source_files + symbols + crate_impls + crate_types".to_string(),
        }))
    }
}
