use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tracing::{debug, info};

use crate::db::indexing::{
    clear_osv_advisory_matches, clear_rustsec_db_advisory_matches, fetch_security_crates_page,
    fetch_security_versions_for_crate, insert_crate_level_advisory_match,
    upsert_version_advisory_match,
};
use crate::db::models::{CrateLevelAdvisoryMatchInsert, VersionAdvisoryMatchInsert};
use crate::integration::osv::{
    OsvAffected, OsvDevClient, OsvQueryResponse, OsvSeverity, OsvVulnerability,
};
use crate::mcp::server::McpServer;

/// Outcome of a security advisory synchronization.
#[derive(Debug, Default, Serialize)]
pub struct SecuritySyncOutcome {
    /// Number of crates whose advisories were checked.
    pub crates_processed: usize,
    /// Number of advisory records written to the database.
    pub advisories_written: usize,
    /// Errors encountered during the sync, one per failed operation.
    pub errors: Vec<String>,
    /// Names of crates that had advisory data updated.
    pub touched_crates: Vec<String>,
}

impl SecuritySyncOutcome {
    /// Merges another outcome into this one.
    pub fn merge(&mut self, mut other: SecuritySyncOutcome) {
        self.crates_processed += other.crates_processed;
        self.advisories_written += other.advisories_written;
        self.errors
            .append(&mut other.errors);
        self.touched_crates
            .append(&mut other.touched_crates);
        self.touched_crates.sort();
        self.touched_crates.dedup();
    }
}

#[derive(Debug, Default, Deserialize)]
struct RustSecFrontmatter {
    #[serde(default)]
    advisory: RustSecAdvisory,
    #[serde(default)]
    versions: RustSecVersions,
}

