use std::collections::BTreeSet;

use rmcp::Json;

use super::models::{
    AlternativesCandidateRow, ConfidenceAssessment, ConfidenceLevel, CrateAlternativeHit,
    CrateAlternativesRequest, CrateAlternativesResponse, CrateCoreRow, CrateVersionSelectionRow,
    LicensePolicyResult, ResponseFreshnessSource,
};
use super::server::McpServer;
use super::utils::{alternatives_limit, normalize_optional, normalize_required};

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

fn overlap_count(left: &[String], right: &[String]) -> usize {
    let left = left
        .iter()
        .map(|value| value.to_ascii_lowercase())
        .collect::<BTreeSet<_>>();
    let right = right
        .iter()
        .map(|value| value.to_ascii_lowercase())
        .collect::<BTreeSet<_>>();
    left.intersection(&right)
        .count()
}

fn evaluate_policy(
    matched_licenses: &[String],
    allow_licenses: &[String],
    deny_licenses: &[String],
) -> (LicensePolicyResult, Vec<String>) {
    if matched_licenses.is_empty() {
        return (
            LicensePolicyResult::Unknown,
            vec!["license metadata missing or unparsable".to_string()],
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

fn score_candidate(
    candidate: &AlternativesCandidateRow,
    category_overlap: usize,
    keyword_overlap: usize,
    policy_result: LicensePolicyResult,
) -> (f64, Vec<String>) {
    let mut score = 0.0_f64;
    let mut reasons = Vec::<String>::new();

    let name_component = (candidate
        .name_similarity
        .max(0.0)
        * 4.0)
        .min(4.0);
    score += name_component;
    if name_component > 0.0 {
        reasons.push(format!("name_similarity={:.3}", candidate.name_similarity));
    }

    if category_overlap > 0 {
        let category_component = (category_overlap as f64) * 1.5;
        score += category_component;
        reasons.push(format!("category_overlap={category_overlap}"));
    }

    if keyword_overlap > 0 {
        let keyword_component = keyword_overlap as f64;
        score += keyword_component;
        reasons.push(format!("keyword_overlap={keyword_overlap}"));
    }

    let adoption_component = ((candidate.total_downloads as f64) + 1.0).ln() * 0.35;
    score += adoption_component;
    reasons.push(format!("downloads={}", candidate.total_downloads));

    let dependents_component = ((candidate.dependent_count as f64) + 1.0).ln() * 0.45;
    score += dependents_component;
    reasons.push(format!("dependents={}", candidate.dependent_count));

    if candidate.advisory_count > 0 {
        let penalty = (candidate.advisory_count as f64) * 1.25;
        score -= penalty;
        reasons.push(format!("advisory_penalty=-{penalty:.2}"));
    }

    if candidate.yanked {
        score -= 5.0;
        reasons.push("yanked_penalty=-5.00".to_string());
    }

    match policy_result {
        LicensePolicyResult::Denied => {
            score -= 8.0;
            reasons.push("license_policy_penalty=-8.00".to_string());
        }
        LicensePolicyResult::Unknown => {
            score -= 1.0;
            reasons.push("license_unknown_penalty=-1.00".to_string());
        }
        LicensePolicyResult::Allowed => {}
    }

    (score, reasons)
}

impl McpServer {
    pub(super) async fn handle_crate_alternatives(
        &self,
        request: CrateAlternativesRequest,
    ) -> Result<Json<CrateAlternativesResponse>, String> {
        let crate_name = normalize_required(request.crate_name, "crate_name")?;
        let requested_version = normalize_optional(request.version);
        let limit = alternatives_limit(request.limit);
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

        let candidates = sqlx::query_as::<_, AlternativesCandidateRow>(
            "SELECT
                c.name AS crate_name,
                c.description,
                c.categories,
                c.keywords,
                lv.version AS latest_version,
                COALESCE(lv.total_downloads, 0)::BIGINT AS total_downloads,
                COALESCE(lv.yanked, false) AS yanked,
                COALESCE(lv.advisory_count, 0)::BIGINT AS advisory_count,
                lv.license_expression,
                COALESCE(dep.dependent_count, 0)::BIGINT AS dependent_count,
                similarity(c.name, $2)::DOUBLE PRECISION AS name_similarity
             FROM crates c
             LEFT JOIN LATERAL (
                 SELECT
                     cv.version,
                     cv.total_downloads,
                     cv.yanked,
                     cv.license_expression,
                     COUNT(am.id)::BIGINT AS advisory_count
                 FROM crate_versions cv
                 LEFT JOIN advisory_matches am ON am.version_id = cv.id
                 WHERE cv.crate_id = c.id
                 GROUP BY cv.id
                 ORDER BY cv.published_at DESC NULLS LAST, cv.id DESC
                 LIMIT 1
             ) lv ON true
             LEFT JOIN LATERAL (
                 SELECT COALESCE(COUNT(DISTINCT cv_from.crate_id), 0)::BIGINT AS dependent_count
                 FROM dependency_edges d
                 JOIN crate_versions cv_from ON cv_from.id = d.from_version_id
                 WHERE d.to_crate_id = c.id
             ) dep ON true
             WHERE c.id <> $1
               AND (
                   similarity(c.name, $2) >= 0.10
                   OR c.categories && $3::TEXT[]
                   OR c.keywords && $4::TEXT[]
               )
             ORDER BY
                similarity(c.name, $2) DESC,
                COALESCE(lv.total_downloads, 0) DESC,
                c.name ASC
             LIMIT $5",
        )
        .bind(crate_row.id)
        .bind(&crate_row.name)
        .bind(&crate_row.categories)
        .bind(&crate_row.keywords)
        .bind(i64::from(limit * 8))
        .fetch_all(&self.state.db)
        .await
        .map_err(|e| format!("crate.alternatives candidate query failed: {e}"))?;

        let mut alternatives = candidates
            .into_iter()
            .filter_map(|candidate| {
                let matched_licenses = candidate
                    .license_expression
                    .as_deref()
                    .map(extract_license_identifiers)
                    .unwrap_or_default();

                let (policy_result, policy_reasons) =
                    evaluate_policy(&matched_licenses, &allow_licenses, &deny_licenses);

                if matches!(policy_result, LicensePolicyResult::Denied)
                    && (!allow_licenses.is_empty() || !deny_licenses.is_empty())
                {
                    return None;
                }

                let category_overlap = overlap_count(&crate_row.categories, &candidate.categories);
                let keyword_overlap = overlap_count(&crate_row.keywords, &candidate.keywords);
                let (score, rank_reasons) =
                    score_candidate(&candidate, category_overlap, keyword_overlap, policy_result);

                Some(CrateAlternativeHit {
                    crate_name: candidate.crate_name,
                    latest_version: candidate.latest_version,
                    description: candidate.description,
                    categories: candidate.categories,
                    keywords: candidate.keywords,
                    total_downloads: candidate.total_downloads,
                    dependent_crates: candidate.dependent_count,
                    advisory_count: candidate.advisory_count,
                    yanked: candidate.yanked,
                    license_expression: candidate.license_expression,
                    policy_result,
                    policy_reasons,
                    score,
                    rank_reasons,
                })
            })
            .collect::<Vec<_>>();

        alternatives.sort_by(|left, right| {
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    right
                        .total_downloads
                        .cmp(&left.total_downloads)
                })
                .then_with(|| {
                    left.crate_name
                        .cmp(&right.crate_name)
                })
        });
        alternatives.truncate(limit as usize);

        let confidence_assessment = if alternatives.is_empty() {
            ConfidenceAssessment {
                level: ConfidenceLevel::Low,
                reason: "no alternatives matched similarity, taxonomy, and policy constraints"
                    .to_string(),
            }
        } else if crate_row
            .categories
            .is_empty()
            && crate_row.keywords.is_empty()
        {
            ConfidenceAssessment {
                level: ConfidenceLevel::Medium,
                reason: "ranking relied mostly on name similarity and adoption signals".to_string(),
            }
        } else {
            ConfidenceAssessment {
                level: ConfidenceLevel::High,
                reason: "ranking combined similarity, taxonomy overlap, adoption, and risk signals"
                    .to_string(),
            }
        };

        let freshness_check_result = freshness_outcome
            .freshness_check_result
            .clone();

        Ok(Json(CrateAlternativesResponse {
            crate_name: crate_row.name,
            selected_version: selected_version.version,
            latest_version: latest_version.version,
            limit,
            count: alternatives.len(),
            allow_licenses,
            deny_licenses,
            alternatives,
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
                "crate.license_check".to_string(),
                "crate.graph".to_string(),
            ],
            provenance: "local_postgres_index".to_string(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::{LicensePolicyResult, evaluate_policy, extract_license_identifiers, overlap_count};

    #[test]
    fn parses_spdx_identifiers() {
        let values = extract_license_identifiers("(MIT OR Apache-2.0) WITH LLVM-exception");
        assert_eq!(
            values,
            vec!["Apache-2.0".to_string(), "LLVM-exception".to_string(), "MIT".to_string(),]
        );
    }

    #[test]
    fn overlap_count_is_case_insensitive() {
        let count = overlap_count(
            &["Async".to_string(), "Networking".to_string()],
            &["networking".to_string(), "web".to_string()],
        );
        assert_eq!(count, 1);
    }

    #[test]
    fn policy_denies_when_allow_list_misses() {
        let (result, reasons) =
            evaluate_policy(&["MIT".to_string()], &["Apache-2.0".to_string()], &[]);
        assert!(matches!(result, LicensePolicyResult::Denied));
        assert!(reasons[0].contains("allow_licenses"));
    }
}
