use std::collections::BTreeSet;

use rmcp::Json;

use super::models::{
    ConfidenceAssessment, ConfidenceLevel, CrateCoreRow, CrateHotspotHit, CrateHotspotsRequest,
    CrateHotspotsResponse, CrateVersionSelectionRow, HotspotKind, HotspotSeverity,
    HotspotSourceFileRow, ResponseFreshnessSource,
};
use super::server::McpServer;
use super::utils::{hotspots_limit, normalize_optional, normalize_required, path_glob_to_like};

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
    pub(super) async fn handle_crate_hotspots(
        &self,
        request: CrateHotspotsRequest,
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
        let limit = hotspots_limit(request.limit);

        if !include_unsafe && !include_concurrency {
            return Err(
                "at least one of include_unsafe/include_concurrency must be true".to_string()
            );
        }

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

        let patterns = hotspot_patterns(include_unsafe, include_concurrency);

        let files = if let Some(path_filter) = path_glob.as_deref() {
            sqlx::query_as::<_, HotspotSourceFileRow>(
                "SELECT
                    sf.path,
                    sf.content
                 FROM source_files sf
                 WHERE sf.crate_version_id = $1
                   AND sf.path ILIKE $2 ESCAPE '\\\\'
                 ORDER BY sf.path ASC
                 LIMIT 1000",
            )
            .bind(selected_version.id)
            .bind(path_glob_to_like(path_filter))
            .fetch_all(&self.state.db)
            .await
            .map_err(|e| format!("crate.hotspots source scan failed: {e}"))?
        } else {
            sqlx::query_as::<_, HotspotSourceFileRow>(
                "SELECT
                    sf.path,
                    sf.content
                 FROM source_files sf
                 WHERE sf.crate_version_id = $1
                 ORDER BY sf.path ASC
                 LIMIT 1000",
            )
            .bind(selected_version.id)
            .fetch_all(&self.state.db)
            .await
            .map_err(|e| format!("crate.hotspots source scan failed: {e}"))?
        };

        let scanned_files = files.len();
        let mut hotspots = files
            .iter()
            .flat_map(|row| detect_hotspots_in_file(&row.path, &row.content, &patterns))
            .collect::<Vec<_>>();

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
        hotspots.truncate(limit as usize);

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

        let freshness_check_result = freshness_outcome
            .freshness_check_result
            .clone();

        Ok(Json(CrateHotspotsResponse {
            crate_name: crate_row.name,
            selected_version: selected_version.version,
            latest_version: latest_version.version,
            path_glob,
            include_unsafe,
            include_concurrency,
            limit,
            scanned_files,
            count: hotspots.len(),
            hotspots,
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
                "symbol.search".to_string(),
                "crate.graph".to_string(),
            ],
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