#[derive(Debug, Default, Deserialize)]
struct RustSecAdvisory {
    id: Option<String>,
    package: Option<String>,
    title: Option<String>,
    description: Option<String>,
    url: Option<String>,
    cvss: Option<String>,
    withdrawn: Option<String>,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    categories: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct RustSecVersions {
    #[serde(default)]
    patched: Vec<String>,
    #[serde(default)]
    unaffected: Vec<String>,
}

fn affected_range_text(affected: &[OsvAffected]) -> String {
    let mut ranges = Vec::new();
    for item in affected {
        for range in &item.ranges {
            let mut parts = Vec::new();
            for event in &range.events {
                if let Some(v) = &event.introduced {
                    parts.push(format!("introduced={v}"));
                }
                if let Some(v) = &event.fixed {
                    parts.push(format!("fixed={v}"));
                }
                if let Some(v) = &event.last_affected {
                    parts.push(format!("last_affected={v}"));
                }
                if let Some(v) = &event.limit {
                    parts.push(format!("limit={v}"));
                }
            }
            let range_text = if parts.is_empty() {
                range.range_type.clone()
            } else {
                format!("{}({})", range.range_type, parts.join(","))
            };
            ranges.push(range_text);
        }
    }

    if ranges.is_empty() { "osv_affected_unknown".to_string() } else { ranges.join("; ") }
}

fn fixed_versions(affected: &[OsvAffected]) -> Value {
    let mut versions = Vec::<String>::new();
    for item in affected {
        for range in &item.ranges {
            for event in &range.events {
                if let Some(v) = &event.fixed {
                    versions.push(v.clone());
                }
            }
        }
    }
    versions.sort();
    versions.dedup();
    Value::Array(
        versions
            .into_iter()
            .map(Value::String)
            .collect(),
    )
}

fn first_rustsec_alias(aliases: &[String]) -> Option<String> {
    aliases
        .iter()
        .find(|alias| alias.starts_with("RUSTSEC-"))
        .cloned()
}

fn advisory_identity(vuln: &OsvVulnerability) -> (String, String) {
    if let Some(rustsec_id) = first_rustsec_alias(&vuln.aliases) {
        return (rustsec_id, "rustsec_osv".to_string());
    }
    (vuln.id.clone(), "osv".to_string())
}

fn extract_first_float(text: &str) -> Option<f64> {
    let mut token = String::new();
    for ch in text.chars() {
        if ch.is_ascii_digit() || ch == '.' {
            token.push(ch);
        } else if !token.is_empty() {
            break;
        }
    }
    if token.is_empty() { None } else { token.parse::<f64>().ok() }
}

fn normalize_severity_label(severity: &[OsvSeverity]) -> Option<String> {
    for item in severity {
        if let Some(score_text) = item.score.as_deref()
            && let Some(score) = extract_first_float(score_text)
        {
            let normalized = if score >= 9.0 {
                "critical"
            } else if score >= 7.0 {
                "high"
            } else if score >= 4.0 {
                "medium"
            } else if score > 0.0 {
                "low"
            } else {
                "unknown"
            };
            return Some(normalized.to_string());
        }

        if let Some(kind) = item
            .severity_type
            .as_deref()
            .map(|v| v.to_ascii_lowercase())
            && (kind.contains("critical")
                || kind.contains("high")
                || kind.contains("medium")
                || kind.contains("low"))
        {
            return Some(kind);
        }
    }

    None
}

fn split_toml_frontmatter(markdown: &str) -> Option<(&str, &str)> {
    let mut lines = markdown.lines();
    let first = lines.next()?.trim();
    if first != "+++" && first != "---" {
        return None;
    }

    let offset = markdown.find('\n')? + 1;
    let tail = &markdown[offset..];
    let end_marker = format!("\n{}\n", first);
    let end_idx = tail.find(&end_marker)?;
    let frontmatter = &tail[..end_idx];
    let body_start = end_idx + end_marker.len();
    let body = &tail[body_start..];
    Some((frontmatter, body))
}

fn rustsec_title_from_body(body: &str) -> Option<String> {
    body.lines()
        .map(str::trim)
        .find_map(|line| {
            if line.starts_with('#') {
                let title = line
                    .trim_start_matches('#')
                    .trim();
                if title.is_empty() { None } else { Some(title.to_string()) }
            } else {
                None
            }
        })
}

fn parse_semver_for_matching(version: &str) -> Option<Version> {
    Version::parse(version)
        .ok()
        .or_else(|| Version::parse(version.trim_start_matches('v')).ok())
}

fn parse_version_reqs(requirements: &[String]) -> Vec<VersionReq> {
    requirements
        .iter()
        .filter_map(|req| VersionReq::parse(req).ok())
        .collect()
}

fn is_version_affected(
    version: &str,
    patched_reqs: &[VersionReq],
    unaffected_reqs: &[VersionReq],
) -> Option<bool> {
    let parsed = parse_semver_for_matching(version)?;

    if patched_reqs.is_empty() && unaffected_reqs.is_empty() {
        return Some(true);
    }

    let patched = patched_reqs
        .iter()
        .any(|req| req.matches(&parsed));
    let unaffected = unaffected_reqs
        .iter()
        .any(|req| req.matches(&parsed));
    Some(!(patched || unaffected))
}

fn advisory_markdown_files(crate_dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(crate_dir) else {
        return Vec::new();
    };

    let mut files = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.eq_ignore_ascii_case("md"))
                    .unwrap_or(false)
        })
        .collect::<Vec<_>>();

    files.sort();
    files
}

