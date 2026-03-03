use rmcp::Json;
pub use rust_mcp_types::types::krate::{
    CrateReExportEntry, CrateReExportsRequest, CrateReExportsResponse,
};
use serde::{Deserialize, Serialize};

use crate::db::tools;
use crate::mcp::models::{ConfidenceAssessment, ConfidenceLevel};
use crate::mcp::server::McpServer;
use crate::mcp::utils::{
    CursorToken, build_crate_freshness_sources, decode_cursor, encode_cursor, normalize_optional,
    normalize_required, re_exports_limit, read_source_file_from_disk_or_cache, resolve_pagination,
    sync_page,
};

#[derive(Debug, Serialize, Deserialize)]
struct CrateReExportsCursorToken {
    v: u8,
    offset: u32,
    limit: u32,
    crate_name: String,
    version: Option<String>,
    path_prefix: Option<String>,
}

impl CursorToken for CrateReExportsCursorToken {
    fn version(&self) -> u8 {
        self.v
    }
    fn limit(&self) -> u32 {
        self.limit
    }
    fn offset(&self) -> u32 {
        self.offset
    }
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
    /// Handles the `crate.re_exports` tool call.
    pub async fn handle_crate_re_exports(
        &self,
        request: CrateReExportsRequest,
    ) -> Result<Json<CrateReExportsResponse>, String> {
        let crate_name = normalize_required(request.crate_name, "crate_name")?;
        let requested_version = normalize_optional(request.version);
        let path_prefix = normalize_optional(request.path_prefix);
        let cursor = normalize_optional(request.cursor);
        let page = sync_page(request.page);
        let requested_limit = re_exports_limit(request.limit);

        let decoded = cursor
            .as_deref()
            .map(decode_cursor::<CrateReExportsCursorToken>)
            .transpose()?;

        if let Some(ref token) = decoded
            && (token.crate_name != crate_name
                || token.version != requested_version
                || token.path_prefix != path_prefix)
        {
            return Err("cursor does not match current crate.re_exports filters".to_string());
        }

        let pag =
            resolve_pagination(decoded.as_ref(), request.limit.is_some(), requested_limit, page)?;

        let ctx = self
            .fetch_crate_context(&crate_name)
            .await?;
        let resolution = self
            .resolve_version_or_latest(&ctx, requested_version.as_deref())
            .await?;

        self.ensure_rustdoc_indexed(&crate_name, resolution.selected_version.id)
            .await?;

        let rustdoc_re_exports = tools::list_rustdoc_re_exports(
            &self.state.db,
            resolution.selected_version.id,
            path_prefix.as_deref(),
            i64::from(pag.limit.saturating_add(1)),
            i64::from(pag.offset),
        )
        .await
        .map_err(|e| format!("crate_re_exports rustdoc query failed: {e}"))?;

        let (mut re_exports, confidence_assessment, provenance) = if !rustdoc_re_exports.is_empty()
        {
            let entries = rustdoc_re_exports
                .into_iter()
                .map(|row| CrateReExportEntry {
                    canonical_path: row.canonical_path,
                    original_definition_path: row.original_definition_path,
                    kind: row.kind,
                    visibility: row
                        .visibility
                        .unwrap_or_else(|| "public".to_string()),
                    shortest_public_path: true,
                    source_path: row.source_path,
                    source_line: row.source_line.max(1) as u32,
                })
                .collect::<Vec<_>>();

            (
                entries,
                ConfidenceAssessment {
                    level: ConfidenceLevel::High,
                    reason: "re-export canonical paths were resolved from rustdoc_json symbol \
                             graph"
                        .to_string(),
                },
                "local_postgres_index(symbols[rustdoc_json], source_files)".to_string(),
            )
        } else {
            let sources = tools::list_re_export_sources_for_version(
                &self.state.db,
                resolution.selected_version.id,
            )
            .await
            .map_err(|e| format!("crate_re_exports source query failed: {e}"))?;

            let max_results = pag
                .offset
                .saturating_add(pag.limit)
                .saturating_add(1) as usize;
            let mut entries = Vec::<CrateReExportEntry>::new();
            for source in sources {
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
                    &ctx.crate_row.name,
                    &resolution
                        .selected_version
                        .version,
                    &source.path,
                );
                let Some(ref content) = content else {
                    continue;
                };
                let module_prefix = module_prefix_from_path(&ctx.crate_row.name, &source.path);
                for (line_idx, line) in content.lines().enumerate() {
                    let parsed = parse_pub_use_statement(line, (line_idx + 1) as u32);
                    for entry in parsed {
                        let canonical_path = format!("{}::{}", module_prefix, entry.exported_name);
                        if let Some(prefix) = path_prefix.as_deref()
                            && !canonical_path.starts_with(prefix)
                        {
                            continue;
                        }

                        let normalized_target =
                            normalize_target_path(&ctx.crate_row.name, &entry.target_path);

                        let symbol = tools::fetch_re_export_symbol_kind(
                            &self.state.db,
                            resolution.selected_version.id,
                            &entry.exported_name,
                        )
                        .await
                        .map_err(|e| format!("crate_re_exports symbol lookup failed: {e}"))?;

                        let kind = symbol
                            .as_ref()
                            .map(|row| row.kind.clone())
                            .unwrap_or_else(|| "unknown".to_string());
                        let visibility = symbol
                            .and_then(|row| row.visibility)
                            .unwrap_or_else(|| "public".to_string());

                        entries.push(CrateReExportEntry {
                            canonical_path,
                            original_definition_path: normalized_target,
                            kind,
                            visibility,
                            shortest_public_path: true,
                            source_path: source.path.clone(),
                            source_line: entry.source_line,
                        });

                        if entries.len() >= max_results {
                            break;
                        }
                    }

                    if entries.len() >= max_results {
                        break;
                    }
                }

                if entries.len() >= max_results {
                    break;
                }
            }

            let confidence = if entries.is_empty() {
                ConfidenceAssessment {
                    level: ConfidenceLevel::Low,
                    reason: "no public re-exports were detected in indexed module roots"
                        .to_string(),
                }
            } else if entries
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

            (entries, confidence, "local_postgres_index(source_files, symbols)".to_string())
        };

