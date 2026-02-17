use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rmcp::Json;
pub use rust_mcp_types::types::dependency::{
    DependencyAuditDependency, DependencyAuditIssue, DependencyAuditIssueCategory,
    DependencyAuditRequest, DependencyAuditResponse, DependencyAuditSeverity,
};
use semver::{Version, VersionReq};
use sqlx::FromRow;
use toml::Value;

use crate::mcp::models::{ConfidenceAssessment, ConfidenceLevel, ResponseFreshnessSource};
use crate::mcp::server::McpServer;
use crate::mcp::utils::normalize_required;

#[derive(Debug, Clone, FromRow)]
pub(crate) struct DependencyAuditCrateRow {
    pub(crate) id: i64,
    pub(crate) name: String,
}

#[derive(Debug, Clone, FromRow)]
pub(crate) struct DependencyAuditVersionRow {
    pub(crate) id: i64,
    pub(crate) version: String,
    pub(crate) rust_version: Option<String>,
    pub(crate) yanked: bool,
}

#[derive(Debug, Clone)]
struct ParsedDependency {
    name: String,
    requirement: Option<String>,
}

#[derive(Debug)]
struct ParsedManifest {
    package_name: Option<String>,
    package_rust_version: Option<String>,
    dependencies: Vec<ParsedDependency>,
}

fn parse_semver_for_matching(version: &str) -> Option<Version> {
    Version::parse(version)
        .ok()
        .or_else(|| Version::parse(version.trim_start_matches('v')).ok())
}

fn parse_rust_version_floor(version: &str) -> Option<Version> {
    let trimmed = version.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Ok(parsed) = Version::parse(trimmed) {
        return Some(parsed);
    }

    let dotted = trimmed.trim_start_matches('v');
    let parts = dotted
        .split('.')
        .collect::<Vec<_>>();
    match parts.as_slice() {
        [major] => Version::parse(&format!("{major}.0.0")).ok(),
        [major, minor] => Version::parse(&format!("{major}.{minor}.0")).ok(),
        [major, minor, patch] => Version::parse(&format!("{major}.{minor}.{patch}")).ok(),
        _ => None,
    }
}