impl McpServer {
    /// Synchronizes OSV vulnerability data for indexed crates.
    pub async fn sync_osv_security(
        &self,
        limit: u32,
        offset: u32,
    ) -> Result<SecuritySyncOutcome, String> {
        let crate_rows =
            fetch_security_crates_page(&self.state.db, i64::from(limit), i64::from(offset))
                .await
                .map_err(|e| format!("failed to fetch crates for security sync: {e}"))?;

        let mut outcome = SecuritySyncOutcome::default();
        let osv_client = OsvDevClient::new(&self.state);

        for krate in crate_rows {
            info!(crate_name = %krate.name, "querying OSV for security advisories");

            let version_rows = fetch_security_versions_for_crate(&self.state.db, krate.id)
                .await
                .map_err(|e| format!("failed to load versions for {}: {e}", krate.name))?;

            let version_map = version_rows
                .iter()
                .map(|v| (v.version.clone(), v.id))
                .collect::<HashMap<_, _>>();

            let osv: OsvQueryResponse = match osv_client
                .query_crate(&krate.name)
                .await
            {
                Ok(value) => value,
                Err(error) => {
                    outcome.errors.push(error);
                    continue;
                }
            };

            debug!(
                crate_name = %krate.name,
                vulnerabilities = osv.vulns.len(),
                indexed_versions = version_map.len(),
                "OSV query returned"
            );

            outcome.crates_processed += 1;
            outcome
                .touched_crates
                .push(krate.name.clone());

            clear_osv_advisory_matches(&self.state.db, krate.id)
                .await
                .map_err(|e| format!("failed to clear OSV advisories for {}: {e}", krate.name))?;

            for vuln in osv.vulns {
                let (advisory_id, advisory_source) = advisory_identity(&vuln);
                let title = vuln
                    .summary
                    .clone()
                    .or(vuln.details.clone())
                    .or_else(|| vuln.aliases.first().cloned())
                    .unwrap_or_else(|| vuln.id.clone());
                let severity = normalize_severity_label(&vuln.severity);
                let url = vuln
                    .references
                    .first()
                    .map(|r| r.url.clone());
                let affected_range = affected_range_text(&vuln.affected);
                let fixed_versions_json = fixed_versions(&vuln.affected);

                let mut matched_any = false;
                for affected in &vuln.affected {
                    for version in &affected.versions {
                        if let Some(version_id) = version_map
                            .get(version)
                            .copied()
                        {
                            matched_any = true;
                            let insert = VersionAdvisoryMatchInsert {
                                crate_id: krate.id,
                                version_id,
                                advisory_id: &advisory_id,
                                severity: severity.as_deref(),
                                title: &title,
                                url: url.as_deref(),
                                affected_range: &affected_range,
                                fixed_versions: &fixed_versions_json,
                                source: &advisory_source,
                            };
                            upsert_version_advisory_match(&self.state.db, &insert)
                                .await
                                .map_err(|e| {
                                    format!(
                                        "failed to upsert advisory {} for {}@{}: {e}",
                                        vuln.id, krate.name, version
                                    )
                                })?;
                            outcome.advisories_written += 1;
                        }
                    }
                }

                if !matched_any {
                    let insert = CrateLevelAdvisoryMatchInsert {
                        crate_id: krate.id,
                        advisory_id: &advisory_id,
                        severity: severity.as_deref(),
                        title: &title,
                        url: url.as_deref(),
                        affected_range: &affected_range,
                        fixed_versions: &fixed_versions_json,
                        source: &advisory_source,
                    };
                    insert_crate_level_advisory_match(&self.state.db, &insert)
                        .await
                        .map_err(|e| {
                            format!(
                                "failed to insert crate-level advisory {} for {}: {e}",
                                vuln.id, krate.name
                            )
                        })?;
                    outcome.advisories_written += 1;
                }
            }
        }

        Ok(outcome)
    }

