use rmcp::Json;
pub use rust_mcp_types::types::source::{
    SourceContextImplBlock, SourceContextRequest, SourceContextResponse, SourceContextTypeContext,
};

use crate::db::tools;
use crate::mcp::models::{ConfidenceAssessment, ConfidenceLevel, ResponseFreshnessSource};
use crate::mcp::server::McpServer;
use crate::mcp::utils::{
    normalize_optional, normalize_required, read_source_file_from_disk_or_cache,
};

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
    /// Handles the `source.context` tool call.
    pub async fn handle_source_context(
        &self,
        request: SourceContextRequest,
    ) -> Result<Json<SourceContextResponse>, String> {
        let crate_name = normalize_required(request.crate_name, "crate_name")?;
        let path = normalize_required(request.path, "path")?;
        let requested_version = normalize_optional(request.version);
        let symbol_name = normalize_optional(request.symbol_name);

        let crate_row = tools::fetch_crate_core_by_name(&self.state.db, &crate_name)
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

        let mut refresh_enqueued = freshness_outcome.refresh_enqueued;
        let mut refresh_job_id = freshness_outcome
            .refresh_job_id
            .clone();

        let selected_version = if let Some(version) = requested_version {
            let selected =
                tools::fetch_crate_version_by_name(&self.state.db, crate_row.id, &version)
                    .await
                    .map_err(|e| {
                        format!(
                            "selected version lookup failed for {}@{}: {e}",
                            crate_row.name, version
                        )
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

                tools::fetch_crate_version_by_name(&self.state.db, crate_row.id, &version)
                    .await
                    .map_err(|e| {
                        format!(
                            "selected version lookup failed after backfill for {}@{}: {e}",
                            crate_row.name, version
                        )
                    })?
                    .ok_or_else(|| {
                        format!(
                            "version '{}' for crate '{}' is not indexed locally (refresh \
                             attempted)",
                            version, crate_row.name
                        )
                    })?
            }
        } else {
            latest_version.clone()
        };

        self.ensure_source_indexed(&crate_name, selected_version.id)
            .await?;
        self.ensure_source_on_disk(&crate_name, &selected_version.version, selected_version.id)
            .await?;

        let row = tools::fetch_source_read_for_crate_version_path(
            &self.state.db,
            &crate_row.name,
            selected_version.id,
            &path,
        )
        .await
        .map_err(|e| {
            format!(
                "source_context lookup failed for {}@{}:{}: {e}",
                crate_row.name, selected_version.version, path
            )
        })?
        .ok_or_else(|| {
            format!(
                "source file not found for {}@{}:{}. Try crate_api or symbol_search to explore \
                 this crate's API surface.",
                crate_row.name, selected_version.version, path
            )
        })?;

        let content = read_source_file_from_disk_or_cache(
            &self
                .state
                .config
                .cargo_registry_dir,
            Some(
                &self
                    .state
                    .config
                    .crate_source_cache_dir,
            ),
            &crate_row.name,
            &selected_version.version,
            &row.path,
        )
        .ok_or_else(|| {
            format!(
                "source content not available on disk for {}@{}:{} (not found in cargo registry). \
                 Try crate_type_info, crate_api, or symbol_search for rustdoc-based API data.",
                crate_row.name, selected_version.version, path
            )
        })?;

        let total_lines = content.lines().count().max(1) as u32;

        let resolved_line = if let Some(line) = request.line {
            line.clamp(1, total_lines)
        } else if let Some(ref symbol) = symbol_name {
            let symbol_line = tools::fetch_symbol_start_line_for_context(
                &self.state.db,
                selected_version.id,
                &path,
                symbol,
            )
            .await
            .map_err(|e| {
                format!(
                    "source_context symbol lookup failed for {}@{}:{}:{}: {e}",
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
            return Err("source_context requires either line or symbol_name".to_string());
        };

        let imports_in_scope = collect_imports_in_scope(&content, resolved_line);
        let module_path = module_path_from_source_path(&crate_row.name, &path);

        let containing_impl = tools::fetch_containing_impl_for_context(
            &self.state.db,
            selected_version.id,
            &path,
            resolved_line as i32,
        )
        .await
        .map_err(|e| format!("source_context impl lookup failed: {e}"))?
        .filter(|row| ((resolved_line as i32) - row.start_line).abs() <= 200)
        .map(|row| SourceContextImplBlock {
            type_name: row.type_name,
            type_name_display: row.type_name_display,
            trait_name: row.trait_name,
            trait_name_display: row.trait_name_display,
            impl_kind: row.impl_kind,
            source_line: row.start_line,
        });

        let surrounding_type_rows = tools::fetch_surrounding_types_for_context(
            &self.state.db,
            selected_version.id,
            &path,
            resolved_line as i32,
            5,
        )
        .await
        .map_err(|e| format!("source_context type lookup failed: {e}"))?;

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
                "source_read".to_string(),
                "symbol_search".to_string(),
                "crate_type_info".to_string(),
            ],
            provenance: "source_files + symbols + crate_impls + crate_types".to_string(),
        }))
    }
}
