use std::collections::BTreeSet;

use rmcp::Json;
pub use rust_mcp_types::types::krate::{
    CrateAlternativeHit, CrateAlternativesRequest, CrateAlternativesResponse,
};
use serde::{Deserialize, Serialize};

use crate::db::models::AlternativesCandidateRow;
use crate::db::tools;
use crate::mcp::models::{ConfidenceAssessment, ConfidenceLevel, LicensePolicyResult};
use crate::mcp::progress::ToolCallContext;
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

fn days_since_published(published_at: Option<&str>) -> Option<f64> {
    use std::time::{SystemTime, UNIX_EPOCH};

    let ts = published_at?;
    // PostgreSQL `timestamptz::TEXT` format: "2024-01-15 14:30:00.123456+00"
    // We only need year-month-day for a rough day count.
    let date_part = ts.get(..10)?;
    let mut parts = date_part.split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: i64 = parts.next()?.parse().ok()?;
    let day: i64 = parts.next()?.parse().ok()?;

    // Approximate days from epoch using the same formula for both published and
    // now.
    fn approx_epoch_days(y: i64, m: i64, d: i64) -> i64 {
        // Rata Die approximation (good enough for day-level delta).
        let m_adj = if m > 2 { m - 3 } else { m + 9 };
        let y_adj = if m > 2 { y } else { y - 1 };
        y_adj * 365 + y_adj / 4 - y_adj / 100 + y_adj / 400 + (m_adj * 306 + 5) / 10 + d - 1
    }

    let published_days = approx_epoch_days(year, month, day);

    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_secs();
    let now_days_from_epoch = (now_secs / 86_400) as i64;
    // Unix epoch is 1970-01-01.
    let epoch_rata_die = approx_epoch_days(1970, 1, 1);
    let now_days = epoch_rata_die + now_days_from_epoch;

    let delta = (now_days - published_days).max(0);
    Some(delta as f64)
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

    let adoption_component = ((candidate.total_downloads as f64) + 1.0).ln() * 0.15;
    score += adoption_component;
    reasons.push(format!("downloads={}", candidate.total_downloads));

    let dependents_component = ((candidate.dependent_count as f64) + 1.0).ln() * 0.20;
    score += dependents_component;
    reasons.push(format!("dependents={}", candidate.dependent_count));

    if let Some(days_old) = days_since_published(
        candidate
            .published_at
            .as_deref(),
    ) {
        // Bonus decays linearly: 2.0 at day 0, 0.0 at 2+ years.
        let recency_component = (2.0 - (days_old / 365.0)).clamp(0.0, 2.0);
        score += recency_component;
        reasons.push(format!("recency_days={days_old:.0}"));
    }

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
    /// Handles the `crate_alternatives` tool call.
    pub async fn handle_crate_alternatives(
        &self,
        request: CrateAlternativesRequest,
        tcx: ToolCallContext,
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
            return Err("cursor does not match current crate_alternatives filters".to_string());
        }
        let pag =
            resolve_pagination(decoded.as_ref(), request.limit.is_some(), requested_limit, page)?;

        let ctx = self
            .fetch_crate_context(&crate_name, &tcx)
            .await?;
        let resolution = self
            .resolve_version_or_latest(&ctx, requested_version.as_deref(), &tcx)
            .await?;

        let candidates = tools::list_alternatives_candidates(
            &self.state.db,
            ctx.crate_row.id,
            &ctx.crate_row.name,
            &ctx.crate_row.categories,
            &ctx.crate_row.keywords,
            i64::from(
                pag.offset
                    .saturating_add(pag.limit)
                    .saturating_add(1)
                    .saturating_mul(8),
            ),
        )
        .await
        .map_err(|e| format!("crate_alternatives candidate query failed: {e}"))?;

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

                // Gate: require at least some relevance signal beyond raw popularity.
                if category_overlap == 0 && keyword_overlap == 0 && candidate.name_similarity < 0.30
                {
                    return None;
                }

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
            suggested_next_tools: vec![
                "crate_intel".to_string(),
                "crate_license_check".to_string(),
                "crate_graph".to_string(),
            ],
            provenance: "local_postgres_index".to_string(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AlternativesCandidateRow, LicensePolicyResult, days_since_published, evaluate_policy,
        extract_license_identifiers, overlap_count, score_candidate,
    };

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

    fn make_candidate(
        name_similarity: f64,
        total_downloads: i64,
        dependent_count: i64,
        published_at: Option<&str>,
    ) -> AlternativesCandidateRow {
        AlternativesCandidateRow {
            crate_name: "test_crate".to_string(),
            description: None,
            categories: vec![],
            keywords: vec![],
            latest_version: Some("1.0.0".to_string()),
            published_at: published_at.map(ToString::to_string),
            total_downloads,
            yanked: false,
            advisory_count: 0,
            license_expression: Some("MIT".to_string()),
            dependent_count,
            name_similarity,
        }
    }

    #[test]
    fn days_since_published_parses_postgres_timestamp() {
        // A well-known date in the past.
        let days = days_since_published(Some("2020-01-01 00:00:00+00"));
        assert!(days.is_some());
        // At minimum 5+ years (1800+ days).
        assert!(days.unwrap() > 1800.0);
    }

    #[test]
    fn days_since_published_returns_none_for_empty_or_bad_input() {
        assert!(days_since_published(None).is_none());
        assert!(days_since_published(Some("")).is_none());
        assert!(days_since_published(Some("not-a-date")).is_none());
        assert!(days_since_published(Some("2024")).is_none());
    }

    #[test]
    fn score_candidate_includes_recency_component_for_recent_crate() {
        // A crate published "today" (use a far-future date to guarantee recency).
        let candidate = make_candidate(0.5, 1000, 10, Some("2026-01-01 00:00:00+00"));
        let (score, reasons) = score_candidate(&candidate, 1, 1, LicensePolicyResult::Allowed);
        assert!(score > 0.0);
        assert!(
            reasons
                .iter()
                .any(|r| r.starts_with("recency_days=")),
            "expected recency_days reason, got: {reasons:?}"
        );
    }

    #[test]
    fn score_candidate_no_recency_for_very_old_crate() {
        // A crate published 10+ years ago: recency component should be 0.
        let candidate = make_candidate(0.5, 1000, 10, Some("2015-01-01 00:00:00+00"));
        let (score_old, _) = score_candidate(&candidate, 1, 1, LicensePolicyResult::Allowed);

        // Compare with a recently published crate (same other signals).
        let recent = make_candidate(0.5, 1000, 10, Some("2026-01-01 00:00:00+00"));
        let (score_recent, _) = score_candidate(&recent, 1, 1, LicensePolicyResult::Allowed);

        assert!(
            score_recent > score_old,
            "recent crate (score={score_recent}) should score higher than old crate \
             (score={score_old})"
        );
    }

    #[test]
    fn score_candidate_adoption_signals_bounded() {
        // Even with massive downloads/dependents, adoption should not dominate.
        let popular = make_candidate(0.0, 100_000_000, 50_000, None);
        let (popular_score, _) = score_candidate(&popular, 0, 0, LicensePolicyResult::Allowed);

        // A crate with moderate popularity but strong taxonomy overlap should win.
        let relevant = make_candidate(0.5, 1000, 10, None);
        let (relevant_score, _) = score_candidate(&relevant, 2, 3, LicensePolicyResult::Allowed);

        assert!(
            relevant_score > popular_score,
            "relevant crate (score={relevant_score}) should outscore unrelated popular crate \
             (score={popular_score})"
        );
    }

    #[test]
    fn score_candidate_yanked_penalty_applied() {
        let mut candidate = make_candidate(0.5, 1000, 10, None);
        let (score_normal, _) = score_candidate(&candidate, 1, 0, LicensePolicyResult::Allowed);
        candidate.yanked = true;
        let (score_yanked, reasons) =
            score_candidate(&candidate, 1, 0, LicensePolicyResult::Allowed);
        assert!(score_yanked < score_normal);
        assert!(
            reasons
                .iter()
                .any(|r| r.contains("yanked_penalty")),
            "expected yanked penalty in reasons"
        );
    }

    #[test]
    fn score_candidate_advisory_penalty_applied() {
        let mut candidate = make_candidate(0.5, 1000, 10, None);
        let (score_clean, _) = score_candidate(&candidate, 1, 0, LicensePolicyResult::Allowed);
        candidate.advisory_count = 3;
        let (score_advisory, reasons) =
            score_candidate(&candidate, 1, 0, LicensePolicyResult::Allowed);
        assert!(score_advisory < score_clean);
        assert!(
            reasons
                .iter()
                .any(|r| r.contains("advisory_penalty")),
            "expected advisory penalty in reasons"
        );
    }

    #[test]
    fn score_candidate_license_denied_penalty() {
        let candidate = make_candidate(0.5, 1000, 10, None);
        let (score_allowed, _) = score_candidate(&candidate, 1, 0, LicensePolicyResult::Allowed);
        let (score_denied, _) = score_candidate(&candidate, 1, 0, LicensePolicyResult::Denied);
        assert!(score_denied < score_allowed - 7.0);
    }
}