fn parse_requirement(value: &Value) -> Option<String> {
    if let Some(req) = value.as_str() {
        let trimmed = req.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    let table = value.as_table()?;
    let version = table
        .get("version")?
        .as_str()?;
    let trimmed = version.trim();
    if trimmed.is_empty() { None } else { Some(trimmed.to_string()) }
}

fn collect_dependency_table(
    table: Option<&toml::map::Map<String, Value>>,
) -> Vec<ParsedDependency> {
    let mut out = Vec::<ParsedDependency>::new();
    let Some(table) = table else {
        return out;
    };

    for (name, spec) in table {
        let trimmed_name = name.trim();
        if trimmed_name.is_empty() {
            continue;
        }

        out.push(ParsedDependency {
            name: trimmed_name.to_string(),
            requirement: parse_requirement(spec),
        });
    }

    out
}

fn parse_manifest(manifest: &str) -> Result<ParsedManifest, String> {
    let parsed =
        toml::from_str::<Value>(manifest).map_err(|e| format!("invalid Cargo.toml: {e}"))?;

    let package_name = parsed
        .get("package")
        .and_then(Value::as_table)
        .and_then(|table| table.get("name"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(ToString::to_string);

    let package_rust_version = parsed
        .get("package")
        .and_then(Value::as_table)
        .and_then(|table| table.get("rust-version"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(ToString::to_string);

    let mut merged = BTreeMap::<String, ParsedDependency>::new();

    let root = parsed.as_table();
    let mut root_deps = Vec::<ParsedDependency>::new();
    if let Some(root) = root {
        root_deps.extend(collect_dependency_table(
            root.get("dependencies")
                .and_then(Value::as_table),
        ));
        root_deps.extend(collect_dependency_table(
            root.get("dev-dependencies")
                .and_then(Value::as_table),
        ));
        root_deps.extend(collect_dependency_table(
            root.get("build-dependencies")
                .and_then(Value::as_table),
        ));

        if let Some(targets) = root
            .get("target")
            .and_then(Value::as_table)
        {
            for target_value in targets.values() {
                let Some(target_table) = target_value.as_table() else {
                    continue;
                };
                root_deps.extend(collect_dependency_table(
                    target_table
                        .get("dependencies")
                        .and_then(Value::as_table),
                ));
                root_deps.extend(collect_dependency_table(
                    target_table
                        .get("dev-dependencies")
                        .and_then(Value::as_table),
                ));
                root_deps.extend(collect_dependency_table(
                    target_table
                        .get("build-dependencies")
                        .and_then(Value::as_table),
                ));
            }
        }
    }

    for dep in root_deps {
        let key = dep.name.to_ascii_lowercase();
        match merged.get(&key) {
            None => {
                merged.insert(key, dep);
            }
            Some(existing) if existing.requirement.is_none() && dep.requirement.is_some() => {
                merged.insert(key, dep);
            }
            _ => {}
        }
    }

    Ok(ParsedManifest {
        package_name,
        package_rust_version,
        dependencies: merged.into_values().collect(),
    })
}

fn resolve_manifest_path(raw_path: String) -> Result<PathBuf, String> {
    let normalized = normalize_required(raw_path, "cargo_toml_path")?;
    let candidate = PathBuf::from(&normalized);

    let final_path = if candidate.is_absolute() {
        candidate
    } else {
        std::env::current_dir()
            .map_err(|e| format!("failed to resolve current directory: {e}"))?
            .join(candidate)
    };

    if final_path
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.eq_ignore_ascii_case("Cargo.toml"))
        .unwrap_or(false)
    {
        Ok(final_path)
    } else {
        Err("cargo_toml_path must point to a Cargo.toml file".to_string())
    }
}

fn evaluate_confidence(
    dependency_count: usize,
    unresolved_count: usize,
    no_requirement_count: usize,
) -> ConfidenceAssessment {
    if dependency_count == 0 {
        return ConfidenceAssessment {
            level: ConfidenceLevel::Low,
            reason: "manifest has no dependencies to audit".to_string(),
        };
    }

    if unresolved_count > 0 {
        return ConfidenceAssessment {
            level: ConfidenceLevel::Medium,
            reason: format!("{unresolved_count} dependencies were not found in the local index"),
        };
    }

    if no_requirement_count > 0 {
        return ConfidenceAssessment {
            level: ConfidenceLevel::Medium,
            reason: format!(
                "{no_requirement_count} dependencies do not declare semver requirements"
            ),
        };
    }

    ConfidenceAssessment {
        level: ConfidenceLevel::High,
        reason: "all dependencies resolved against indexed versions and advisories".to_string(),
    }
}

impl McpServer {
    pub(crate) async fn handle_dependency_audit(
        &self,
        request: DependencyAuditRequest,
    ) -> Result<Json<DependencyAuditResponse>, String> {
        let manifest_path = resolve_manifest_path(request.cargo_toml_path)?;
        let manifest_text = tokio::fs::read_to_string(&manifest_path)
            .await
            .map_err(|e| format!("failed to read {}: {e}", manifest_path.display()))?;

        let parsed_manifest = parse_manifest(&manifest_text)?;
        let package_rust_floor = parsed_manifest
            .package_rust_version
            .as_deref()
            .and_then(parse_rust_version_floor);

        let mut dependencies = Vec::<DependencyAuditDependency>::new();
        let mut issues = Vec::<DependencyAuditIssue>::new();
        let mut unresolved_count = 0usize;
        let mut no_requirement_count = 0usize;

        for dep in parsed_manifest.dependencies {
            let crate_row = sqlx::query_as::<_, DependencyAuditCrateRow>(
                "SELECT id, name
                 FROM crates
                 WHERE name = $1
                 LIMIT 1",
            )
            .bind(&dep.name)
            .fetch_optional(&self.state.db)
            .await
            .map_err(|e| format!("dependency lookup failed for {}: {e}", dep.name))?;

            let Some(crate_row) = crate_row else {
                unresolved_count += 1;
                dependencies.push(DependencyAuditDependency {
                    dependency_name: dep.name.clone(),
                    requirement: dep.requirement.clone(),
                    selected_version: None,
                    latest_version: None,
                    selected_rust_version: None,
                    yanked: false,
                    advisory_count: 0,
                    status_markers: vec!["not_indexed".to_string()],
                });
                issues.push(DependencyAuditIssue {
                    dependency_name: dep.name,
                    category: DependencyAuditIssueCategory::Unresolved,
                    severity: DependencyAuditSeverity::Medium,
                    message: "dependency not found in local index; run index.sync_crates or \
                              index.refresh"
                        .to_string(),
                    selected_version: None,
                    latest_version: None,
                });
                continue;
            };

            let versions = sqlx::query_as::<_, DependencyAuditVersionRow>(
                "SELECT
                    id,
                    version,
                    rust_version,
                    yanked
                 FROM crate_versions
                 WHERE crate_id = $1
                 ORDER BY published_at DESC NULLS LAST, id DESC",
            )
            .bind(crate_row.id)
            .fetch_all(&self.state.db)
            .await
            .map_err(|e| format!("version lookup failed for {}: {e}", crate_row.name))?;

            let latest = versions.first().cloned();

            let requirement_req = dep
                .requirement
                .as_deref()
                .and_then(|req| VersionReq::parse(req).ok());

            if dep.requirement.is_none() {
                no_requirement_count += 1;
            }

            let selected = if let Some(req) = &requirement_req {
                versions
                    .iter()
                    .filter_map(|row| {
                        let parsed = parse_semver_for_matching(&row.version)?;
                        if req.matches(&parsed) { Some((parsed, row.clone())) } else { None }
                    })
                    .max_by(|left, right| left.0.cmp(&right.0))
                    .map(|(_, row)| row)
            } else {
                latest.clone()
            };

            if requirement_req.is_some() && selected.is_none() {
                issues.push(DependencyAuditIssue {
                    dependency_name: dep.name.clone(),
                    category: DependencyAuditIssueCategory::Unresolved,
                    severity: DependencyAuditSeverity::Medium,
                    message: "no indexed versions satisfy declared semver requirement".to_string(),
                    selected_version: None,
                    latest_version: latest
                        .as_ref()
                        .map(|row| row.version.clone()),
                });
            }

            let advisory_count = if let Some(ref selected_row) = selected {
                sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*)::BIGINT
                     FROM advisory_matches
                     WHERE crate_id = $1
                       AND (version_id = $2 OR version_id IS NULL)",
                )
                .bind(crate_row.id)
                .bind(selected_row.id)
                .fetch_one(&self.state.db)
                .await
                .map_err(|e| format!("advisory lookup failed for {}: {e}", dep.name))?
            } else {
                0
            };

            let mut status_markers = Vec::<String>::new();
            if dep.requirement.is_none() {
                status_markers.push("missing_requirement".to_string());
            }
            if let Some(ref selected_row) = selected {
                if selected_row.yanked {
                    status_markers.push("yanked".to_string());
                    issues.push(DependencyAuditIssue {
                        dependency_name: dep.name.clone(),
                        category: DependencyAuditIssueCategory::Yanked,
                        severity: DependencyAuditSeverity::High,
                        message: "selected dependency version is yanked".to_string(),
                        selected_version: Some(selected_row.version.clone()),
                        latest_version: latest
                            .as_ref()
                            .map(|row| row.version.clone()),
                    });
                }

                if advisory_count > 0 {
                    status_markers.push("has_advisory".to_string());
                    issues.push(DependencyAuditIssue {
                        dependency_name: dep.name.clone(),
                        category: DependencyAuditIssueCategory::Advisory,
                        severity: DependencyAuditSeverity::High,
                        message: format!(
                            "{advisory_count} indexed advisories affect selected version"
                        ),
                        selected_version: Some(selected_row.version.clone()),
                        latest_version: latest
                            .as_ref()
                            .map(|row| row.version.clone()),
                    });
                }

                if let Some(latest_row) = latest.as_ref()
                    && latest_row.version != selected_row.version
                {
                    status_markers.push("outdated".to_string());
                    issues.push(DependencyAuditIssue {
                        dependency_name: dep.name.clone(),
                        category: DependencyAuditIssueCategory::Outdated,
                        severity: DependencyAuditSeverity::Low,
                        message: "newer indexed version is available".to_string(),
                        selected_version: Some(selected_row.version.clone()),
                        latest_version: Some(latest_row.version.clone()),
                    });
                }

                if let Some(package_msrv) = package_rust_floor.as_ref()
                    && let Some(dep_msrv) = selected_row
                        .rust_version
                        .as_deref()
                        .and_then(parse_rust_version_floor)
                    && dep_msrv > *package_msrv
                {
                    status_markers.push("msrv_conflict".to_string());
                    issues.push(DependencyAuditIssue {
                        dependency_name: dep.name.clone(),
                        category: DependencyAuditIssueCategory::MsrvConflict,
                        severity: DependencyAuditSeverity::Medium,
                        message: format!(
                            "dependency MSRV {} exceeds package rust-version {}",
                            selected_row
                                .rust_version
                                .as_deref()
                                .unwrap_or("unknown"),
                            parsed_manifest
                                .package_rust_version
                                .as_deref()
                                .unwrap_or("unknown")
                        ),
                        selected_version: Some(selected_row.version.clone()),
                        latest_version: latest
                            .as_ref()
                            .map(|row| row.version.clone()),
                    });
                }
            }

            if status_markers.is_empty() {
                status_markers.push("ok".to_string());
            }

            dependencies.push(DependencyAuditDependency {
                dependency_name: dep.name,
                requirement: dep.requirement,
                selected_version: selected
                    .as_ref()
                    .map(|row| row.version.clone()),
                latest_version: latest
                    .as_ref()
                    .map(|row| row.version.clone()),
                selected_rust_version: selected
                    .as_ref()
                    .and_then(|row| row.rust_version.clone()),
                yanked: selected
                    .as_ref()
                    .map(|row| row.yanked)
                    .unwrap_or(false),
                advisory_count,
                status_markers,
            });
        }

        dependencies.sort_by(|left, right| {
            left.dependency_name
                .cmp(&right.dependency_name)
        });
        issues.sort_by(|left, right| {
            left.dependency_name
                .cmp(&right.dependency_name)
                .then_with(|| {
                    left.category
                        .cmp(&right.category)
                })
        });

        let confidence_assessment =
            evaluate_confidence(dependencies.len(), unresolved_count, no_requirement_count);

        let display_path = manifest_path
            .strip_prefix(std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf()))
            .ok()
            .map(|value| value.display().to_string())
            .unwrap_or_else(|| {
                manifest_path
                    .display()
                    .to_string()
            });

        Ok(Json(DependencyAuditResponse {
            cargo_toml_path: display_path,
            package_name: parsed_manifest.package_name,
            package_rust_version: parsed_manifest.package_rust_version,
            dependency_count: dependencies.len(),
            issue_count: issues.len(),
            dependencies,
            issues,
            freshness: vec![
                ResponseFreshnessSource {
                    source: "local_manifest".to_string(),
                    status: "read".to_string(),
                    checked_at: None,
                },
                ResponseFreshnessSource {
                    source: "local_postgres_index".to_string(),
                    status: "queried".to_string(),
                    checked_at: None,
                },
            ],
            confidence: confidence_assessment
                .level
                .as_str()
                .to_string(),
            confidence_assessment,
            next_best_calls: vec![
                "crate.intel".to_string(),
                "crate.versions".to_string(),
                "index.refresh".to_string(),
            ],
            provenance: "local_manifest+local_postgres_index".to_string(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_manifest, parse_rust_version_floor, parse_semver_for_matching};

    #[test]
    fn parse_manifest_collects_root_and_target_dependencies() {
        let manifest = r#"
            [package]
            name = "demo"
            rust-version = "1.70"

            [dependencies]
            serde = "1"

            [dev-dependencies]
            tokio = { version = "1" }

            [target.'cfg(unix)'.dependencies]
            libc = "0.2"
        "#;

        let parsed = parse_manifest(manifest).expect("manifest parses");
        let names = parsed
            .dependencies
            .iter()
            .map(|d| d.name.as_str())
            .collect::<Vec<_>>();

        assert!(names.contains(&"serde"));
        assert!(names.contains(&"tokio"));
        assert!(names.contains(&"libc"));
        assert_eq!(parsed.package_name.as_deref(), Some("demo"));
        assert_eq!(
            parsed
                .package_rust_version
                .as_deref(),
            Some("1.70")
        );
    }

    #[test]
    fn parse_rust_version_floor_handles_short_forms() {
        assert_eq!(
            parse_rust_version_floor("1.70")
                .expect("version")
                .to_string(),
            "1.70.0"
        );
        assert_eq!(
            parse_rust_version_floor("1")
                .expect("version")
                .to_string(),
            "1.0.0"
        );
    }

    #[test]
    fn parse_semver_for_matching_accepts_v_prefix() {
        assert!(parse_semver_for_matching("v1.2.3").is_some());
    }
}
