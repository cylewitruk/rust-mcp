use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::server::McpServer;

#[derive(Debug, Deserialize)]
struct OsvQueryResponse {
    #[serde(default)]
    vulns: Vec<OsvVulnerability>,
}

#[derive(Debug, Deserialize)]
struct OsvVulnerability {
    id: String,
    summary: Option<String>,
    details: Option<String>,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    affected: Vec<OsvAffected>,
    #[serde(default)]
    references: Vec<OsvReference>,
    #[serde(default)]
    severity: Vec<OsvSeverity>,
}

#[derive(Debug, Deserialize)]
struct OsvAffected {
    #[serde(default)]
    versions: Vec<String>,
    #[serde(default)]
    ranges: Vec<OsvRange>,
}

#[derive(Debug, Deserialize)]
struct OsvRange {
    #[serde(rename = "type")]
    range_type: String,
    #[serde(default)]
    events: Vec<OsvRangeEvent>,
}

#[derive(Debug, Deserialize)]
struct OsvRangeEvent {
    introduced: Option<String>,
    fixed: Option<String>,
    last_affected: Option<String>,
    limit: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OsvReference {
    url: String,
}

#[derive(Debug, Deserialize)]
struct OsvSeverity {
    #[serde(rename = "type")]
    severity_type: Option<String>,
    score: Option<String>,
}

#[derive(Debug, sqlx::FromRow)]
struct SecurityCrateRow {
    id: i64,
    name: String,
}

#[derive(Debug, sqlx::FromRow)]
struct SecurityVersionRow {
    id: i64,
    version: String,
}

#[derive(Debug, Default, Serialize)]
pub(super) struct SecuritySyncOutcome {
    pub(super) crates_processed: usize,
    pub(super) advisories_written: usize,
    pub(super) errors: Vec<String>,
    pub(super) touched_crates: Vec<String>,
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

impl McpServer {
    async fn query_osv_for_crate(&self, crate_name: &str) -> Result<OsvQueryResponse, String> {
        let response = self
            .state
            .http
            .post("https://api.osv.dev/v1/query")
            .json(&json!({
                "package": {
                    "name": crate_name,
                    "ecosystem": "crates.io"
                }
            }))
            .send()
            .await
            .map_err(|e| format!("OSV query failed for {crate_name}: {e}"))?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<body unavailable>".to_string());
            return Err(format!(
                "OSV query failed for {crate_name} with status {}: {}",
                status, body
            ));
        }

        response
            .json::<OsvQueryResponse>()
            .await
            .map_err(|e| format!("failed to decode OSV response for {crate_name}: {e}"))
    }

    pub(super) async fn sync_osv_security(
        &self,
        limit: u32,
        offset: u32,
    ) -> Result<SecuritySyncOutcome, String> {
        let crate_rows = sqlx::query_as::<_, SecurityCrateRow>(
            "SELECT id, name
             FROM crates
             ORDER BY updated_at DESC NULLS LAST, id DESC
             LIMIT $1 OFFSET $2",
        )
        .bind(i64::from(limit))
        .bind(i64::from(offset))
        .fetch_all(&self.state.db)
        .await
        .map_err(|e| format!("failed to fetch crates for security sync: {e}"))?;

        let mut outcome = SecuritySyncOutcome::default();

        for krate in crate_rows {
            let version_rows = sqlx::query_as::<_, SecurityVersionRow>(
                "SELECT id, version
                 FROM crate_versions
                 WHERE crate_id = $1",
            )
            .bind(krate.id)
            .fetch_all(&self.state.db)
            .await
            .map_err(|e| format!("failed to load versions for {}: {e}", krate.name))?;

            let version_map = version_rows
                .iter()
                .map(|v| (v.version.clone(), v.id))
                .collect::<HashMap<_, _>>();

            let osv = match self
                .query_osv_for_crate(&krate.name)
                .await
            {
                Ok(value) => value,
                Err(error) => {
                    outcome.errors.push(error);
                    continue;
                }
            };

            outcome.crates_processed += 1;
            outcome
                .touched_crates
                .push(krate.name.clone());

            sqlx::query(
                "DELETE FROM advisory_matches
                 WHERE crate_id = $1 AND source IN ('osv', 'rustsec_osv')",
            )
            .bind(krate.id)
            .execute(&self.state.db)
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
                            sqlx::query(
                                "INSERT INTO advisory_matches (
                                    crate_id, version_id, advisory_id, severity, title, url,
                                                affected_range, fixed_versions, source, created_at
                                 ) VALUES (
                                                $1, $2, $3, $4, $5, $6, $7, $8, $9, NOW()
                                 )
                                 ON CONFLICT (crate_id, version_id, advisory_id)
                                 DO UPDATE SET
                                    severity = EXCLUDED.severity,
                                    title = EXCLUDED.title,
                                    url = EXCLUDED.url,
                                    affected_range = EXCLUDED.affected_range,
                                    fixed_versions = EXCLUDED.fixed_versions,
                                    source = EXCLUDED.source,
                                    created_at = NOW()",
                            )
                            .bind(krate.id)
                            .bind(version_id)
                            .bind(&advisory_id)
                            .bind(severity.as_deref())
                            .bind(&title)
                            .bind(url.as_deref())
                            .bind(&affected_range)
                            .bind(fixed_versions_json.clone())
                            .bind(&advisory_source)
                            .execute(&self.state.db)
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
                    sqlx::query(
                        "INSERT INTO advisory_matches (
                            crate_id, version_id, advisory_id, severity, title, url,
                            affected_range, fixed_versions, source, created_at
                         ) VALUES (
                                     $1, NULL, $2, $3, $4, $5, $6, $7, $8, NOW()
                         )",
                    )
                    .bind(krate.id)
                    .bind(&advisory_id)
                    .bind(severity.as_deref())
                    .bind(&title)
                    .bind(url.as_deref())
                    .bind(&affected_range)
                    .bind(fixed_versions_json)
                    .bind(&advisory_source)
                    .execute(&self.state.db)
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
}
