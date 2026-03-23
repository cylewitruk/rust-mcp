use std::collections::BTreeSet;

use rmcp::Json;
pub use rust_mcp_types::types::krate::{
    CrateHotspotHit, CrateHotspotsRequest, CrateHotspotsResponse, HotspotKind, HotspotSeverity,
};
use serde::{Deserialize, Serialize};

use crate::db::tools;
use crate::mcp::models::{ConfidenceAssessment, ConfidenceLevel};
use crate::mcp::progress::ToolCallContext;
use crate::mcp::server::McpServer;
use crate::mcp::utils::{
    CursorToken, build_crate_freshness_sources, decode_cursor, encode_cursor, hotspots_limit,
    normalize_optional, normalize_required, path_glob_to_like, resolve_pagination,
    resolve_source_dir, sync_page,
};

#[derive(Debug, Serialize, Deserialize)]
struct CrateHotspotsCursorToken {
    v: u8,
    offset: u32,
    limit: u32,
    crate_name: String,
    version: Option<String>,
    path_glob: Option<String>,
    include_unsafe: bool,
    include_concurrency: bool,
}

impl CursorToken for CrateHotspotsCursorToken {
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

#[derive(Debug, Clone, Copy)]
struct HotspotPattern {
    kind: HotspotKind,
    text: &'static str,
    severity: HotspotSeverity,
}

fn severity_rank(severity: HotspotSeverity) -> u8 {
    match severity {
        HotspotSeverity::High => 0,
        HotspotSeverity::Medium => 1,
        HotspotSeverity::Low => 2,
    }
}

fn hotspot_patterns(include_unsafe: bool, include_concurrency: bool) -> Vec<HotspotPattern> {
    let mut out = Vec::<HotspotPattern>::new();

    if include_unsafe {
        out.extend([
            HotspotPattern {
                kind: HotspotKind::Unsafe,
                text: "unsafe",
                severity: HotspotSeverity::High,
            },
            HotspotPattern {
                kind: HotspotKind::Unsafe,
                text: "extern \"C\"",
                severity: HotspotSeverity::High,
            },
            HotspotPattern {
                kind: HotspotKind::Unsafe,
                text: "*const",
                severity: HotspotSeverity::High,
            },
            HotspotPattern {
                kind: HotspotKind::Unsafe,
                text: "*mut",
                severity: HotspotSeverity::High,
            },
            HotspotPattern {
                kind: HotspotKind::Unsafe,
                text: "UnsafeCell",
                severity: HotspotSeverity::Medium,
            },
        ]);
    }

    if include_concurrency {
        out.extend([
            HotspotPattern {
                kind: HotspotKind::Concurrency,
                text: "std::sync",
                severity: HotspotSeverity::Medium,
            },
            HotspotPattern {
                kind: HotspotKind::Concurrency,
                text: "Mutex",
                severity: HotspotSeverity::Medium,
            },
            HotspotPattern {
                kind: HotspotKind::Concurrency,
                text: "RwLock",
                severity: HotspotSeverity::Medium,
            },
            HotspotPattern {
                kind: HotspotKind::Concurrency,
                text: "Atomic",
                severity: HotspotSeverity::Medium,
            },
            HotspotPattern {
                kind: HotspotKind::Concurrency,
                text: "channel(",
                severity: HotspotSeverity::Low,
            },
            HotspotPattern {
                kind: HotspotKind::Concurrency,
                text: "parking_lot",
                severity: HotspotSeverity::Medium,
            },
        ]);
    }

    out
}

fn detect_hotspots_in_file(
    path: &str,
    content: &str,
    patterns: &[HotspotPattern],
) -> Vec<CrateHotspotHit> {
    let mut hits = Vec::<CrateHotspotHit>::new();
    let mut seen = BTreeSet::<(u32, String)>::new();

    for (line_index, line) in content.lines().enumerate() {
        let line_number = (line_index + 1) as u32;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let line_lower = line.to_ascii_lowercase();
        for pattern in patterns {
            if !line_lower.contains(
                &pattern
                    .text
                    .to_ascii_lowercase(),
            ) {
                continue;
            }

            let dedupe_key = (line_number, pattern.text.to_string());
            if !seen.insert(dedupe_key) {
                continue;
            }

            hits.push(CrateHotspotHit {
                path: path.to_string(),
                line: line_number,
                kind: pattern.kind,
                pattern: pattern.text.to_string(),
                severity: pattern.severity,
                snippet: trimmed
                    .chars()
                    .take(220)
                    .collect(),
            });
        }
    }

    hits
}

impl McpServer {
    /// Handles the `crate_hotspots` tool call.
    pub async fn handle_crate_hotspots(
        &self,
        request: CrateHotspotsRequest,
        tcx: ToolCallContext,
    ) -> Result<Json<CrateHotspotsResponse>, String> {
        let crate_name = normalize_required(request.crate_name, "crate_name")?;
        let requested_version = normalize_optional(request.version);
        let path_glob = normalize_optional(request.path_glob);
        let include_unsafe = request
            .include_unsafe
            .unwrap_or(true);
        let include_concurrency = request
            .include_concurrency
            .unwrap_or(true);
        let cursor = normalize_optional(request.cursor);
        let page = sync_page(request.page);
        let requested_limit = hotspots_limit(request.limit);

        let decoded = cursor
            .as_deref()
            .map(decode_cursor::<CrateHotspotsCursorToken>)
            .transpose()?;

        if let Some(ref token) = decoded
            && (token.crate_name != crate_name
                || token.version != requested_version
                || token.path_glob != path_glob
                || token.include_unsafe != include_unsafe
                || token.include_concurrency != include_concurrency)
        {
            return Err("cursor does not match current crate_hotspots filters".to_string());
        }

        let pag =
            resolve_pagination(decoded.as_ref(), request.limit.is_some(), requested_limit, page)?;

        if !include_unsafe && !include_concurrency {
            return Err(
                "at least one of include_unsafe/include_concurrency must be true".to_string()
            );
        }

        let ctx = self
            .fetch_crate_context(&crate_name, &tcx)
            .await?;
        let resolution = self
            .resolve_version_or_latest(&ctx, requested_version.as_deref(), &tcx)
            .await?;

        let patterns = hotspot_patterns(include_unsafe, include_concurrency);

        let path_like = path_glob
            .as_deref()
            .map(path_glob_to_like);
        let file_rows = tools::list_source_file_paths_for_hotspots(
            &self.state.db,
            resolution.selected_version.id,
            path_like.as_deref(),
        )
        .await
        .map_err(|e| format!("crate_hotspots source scan failed: {e}"))?;

        let version_dir = resolve_source_dir(
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
            &crate_name,
            &resolution
                .selected_version
                .version,
        );

        let scanned_files = file_rows.len();
        let mut hotspots = Vec::new();
        if let Some(ref vdir) = version_dir {
            for row in &file_rows {
                let full_path = vdir.join(&row.path);
                if let Ok(content) = std::fs::read_to_string(&full_path) {
                    hotspots.extend(detect_hotspots_in_file(&row.path, &content, &patterns));
                }
            }
        }

        hotspots.sort_by(|left, right| {
            severity_rank(left.severity)
                .cmp(&severity_rank(right.severity))
                .then_with(|| left.path.cmp(&right.path))
                .then_with(|| left.line.cmp(&right.line))
                .then_with(|| {
                    left.pattern
                        .cmp(&right.pattern)
                })
        });
        let mut hotspots = hotspots
            .into_iter()
            .skip(pag.offset as usize)
            .take(pag.limit as usize + 1)
            .collect::<Vec<_>>();
        let has_more = hotspots.len() > pag.limit as usize;
        if has_more {
            hotspots.truncate(pag.limit as usize);
        }
        let next_cursor = if has_more {
            Some(encode_cursor(&CrateHotspotsCursorToken {
                v: 1,
                offset: pag
                    .offset
                    .saturating_add(pag.limit),
                limit: pag.limit,
                crate_name: crate_name.clone(),
                version: requested_version.clone(),
                path_glob: path_glob.clone(),
                include_unsafe,
                include_concurrency,
            })?)
        } else {
            None
        };

        let confidence_assessment = if scanned_files == 0 {
            ConfidenceAssessment {
                level: ConfidenceLevel::Low,
                reason: "no source files were indexed for the selected version".to_string(),
            }
        } else if hotspots.is_empty() {
            ConfidenceAssessment {
                level: ConfidenceLevel::Medium,
                reason: "no hotspots matched configured lexical patterns".to_string(),
            }
        } else {
            ConfidenceAssessment {
                level: ConfidenceLevel::High,
                reason: "hotspots derived from indexed source files with deterministic pattern \
                         matching"
                    .to_string(),
            }
        };

        let freshness_check_result = ctx
            .freshness_outcome
            .freshness_check_result
            .clone();

        let suggested_next_tools = if hotspots.is_empty() {
            vec!["index_crates".to_string(), "source_search".to_string(), "crate_api".to_string()]
        } else {
            vec!["source_read".to_string(), "symbol_search".to_string(), "crate_graph".to_string()]
        };

        Ok(Json(CrateHotspotsResponse {
            crate_name: ctx.crate_row.name,
            selected_version: resolution
                .selected_version
                .version,
            latest_version: ctx.latest_version.version,
            path_glob,
            include_unsafe,
            include_concurrency,
            cursor,
            next_cursor,
            page: pag.effective_page,
            limit: pag.limit,
            has_more,
            truncated: has_more,
            scanned_files,
            count: hotspots.len(),
            hotspots,
            freshness_check_performed: ctx
                .freshness_outcome
                .freshness_check_performed,
            freshness_check_result: freshness_check_result.clone(),
            refresh_enqueued: resolution.refresh_enqueued,
            refresh_job_id: resolution.refresh_job_id,
            freshness: build_crate_freshness_sources(
                ctx.crate_row
                    .updated_at
                    .clone(),
                &freshness_check_result,
            ),
            confidence: confidence_assessment
                .level
                .as_str()
                .to_string(),
            confidence_assessment,
            suggested_next_tools,
            provenance: "local_postgres_index".to_string(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::{HotspotKind, HotspotSeverity, detect_hotspots_in_file, hotspot_patterns};

    #[test]
    fn detects_unsafe_and_concurrency_hotspots() {
        let patterns = hotspot_patterns(true, true);
        let content = r#"
pub fn demo() {
    let _guard = std::sync::Mutex::new(1);
    unsafe { core::ptr::read_volatile(&1) }
}
"#;

        let hits = detect_hotspots_in_file("src/lib.rs", content, &patterns);
        assert!(
            hits.iter()
                .any(|hit| hit.kind == HotspotKind::Unsafe)
        );
        assert!(
            hits.iter()
                .any(|hit| hit.kind == HotspotKind::Concurrency)
        );
    }

    #[test]
    fn dedupes_same_pattern_per_line() {
        let patterns = hotspot_patterns(false, true);
        let content = "let _ = std::sync::Mutex::new(std::sync::Mutex::new(1));";

        let hits = detect_hotspots_in_file("src/lib.rs", content, &patterns);
        let mutex_hits = hits
            .iter()
            .filter(|hit| hit.pattern == "Mutex")
            .count();
        assert_eq!(mutex_hits, 1);
    }

    #[test]
    fn marks_unsafe_as_high_severity() {
        let patterns = hotspot_patterns(true, false);
        let content = "let ptr = x as *const i32;";
        let hits = detect_hotspots_in_file("src/lib.rs", content, &patterns);

        assert!(
            hits.iter()
                .any(|hit| { hit.pattern == "*const" && hit.severity == HotspotSeverity::High })
        );
    }
}