        if pag.offset > 0 {
            re_exports = re_exports
                .into_iter()
                .skip(pag.offset as usize)
                .collect();
        }
        let has_more = re_exports.len() > pag.limit as usize;
        if has_more {
            re_exports.truncate(pag.limit as usize);
        }
        let next_cursor = if has_more {
            Some(encode_cursor(&CrateReExportsCursorToken {
                v: 1,
                offset: pag
                    .offset
                    .saturating_add(pag.limit),
                limit: pag.limit,
                crate_name: crate_name.clone(),
                version: requested_version.clone(),
                path_prefix: path_prefix.clone(),
            })?)
        } else {
            None
        };

        let freshness_check_result = ctx
            .freshness_outcome
            .freshness_check_result
            .clone();

        Ok(Json(CrateReExportsResponse {
            crate_name: ctx.crate_row.name,
            selected_version: resolution
                .selected_version
                .version,
            latest_version: ctx.latest_version.version,
            path_prefix,
            cursor,
            next_cursor,
            page: pag.effective_page,
            limit: pag.limit,
            has_more,
            truncated: has_more,
            count: re_exports.len(),
            re_exports,
            freshness_check_performed: ctx
                .freshness_outcome
                .freshness_check_performed,
            freshness_check_result: freshness_check_result.clone(),
            refresh_enqueued: resolution.refresh_enqueued,
            refresh_job_id: resolution.refresh_job_id,
            freshness: build_crate_freshness_sources(
                ctx.crate_row.updated_at,
                &freshness_check_result,
            ),
            confidence: confidence_assessment
                .level
                .as_str()
                .to_string(),
            confidence_assessment,
            next_best_calls: vec![
                "crate_api".to_string(),
                "symbol_search".to_string(),
                "source_read".to_string(),
            ],
            provenance,
        }))
    }
}
