use std::collections::BTreeSet;

use rmcp::Json;
pub use rust_mcp_types::types::krate::{
    CrateAlternativeHit, CrateAlternativesRequest, CrateAlternativesResponse,
};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::mcp::models::{ConfidenceAssessment, ConfidenceLevel, LicensePolicyResult};
use crate::mcp::server::McpServer;
use crate::mcp::utils::{
    CursorToken, alternatives_limit, apply_pagination_limit, build_crate_freshness_sources,
    decode_cursor, normalize_optional, normalize_required, resolve_pagination, sync_page,
};

#[derive(Debug, Serialize, Deserialize)]
struct CrateAlternativesCursorToken {
    v: u8,
    offset: u32,
    limit: u32,
    crate_name: String,
    version: Option<String>,
    allow_licenses: Vec<String>,
    deny_licenses: Vec<String>,
}

impl CursorToken for CrateAlternativesCursorToken {
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

#[derive(Debug, FromRow)]
pub(crate) struct AlternativesCandidateRow {
    pub(crate) crate_name: String,
    pub(crate) description: Option<String>,
    pub(crate) categories: Vec<String>,
    pub(crate) keywords: Vec<String>,
    pub(crate) latest_version: Option<String>,
    pub(crate) total_downloads: i64,
    pub(crate) yanked: bool,
    pub(crate) advisory_count: i64,
    pub(crate) license_expression: Option<String>,
    pub(crate) dependent_count: i64,
    pub(crate) name_similarity: f64,
}

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
    pub(crate) async fn handle_crate_alternatives(
        &self,
        request: CrateAlternativesRequest,
    ) -> Result<Json<CrateAlternativesResponse>, String> {
        let crate_name = normalize_required(request.crate_name, "crate_name")?;
        let requested_version = normalize_optional(request.version);
        let cursor = normalize_optional(request.cursor);
        let page = sync_page(request.page);
        let requested_limit = alternatives_limit(request.limit);
        let allow_licenses = normalize_policy_list(request.allow_licenses);
        let deny_licenses = normalize_policy_list(request.deny_licenses);

        let decoded = cursor
            .as_deref()
            .map(decode_cursor::<CrateAlternativesCursorToken>)
            .transpose()?;
        if let Some(ref token) = decoded
            && (token.crate_name != crate_name
                || token.version != requested_version
                || token.allow_licenses != allow_licenses
                || token.deny_licenses != deny_licenses)
        {
            return Err("cursor does not match current crate.alternatives filters".to_string());
        }
        let pag =
            resolve_pagination(decoded.as_ref(), request.limit.is_some(), requested_limit, page)?;

        let ctx = self
            .fetch_crate_context(&crate_name)
            .await?;
        let resolution = self
            .resolve_version_or_latest(&ctx, requested_version.as_deref())
            .await?;

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
        .bind(ctx.crate_row.id)
        .bind(&ctx.crate_row.name)
        .bind(&ctx.crate_row.categories)
        .bind(&ctx.crate_row.keywords)
        .bind(i64::from(
            pag.offset
                .saturating_add(pag.limit)
                .saturating_add(1)
                .saturating_mul(8),
        ))
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

                let category_overlap =
                    overlap_count(&ctx.crate_row.categories, &candidate.categories);
                let keyword_overlap = overlap_count(&ctx.crate_row.keywords, &candidate.keywords);
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
        let alternatives: Vec<_> = alternatives
            .into_iter()
            .skip(pag.offset as usize)
            .take(pag.limit as usize + 1)
            .collect();
        let crate_name_clone = crate_name.clone();
        let requested_version_clone = requested_version.clone();
        let allow_licenses_clone = allow_licenses.clone();
        let deny_licenses_clone = deny_licenses.clone();
        let paginated =
            apply_pagination_limit(alternatives, pag.limit, pag.offset, |next_offset| {
                CrateAlternativesCursorToken {
                    v: 1,
                    offset: next_offset,
                    limit: pag.limit,
                    crate_name: crate_name_clone,
                    version: requested_version_clone,
                    allow_licenses: allow_licenses_clone,
                    deny_licenses: deny_licenses_clone,
                }
            })?;

        let confidence_assessment = if paginated.items.is_empty() {
            ConfidenceAssessment {
                level: ConfidenceLevel::Low,
                reason: "no alternatives matched similarity, taxonomy, and policy constraints"
                    .to_string(),
            }
        } else if ctx
            .crate_row
            .categories
            .is_empty()
            && ctx
                .crate_row
                .keywords
                .is_empty()
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

        let freshness_check_result = ctx
            .freshness_outcome
            .freshness_check_result
            .clone();

        Ok(Json(CrateAlternativesResponse {
            crate_name: ctx.crate_row.name,
            selected_version: resolution
                .selected_version
                .version,
            latest_version: ctx.latest_version.version,
            cursor,
            next_cursor: paginated.next_cursor,
            page: pag.effective_page,
            limit: pag.limit,
            has_more: paginated.has_more,
            truncated: paginated.has_more,
            count: paginated.items.len(),
            allow_licenses,
            deny_licenses,
            alternatives: paginated.items,
            freshness_check_performed: ctx
                .freshness_outcome
                .freshness_check_performed,
            freshness_check_result: freshness_check_result.clone(),
            refresh_enqueued: resolution.refresh_enqueued,
            refresh_job_id: resolution
                .refresh_job_id
                .clone(),
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