    /// Synchronizes RustSec advisory-db data for indexed crates.
    pub async fn sync_rustsec_db_security(
        &self,
        limit: u32,
        offset: u32,
    ) -> Result<SecuritySyncOutcome, String> {
        let Some(db_dir) = self
            .state
            .config
            .rustsec_db_dir
            .as_deref()
        else {
            return Ok(SecuritySyncOutcome::default());
        };

        let crate_rows =
            fetch_security_crates_page(&self.state.db, i64::from(limit), i64::from(offset))
                .await
                .map_err(|e| format!("failed to fetch crates for RustSec DB sync: {e}"))?;

        let mut outcome = SecuritySyncOutcome::default();

        for krate in crate_rows {
            let crate_advisories_dir = db_dir
                .join("crates")
                .join(&krate.name);
            if !crate_advisories_dir.exists() {
                continue;
            }

            let version_rows = fetch_security_versions_for_crate(&self.state.db, krate.id)
                .await
                .map_err(|e| format!("failed to load versions for {}: {e}", krate.name))?;

            clear_rustsec_db_advisory_matches(&self.state.db, krate.id)
                .await
                .map_err(|e| {
                    format!("failed to clear RustSec advisories for {}: {e}", krate.name)
                })?;

            let advisory_files = advisory_markdown_files(&crate_advisories_dir);
            if advisory_files.is_empty() {
                continue;
            }

            info!(
                crate_name = %krate.name,
                advisory_files = advisory_files.len(),
                "scanning RustSec DB advisories"
            );

            outcome.crates_processed += 1;
            outcome
                .touched_crates
                .push(krate.name.clone());

            for advisory_file in advisory_files {
                let raw = match fs::read_to_string(&advisory_file) {
                    Ok(value) => value,
                    Err(error) => {
                        outcome.errors.push(format!(
                            "failed reading advisory file {}: {error}",
                            advisory_file.display()
                        ));
                        continue;
                    }
                };

                let Some((frontmatter, body)) = split_toml_frontmatter(&raw) else {
                    outcome
                        .errors
                        .push(format!("missing TOML frontmatter in {}", advisory_file.display()));
                    continue;
                };

                let parsed: RustSecFrontmatter = match toml::from_str(frontmatter) {
                    Ok(value) => value,
                    Err(error) => {
                        outcome.errors.push(format!(
                            "failed parsing TOML frontmatter in {}: {error}",
                            advisory_file.display()
                        ));
                        continue;
                    }
                };

                let advisory_id = parsed
                    .advisory
                    .id
                    .clone()
                    .or_else(|| {
                        advisory_file
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .map(ToString::to_string)
                    })
                    .unwrap_or_else(|| "RUSTSEC-UNKNOWN".to_string());

                if let Some(package) = parsed
                    .advisory
                    .package
                    .as_deref()
                    && package != krate.name
                {
                    continue;
                }

                let title = parsed
                    .advisory
                    .title
                    .clone()
                    .or_else(|| rustsec_title_from_body(body))
                    .or_else(|| {
                        parsed
                            .advisory
                            .description
                            .clone()
                    })
                    .unwrap_or_else(|| advisory_id.clone());

                let severity = parsed
                    .advisory
                    .cvss
                    .as_deref()
                    .and_then(extract_first_float)
                    .map(|score| {
                        if score >= 9.0 {
                            "critical"
                        } else if score >= 7.0 {
                            "high"
                        } else if score >= 4.0 {
                            "medium"
                        } else if score > 0.0 {
                            "low"
                        } else {
                            "unknown"
                        }
                        .to_string()
                    });

                let mut url = parsed.advisory.url.clone();
                if url.is_none() {
                    url = parsed
                        .advisory
                        .aliases
                        .iter()
                        .find(|alias| alias.starts_with("https://"))
                        .cloned();
                }

                let affected_range = {
                    let patched = if parsed
                        .versions
                        .patched
                        .is_empty()
                    {
                        String::new()
                    } else {
                        format!(
                            "patched={}",
                            parsed
                                .versions
                                .patched
                                .join(" | ")
                        )
                    };
                    let unaffected = if parsed
                        .versions
                        .unaffected
                        .is_empty()
                    {
                        String::new()
                    } else {
                        format!(
                            "unaffected={}",
                            parsed
                                .versions
                                .unaffected
                                .join(" | ")
                        )
                    };
                    let withdrawn = parsed
                        .advisory
                        .withdrawn
                        .as_ref()
                        .map(|value| format!("withdrawn={value}"))
                        .unwrap_or_default();
                    let categories = if parsed
                        .advisory
                        .categories
                        .is_empty()
                    {
                        String::new()
                    } else {
                        format!(
                            "categories={}",
                            parsed
                                .advisory
                                .categories
                                .join("|")
                        )
                    };

                    [patched, unaffected, withdrawn, categories]
                        .into_iter()
                        .filter(|value| !value.is_empty())
                        .collect::<Vec<_>>()
                        .join("; ")
                };

                let patched_reqs = parse_version_reqs(&parsed.versions.patched);
                let unaffected_reqs = parse_version_reqs(&parsed.versions.unaffected);
                let fixed_versions_json = json!(parsed.versions.patched);

                let mut matched_any = false;
                for version_row in &version_rows {
                    let Some(affected) =
                        is_version_affected(&version_row.version, &patched_reqs, &unaffected_reqs)
                    else {
                        continue;
                    };

                    if !affected {
                        continue;
                    }

                    matched_any = true;
                    let insert = VersionAdvisoryMatchInsert {
                        crate_id: krate.id,
                        version_id: version_row.id,
                        advisory_id: &advisory_id,
                        severity: severity.as_deref(),
                        title: &title,
                        url: url.as_deref(),
                        affected_range: &affected_range,
                        fixed_versions: &fixed_versions_json,
                        source: "rustsec_db",
                    };
                    upsert_version_advisory_match(&self.state.db, &insert)
                        .await
                        .map_err(|e| {
                            format!(
                                "failed to upsert RustSec advisory {} for {}@{}: {e}",
                                advisory_id, krate.name, version_row.version
                            )
                        })?;
                    outcome.advisories_written += 1;
                }

                if !matched_any {
                    let insert = CrateLevelAdvisoryMatchInsert {
                        crate_id: krate.id,
                        advisory_id: &advisory_id,
                        severity: severity.as_deref(),
                        title: &title,
                        url: url.as_deref(),
                        affected_range: &affected_range,
                        fixed_versions: &fixed_versions_json,
                        source: "rustsec_db",
                    };
                    insert_crate_level_advisory_match(&self.state.db, &insert)
                        .await
                        .map_err(|e| {
                            format!(
                                "failed to insert crate-level RustSec advisory {} for {}: {e}",
                                advisory_id, krate.name
                            )
                        })?;
                    outcome.advisories_written += 1;
                }
            }
        }

        outcome.touched_crates.sort();
        outcome.touched_crates.dedup();
        Ok(outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rustsec_frontmatter_fixture_parses_expected_fields() {
        let fixture = r#"+++
[advisory]
id = "RUSTSEC-2024-9999"
package = "demo-crate"
title = "Demo advisory"
cvss = "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H 9.8"
withdrawn = "2025-01-15"
aliases = ["CVE-2024-9999", "https://rustsec.org/advisories/RUSTSEC-2024-9999"]
categories = ["memory-corruption", "soundness"]

[versions]
patched = [">=1.2.0"]
unaffected = ["<0.8.0"]
+++

# Demo advisory body

Some details.
"#;

        let (frontmatter, body) =
            split_toml_frontmatter(fixture).expect("frontmatter should be extracted");
        let parsed: RustSecFrontmatter =
            toml::from_str(frontmatter).expect("frontmatter should parse as TOML");

        assert_eq!(parsed.advisory.id.as_deref(), Some("RUSTSEC-2024-9999"));
        assert_eq!(
            parsed
                .advisory
                .package
                .as_deref(),
            Some("demo-crate")
        );
        assert_eq!(
            parsed
                .advisory
                .withdrawn
                .as_deref(),
            Some("2025-01-15")
        );
        assert_eq!(
            parsed
                .advisory
                .categories
                .len(),
            2
        );
        assert_eq!(parsed.versions.patched, vec![">=1.2.0"]);
        assert_eq!(parsed.versions.unaffected, vec!["<0.8.0"]);
        assert_eq!(rustsec_title_from_body(body).as_deref(), Some("Demo advisory body"));

        let patched_reqs = parse_version_reqs(&parsed.versions.patched);
        let unaffected_reqs = parse_version_reqs(&parsed.versions.unaffected);

        assert_eq!(is_version_affected("1.0.0", &patched_reqs, &unaffected_reqs), Some(true));
        assert_eq!(is_version_affected("1.2.0", &patched_reqs, &unaffected_reqs), Some(false));
        assert_eq!(is_version_affected("0.7.9", &patched_reqs, &unaffected_reqs), Some(false));
    }
}
