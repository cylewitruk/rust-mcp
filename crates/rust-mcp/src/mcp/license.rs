use std::collections::BTreeSet;

use rmcp::Json;

use super::models::{
    ConfidenceAssessment, ConfidenceLevel, CrateCoreRow, CrateLicenseCheckRequest,
    CrateLicenseCheckResponse, CrateVersionLicenseRow, LicensePolicyResult,
    ResponseFreshnessSource,
};
use super::server::McpServer;
use super::utils::{normalize_optional, normalize_required};

fn normalize_policy_list(values: Option<Vec<String>>) -> Vec<String> {
    let mut out = values
        .unwrap_or_default()
        .into_iter()
        .map(|value| {
            value
                .trim()
                .to_ascii_uppercase()
        })
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    out.sort();
    out.dedup();
    out
}

fn extract_license_identifiers(expression: &str) -> Vec<String> {
    let mut identifiers = BTreeSet::<String>::new();
    let mut token = String::new();

    for ch in expression.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '.' | '+' | ':') {
            token.push(ch);
            continue;
        }

        if !token.is_empty() {
            let upper = token.to_ascii_uppercase();
            if upper != "AND" && upper != "OR" && upper != "WITH" {
                identifiers.insert(token.clone());
            }
            token.clear();
        }
    }

    if !token.is_empty() {
        let upper = token.to_ascii_uppercase();
        if upper != "AND" && upper != "OR" && upper != "WITH" {
            identifiers.insert(token);
        }
    }

    identifiers
        .into_iter()
        .collect()
}

fn evaluate_policy(
    matched_licenses: &[String],
    allow_licenses: &[String],
    deny_licenses: &[String],
    has_expression: bool,
) -> (LicensePolicyResult, Vec<String>) {
    if !has_expression {
        return (
            LicensePolicyResult::Unknown,
            vec!["license expression is missing for selected version".to_string()],
        );
    }

    let matched_upper = matched_licenses
        .iter()
        .map(|item| item.to_ascii_uppercase())
        .collect::<BTreeSet<_>>();

    let denied_matches = deny_licenses
        .iter()
        .filter(|denied| matched_upper.contains(&denied.to_ascii_uppercase()))
        .cloned()
        .collect::<Vec<_>>();
    if !denied_matches.is_empty() {
        return (
            LicensePolicyResult::Denied,
            vec![format!("matched denied licenses: {}", denied_matches.join(", "))],
        );
    }

    if !allow_licenses.is_empty() {
        let allowed_matches = allow_licenses
            .iter()
            .filter(|allowed| matched_upper.contains(&allowed.to_ascii_uppercase()))
            .cloned()
            .collect::<Vec<_>>();

        if allowed_matches.is_empty() {
            return (
                LicensePolicyResult::Denied,
                vec!["no matched license is present in allow_licenses".to_string()],
            );
        }

        return (
            LicensePolicyResult::Allowed,
            vec![format!("matched allowed licenses: {}", allowed_matches.join(", "))],
        );
    }

    (
        LicensePolicyResult::Allowed,
        vec!["policy lists not provided; license metadata is informational".to_string()],
    )
}

impl McpServer {
    pub(super) async fn handle_crate_license_check(
        &self,
        request: CrateLicenseCheckRequest,
    ) -> Result<Json<CrateLicenseCheckResponse>, String> {
        let crate_name = normalize_required(request.crate_name, "crate_name")?;
        let requested_version = normalize_optional(request.version);
        let allow_licenses = normalize_policy_list(request.allow_licenses);
        let deny_licenses = normalize_policy_list(request.deny_licenses);

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

        let latest_version_row = sqlx::query_as::<_, CrateVersionLicenseRow>(
            "SELECT
                version,
                license_expression
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
                &latest_version_row.version,
            )
            .await?;

        let latest_version_row = if freshness_outcome.freshness_check_result == "changed" {
            sqlx::query_as::<_, CrateVersionLicenseRow>(
                "SELECT
                    version,
                    license_expression
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
            latest_version_row
        };

        let mut refresh_enqueued = freshness_outcome.refresh_enqueued;
        let mut refresh_job_id = freshness_outcome
            .refresh_job_id
            .clone();

        let selected_version = if let Some(version) = requested_version {
            let selected = sqlx::query_as::<_, CrateVersionLicenseRow>(
                "SELECT
                    version,
                    license_expression
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

                sqlx::query_as::<_, CrateVersionLicenseRow>(
                    "SELECT
                        version,
                        license_expression
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
            latest_version_row.clone()
        };

        let matched_licenses = selected_version
            .license_expression
            .as_deref()
            .map(extract_license_identifiers)
            .unwrap_or_default();

        let (policy_result, policy_reasons) = evaluate_policy(
            &matched_licenses,
            &allow_licenses,
            &deny_licenses,
            selected_version
                .license_expression
                .is_some(),
        );

        let confidence_assessment = if selected_version
            .license_expression
            .is_none()
        {
            ConfidenceAssessment {
                level: ConfidenceLevel::Low,
                reason: "no license metadata available for selected crate version".to_string(),
            }
        } else if matched_licenses.is_empty() {
            ConfidenceAssessment {
                level: ConfidenceLevel::Medium,
                reason: "license expression was present but no SPDX-like identifiers were parsed"
                    .to_string(),
            }
        } else {
            ConfidenceAssessment {
                level: ConfidenceLevel::High,
                reason: "license expression and SPDX-like identifiers were parsed".to_string(),
            }
        };

        let freshness_check_result = freshness_outcome
            .freshness_check_result
            .clone();

        Ok(Json(CrateLicenseCheckResponse {
            crate_name: crate_row.name,
            selected_version: selected_version.version,
            latest_version: latest_version_row.version,
            license_expression: selected_version.license_expression,
            matched_licenses,
            allow_licenses,
            deny_licenses,
            policy_result,
            policy_reasons,
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
                "crate.intel".to_string(),
                "crate.versions".to_string(),
                "index.refresh".to_string(),
            ],
            provenance: "local_postgres_index".to_string(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::{LicensePolicyResult, evaluate_policy, extract_license_identifiers};

    #[test]
    fn parses_spdx_identifiers_from_expression() {
        let values = extract_license_identifiers("(MIT OR Apache-2.0) WITH LLVM-exception");
        assert_eq!(
            values,
            vec!["Apache-2.0".to_string(), "LLVM-exception".to_string(), "MIT".to_string(),]
        );
    }

    #[test]
    fn deny_list_precedence_over_allow_list() {
        let (result, reasons) = evaluate_policy(
            &["MIT".to_string(), "Apache-2.0".to_string()],
            &["MIT".to_string()],
            &["Apache-2.0".to_string()],
            true,
        );

        assert!(matches!(result, LicensePolicyResult::Denied));
        assert!(reasons[0].contains("denied licenses"));
    }

    #[test]
    fn missing_expression_returns_unknown() {
        let (result, reasons) = evaluate_policy(&[], &[], &[], false);
        assert!(matches!(result, LicensePolicyResult::Unknown));
        assert!(reasons[0].contains("missing"));
    }
}
