//! Advisory applicability: whether a stored advisory affects a crate version.
//!
//! `advisory_matches` rows come in two shapes. A row with a `version_id` is an
//! explicit match against an indexed version. A row without one only records
//! that the advisory concerns the crate (none of its listed versions were
//! indexed when it was synced), so applicability must be derived from the
//! stored range text, whose format depends on the source.

use std::collections::BTreeMap;

use rustsec::advisory::Versions;
use semver::{BuildMetadata, Version, VersionReq};

use crate::db::models::CrateAdvisoryRow;

/// Whether an advisory affects a specific crate version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applicability {
    /// The version is affected.
    Affected,
    /// The version is outside every affected range.
    NotAffected,
    /// The stored range or the version could not be evaluated.
    Unknown,
}

impl Applicability {
    /// Lowercase label used in tool responses.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Affected => "affected",
            Self::NotAffected => "not_affected",
            Self::Unknown => "unknown",
        }
    }
}

/// One advisory as it applies to one version, merged across its stored rows.
#[derive(Debug, Clone, Copy)]
pub struct VersionAdvisory<'a> {
    /// Applicability to the version.
    pub applicability: Applicability,
    /// Representative stored row (the explicit match when there is one).
    pub row: &'a CrateAdvisoryRow,
}

/// Advisories of one crate that are affected or of unknown applicability for
/// `version`, one entry per advisory ID, ordered by advisory ID. All sources
/// contribute: affected evidence wins over unknown, then not affected.
/// Withdrawal suppresses all records of that ID; records awaiting re-sync
/// contribute only unknown applicability.
pub fn advisories_for_version<'a>(
    rows: &'a [CrateAdvisoryRow],
    version_id: i64,
    version: &str,
) -> Vec<VersionAdvisory<'a>> {
    let mut by_id = BTreeMap::<&str, Vec<&CrateAdvisoryRow>>::new();
    for row in rows {
        by_id
            .entry(row.advisory_id.as_str())
            .or_default()
            .push(row);
    }

    by_id
        .into_values()
        .filter_map(|mut rows| {
            // Stable representatives, independent of database row order.
            rows.sort_by_key(|row| (&row.source, &row.affected_range, row.version_id));
            // A withdrawal from the authority or its mirror applies to this ID,
            // including explicit matches retained by another source.
            if rows
                .iter()
                .any(|row| !row.requires_resync && is_withdrawn(&row.affected_range))
            {
                return None;
            }
            let mut selected: Option<VersionAdvisory<'a>> = None;
            for row in &rows {
                let applicability = if row.requires_resync {
                    Applicability::Unknown
                } else if row.version_id == Some(version_id) {
                    Applicability::Affected
                } else {
                    range_applicability(&row.source, &row.affected_range, version)
                };
                if applicability == Applicability::NotAffected {
                    continue;
                }
                let priority = |advisory: &VersionAdvisory<'_>| {
                    (
                        advisory.applicability == Applicability::Affected,
                        advisory.row.version_id == Some(version_id),
                    )
                };
                let candidate = VersionAdvisory { applicability, row };
                if selected
                    .as_ref()
                    .is_none_or(|current| priority(&candidate) > priority(current))
                {
                    selected = Some(candidate);
                }
            }
            selected
        })
        .collect()
}

/// Whether stored RustSec metadata records a withdrawal.
fn is_withdrawn(affected_range: &str) -> bool {
    affected_range
        .split("; ")
        .any(|clause| clause.starts_with("withdrawn="))
}

/// Counts of affected and unknown-applicability advisories for a version.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AdvisoryCounts {
    /// Advisories affecting the version.
    pub affected: usize,
    /// Advisories whose applicability could not be determined.
    pub unknown: usize,
}

/// Counts advisories of one crate for `version` (see
/// [`advisories_for_version`]).
pub fn advisory_counts(
    rows: &[CrateAdvisoryRow],
    version_id: i64,
    version: &str,
) -> AdvisoryCounts {
    let mut counts = AdvisoryCounts::default();
    for advisory in advisories_for_version(rows, version_id, version) {
        match advisory.applicability {
            Applicability::Affected => counts.affected += 1,
            Applicability::Unknown => counts.unknown += 1,
            Applicability::NotAffected => {}
        }
    }
    counts
}

/// Evaluates an advisory's stored range text against `version`.
pub fn range_applicability(source: &str, affected_range: &str, version: &str) -> Applicability {
    if is_withdrawn(affected_range) {
        return Applicability::NotAffected;
    }
    let Some(version) = parse_semver_for_matching(version) else {
        return Applicability::Unknown;
    };
    if affected_range.contains("SEMVER(") || affected_range.contains("ECOSYSTEM(") {
        return osv_applicability(affected_range, &version);
    }
    if source == "rustsec_db"
        || ["patched=", "unaffected=", "withdrawn="]
            .iter()
            .any(|key| affected_range.contains(key))
    {
        return rustsec_applicability(affected_range, &version);
    }
    if source == "ghsa" {
        return ghsa_applicability(affected_range, &version);
    }
    Applicability::Unknown
}

/// OSV ranges rendered as `TYPE(introduced=..,fixed=..)` joined by `; `.
fn osv_applicability(affected_range: &str, version: &Version) -> Applicability {
    let mut unknown = false;
    for range in affected_range.split("; ") {
        let events = range
            .strip_prefix("SEMVER(")
            .or_else(|| range.strip_prefix("ECOSYSTEM("))
            .and_then(|rest| rest.strip_suffix(')'))
            .and_then(parse_osv_events);
        match events {
            Some(events) if osv_range_affects(&events, version) => return Applicability::Affected,
            Some(_) => {}
            None => unknown = true,
        }
    }
    if unknown { Applicability::Unknown } else { Applicability::NotAffected }
}

/// A state transition within an OSV range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum OsvEvent {
    /// Inclusive start of an affected interval.
    Introduced,
    /// Exclusive end of an affected interval.
    Fixed,
    /// Inclusive end of an affected interval.
    LastAffected,
}

/// OSV transitions and independent upper limits. `None` is the start sentinel
/// in transitions and positive infinity in limits.
struct OsvRange {
    /// Vulnerability state transitions ordered by version precedence.
    events: Vec<(Option<Version>, OsvEvent)>,
    /// Independent upper bounds; any matching bound permits evaluation.
    limits: Vec<Option<Version>>,
}

/// Parses stored OSV events without treating upper limits as fixed releases.
fn parse_osv_events(text: &str) -> Option<OsvRange> {
    let mut range = OsvRange {
        events: Vec::new(),
        limits: Vec::new(),
    };
    for event in text.split(',') {
        let (kind, value) = event.trim().split_once('=')?;
        if kind == "limit" {
            range
                .limits
                .push(if value.contains('*') {
                    None
                } else {
                    Some(parse_semver_for_matching(value)?)
                });
            continue;
        }
        let kind = match kind {
            "introduced" => OsvEvent::Introduced,
            "fixed" => OsvEvent::Fixed,
            "last_affected" => OsvEvent::LastAffected,
            _ => return None,
        };
        let bound = if kind == OsvEvent::Introduced && value == "0" {
            None
        } else {
            Some(parse_semver_for_matching(value)?)
        };
        range
            .events
            .push((bound, kind));
    }
    let has = |kind| {
        range
            .events
            .iter()
            .any(|(_, event)| *event == kind)
    };
    if !has(OsvEvent::Introduced) || (has(OsvEvent::Fixed) && has(OsvEvent::LastAffected)) {
        return None;
    }
    range.events.sort();
    Some(range)
}

/// Evaluates OSV transitions only when the version is below at least one limit.
fn osv_range_affects(range: &OsvRange, version: &Version) -> bool {
    if !range.limits.is_empty()
        && !range
            .limits
            .iter()
            .any(|limit| {
                limit
                    .as_ref()
                    .is_none_or(|bound| version < bound)
            })
    {
        return false;
    }
    let mut affected = false;
    for (bound, kind) in &range.events {
        let at_or_after = bound
            .as_ref()
            .is_none_or(|bound| version >= bound);
        match kind {
            OsvEvent::Introduced if at_or_after => affected = true,
            OsvEvent::Fixed if at_or_after => affected = false,
            OsvEvent::LastAffected
                if bound
                    .as_ref()
                    .is_some_and(|bound| version > bound) =>
            {
                affected = false
            }
            _ => {}
        }
    }
    affected
}

/// RustSec database ranges rendered as `patched=a | b; unaffected=c;
/// withdrawn=..`.
fn rustsec_applicability(affected_range: &str, version: &Version) -> Applicability {
    if is_withdrawn(affected_range) {
        return Applicability::NotAffected;
    }
    let mut patched = Vec::new();
    let mut unaffected = Vec::new();
    for clause in affected_range
        .split("; ")
        .filter(|clause| !clause.is_empty())
    {
        let Some((key, value)) = clause.split_once('=') else {
            return Applicability::Unknown;
        };
        let target = match key {
            "patched" => &mut patched,
            "unaffected" => &mut unaffected,
            "categories" => continue,
            _ => return Applicability::Unknown,
        };
        for requirement in value.split(" | ") {
            let Ok(requirement) = VersionReq::parse(requirement) else {
                return Applicability::Unknown;
            };
            target.push(requirement);
        }
    }
    match Versions::new(patched, unaffected) {
        Ok(versions) if versions.is_vulnerable(version) => Applicability::Affected,
        Ok(_) => Applicability::NotAffected,
        Err(_) => Applicability::Unknown,
    }
}

/// GitHub's documented VVR comparators use version precedence, including
/// prereleases. Unsupported expressions remain unknown.
fn ghsa_applicability(ranges: &str, version: &Version) -> Applicability {
    let mut unknown = false;
    for range in ranges.split(" || ") {
        let mut matches = true;
        let mut valid = true;
        for comparator in range.split(',') {
            let comparator = comparator.trim();
            let Some((operator, bound)) = [">=", "<=", ">", "<", "="]
                .into_iter()
                .find_map(|op| {
                    comparator
                        .strip_prefix(op)
                        .map(|bound| (op, bound.trim()))
                })
            else {
                valid = false;
                continue;
            };
            let Some(bound) = parse_semver_for_matching(bound) else {
                valid = false;
                continue;
            };
            matches &= match operator {
                ">=" => version >= &bound,
                "<=" => version <= &bound,
                ">" => version > &bound,
                "<" => version < &bound,
                "=" => version == &bound,
                _ => unreachable!(),
            };
        }
        if valid && matches {
            return Applicability::Affected;
        }
        unknown |= !valid;
    }
    if unknown { Applicability::Unknown } else { Applicability::NotAffected }
}

/// Parses a version for precedence matching, ignoring build metadata and
/// tolerating a leading `v`.
pub fn parse_semver_for_matching(version: &str) -> Option<Version> {
    let mut parsed = Version::parse(
        version
            .strip_prefix('v')
            .unwrap_or(version),
    )
    .ok()?;
    // SemVer precedence ignores build metadata, including at range endpoints.
    parsed.build = BuildMetadata::EMPTY;
    Some(parsed)
}

/// GHSA vulnerable version ranges are comma-separated semver constraints.
pub fn ghsa_version_in_range(version: &str, range_str: &str) -> bool {
    range_applicability("ghsa", range_str, version) == Applicability::Affected
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn row(
        advisory_id: &str,
        version_id: Option<i64>,
        source: &str,
        range: &str,
    ) -> CrateAdvisoryRow {
        CrateAdvisoryRow {
            crate_id: 1,
            requires_resync: false,
            version_id,
            advisory_id: advisory_id.to_string(),
            title: format!("{advisory_id} title"),
            severity: Some("high".to_string()),
            url: None,
            affected_range: range.to_string(),
            fixed_versions: json!([]),
            source: source.to_string(),
            details: None,
            affected_functions: Vec::new(),
            cwe_ids: Vec::new(),
        }
    }

    #[test]
    fn prereleases_follow_advisory_precedence() {
        for (source, range, version, expected) in [
            ("ghsa", "< 2.0.0", "1.5.0-beta.1", Applicability::Affected),
            ("ghsa", ">= 1.0.0, < 1.5.0", "0.9.0-rc.1", Applicability::NotAffected),
            ("ghsa", ">= 1.0.0, < 1.5.0", "1.2.0-rc.1", Applicability::Affected),
            ("ghsa", "= 1.0.0", "1.0.0+build", Applicability::Affected),
            ("ghsa", "< 1.0.0 || >= 2.0.0, <= 2.1.0", "2.1.0", Applicability::Affected),
            ("ghsa", "< 1.0.0 || invalid", "2.1.0", Applicability::Unknown),
            ("ghsa", "^1.0", "1.1.0", Applicability::Unknown),
            ("ghsa", "< 1.0.0, invalid", "2.0.0", Applicability::Unknown),
            ("rustsec_db", "patched=>= 1.4.0", "1.5.0-beta.1", Applicability::NotAffected),
            ("rustsec_db", "unaffected=< 0.2.0", "0.1.0-alpha", Applicability::NotAffected),
            ("rustsec_db", "patched=>= 1.4.0", "1.4.0-beta.1", Applicability::Affected),
        ] {
            assert_eq!(
                range_applicability(source, range, version),
                expected,
                "{source}: {range} @ {version}"
            );
        }
    }

    #[test]
    fn withdrawal_suppresses_mirrors_and_legacy_matches_are_unknown() {
        let mut legacy = row("LEGACY", Some(7), "osv", "SEMVER(introduced=0)");
        legacy.requires_resync = true;
        let rows = vec![
            row("RUSTSEC-X", Some(7), "rustsec_osv", "SEMVER(introduced=0)"),
            row("RUSTSEC-X", None, "rustsec_db", "withdrawn=2025-01-01"),
            legacy,
        ];
        assert_eq!(advisory_counts(&rows, 7, "1.0.0"), AdvisoryCounts { affected: 0, unknown: 1 });
    }

    #[test]
    fn osv_ranges_respect_fixed_and_introduced_bounds() {
        let range = "SEMVER(introduced=0,fixed=2.0.0)";
        assert_eq!(range_applicability("osv", range, "1.9.0"), Applicability::Affected);
        assert_eq!(range_applicability("osv", range, "2.0.0"), Applicability::NotAffected);
        assert_eq!(range_applicability("osv", range, "3.7.0"), Applicability::NotAffected);

        let windowed = "SEMVER(introduced=0.34.0,fixed=0.61.1)";
        assert_eq!(range_applicability("osv", windowed, "0.33.9"), Applicability::NotAffected);
        assert_eq!(range_applicability("osv", windowed, "0.40.0"), Applicability::Affected);
    }

    #[test]
    fn osv_supports_multiple_ranges_last_affected_and_open_ranges() {
        let multi = "SEMVER(introduced=0,fixed=1.0.5); SEMVER(introduced=2.0.0,fixed=2.0.3)";
        assert_eq!(range_applicability("osv", multi, "2.0.1"), Applicability::Affected);
        assert_eq!(range_applicability("osv", multi, "1.5.0"), Applicability::NotAffected);

        let last = "SEMVER(introduced=1.0.0,last_affected=1.2.0)";
        assert_eq!(range_applicability("osv", last, "1.2.0"), Applicability::Affected);
        assert_eq!(range_applicability("osv", last, "1.2.1"), Applicability::NotAffected);

        let open = "SEMVER(introduced=1.0.0)";
        assert_eq!(range_applicability("osv", open, "9.0.0"), Applicability::Affected);
    }

    #[test]
    fn osv_limits_sentinels_and_semver_precedence() {
        for (range, version, expected) in [
            ("SEMVER(introduced=0,limit=2.0.0,limit=4.0.0)", "3.0.0", Applicability::Affected),
            ("SEMVER(introduced=0,limit=2.0.0,limit=4.0.0)", "4.0.0", Applicability::NotAffected),
            ("SEMVER(introduced=0,limit=*)", "99.0.0", Applicability::Affected),
            ("SEMVER(introduced=0,limit=2.0.0,limit=*)", "3.0.0", Applicability::Affected),
            ("SEMVER(introduced=0)", "0.0.0-alpha", Applicability::Affected),
            ("SEMVER(introduced=1.0.0+build)", "1.0.0", Applicability::Affected),
            ("SEMVER(introduced=0,fixed=1.0.0+build)", "1.0.0", Applicability::NotAffected),
            ("SEMVER(introduced=0,last_affected=1.0.0)", "1.0.0+build", Applicability::Affected),
            ("SEMVER(introduced=0,limit=1.0.0+build)", "1.0.0", Applicability::NotAffected),
            ("SEMVER(fixed=2.0.0)", "1.0.0", Applicability::Unknown),
            (
                "SEMVER(introduced=0,fixed=2.0.0,last_affected=3.0.0)",
                "1.0.0",
                Applicability::Unknown,
            ),
            ("SEMVER(introduced=0,limit=bad)", "1.0.0", Applicability::Unknown),
        ] {
            assert_eq!(range_applicability("osv", range, version), expected, "{range} @ {version}");
        }
    }

    #[test]
    fn all_sources_contribute_independently_of_row_order() {
        let mut rows = vec![
            row("A", None, "ghsa", "< 2.0.0"),
            row("A", None, "osv", "SEMVER(introduced=0,fixed=4.0.0)"),
            row("B", None, "ghsa", "< 2.0.0"),
            row("B", None, "osv", "osv_affected_unknown"),
            row("C", Some(7), "rustsec_db", "patched=>= 4.0.0; withdrawn=2025-01-01"),
            row("D", None, "rustsec_db", "patched=>= 4.0.0; withdrawn=2025-01-01"),
            row("D", Some(7), "rustsec_db", "patched=>= 4.0.0"),
        ];
        for _ in 0..2 {
            let actual = advisories_for_version(&rows, 7, "3.0.0");
            assert_eq!(
                actual
                    .iter()
                    .map(|a| (a.row.advisory_id.as_str(), a.applicability, a.row.source.as_str()))
                    .collect::<Vec<_>>(),
                vec![("A", Applicability::Affected, "osv"), ("B", Applicability::Unknown, "osv")]
            );
            rows.reverse();
        }
    }

    #[test]
    fn rustsec_empty_restrictions_and_invalid_requirements() {
        assert_eq!(range_applicability("rustsec_db", "", "1.0.0"), Applicability::Affected);
        assert_eq!(
            range_applicability("rustsec_db", "categories=memory-corruption", "1.0.0"),
            Applicability::Affected
        );
        assert_eq!(
            range_applicability("rustsec_db", "patched=invalid", "1.0.0"),
            Applicability::Unknown
        );
        assert_eq!(
            range_applicability("rustsec_db", "unparseable", "1.0.0"),
            Applicability::Unknown
        );
    }

    #[test]
    fn unparseable_ranges_and_versions_are_unknown() {
        assert_eq!(
            range_applicability("osv", "osv_affected_unknown", "1.0.0"),
            Applicability::Unknown
        );
        assert_eq!(
            range_applicability("osv", "GIT(introduced=abc)", "1.0.0"),
            Applicability::Unknown
        );
        assert_eq!(
            range_applicability("osv", "SEMVER(introduced=0,fixed=2.0.0)", "not-a-version"),
            Applicability::Unknown
        );
        assert_eq!(range_applicability("ghsa", "unknown", "1.0.0"), Applicability::Unknown);
    }

    #[test]
    fn rustsec_and_ghsa_formats_are_evaluated() {
        let rustsec = "patched=>= 1.4.0; unaffected=< 0.2.0; categories=memory-corruption";
        assert_eq!(range_applicability("rustsec_db", rustsec, "1.0.0"), Applicability::Affected);
        assert_eq!(range_applicability("rustsec_db", rustsec, "1.4.0"), Applicability::NotAffected);
        assert_eq!(range_applicability("rustsec_db", rustsec, "0.1.0"), Applicability::NotAffected);
        assert_eq!(
            range_applicability("rustsec_db", "patched=>= 1.0.0; withdrawn=2025-01-01", "0.5.0"),
            Applicability::NotAffected
        );

        assert!(ghsa_version_in_range("1.2.0", ">= 1.0.0, < 1.5.0"));
        assert!(!ghsa_version_in_range("1.5.0", ">= 1.0.0, < 1.5.0"));
    }

    #[test]
    fn advisories_for_version_merges_rows_and_prefers_explicit_matches() {
        let rows = vec![
            // Explicit match for version 7, plus a crate-level row for the same advisory.
            row("A", Some(7), "rustsec_osv", "SEMVER(introduced=5.0.0)"),
            row("A", None, "osv", "SEMVER(introduced=5.0.0)"),
            // Crate-level, fixed long ago.
            row("B", None, "osv", "SEMVER(introduced=0,fixed=2.0.0)"),
            // Crate-level, unparseable.
            row("C", None, "osv", "osv_affected_unknown"),
        ];

        let for_v7 = advisories_for_version(&rows, 7, "3.7.0");
        let summary = for_v7
            .iter()
            .map(|a| (a.row.advisory_id.as_str(), a.applicability, a.row.version_id))
            .collect::<Vec<_>>();
        assert_eq!(
            summary,
            vec![("A", Applicability::Affected, Some(7)), ("C", Applicability::Unknown, None)]
        );
        assert_eq!(advisory_counts(&rows, 7, "3.7.0"), AdvisoryCounts { affected: 1, unknown: 1 });
        assert_eq!(advisory_counts(&rows, 9, "1.0.0"), AdvisoryCounts { affected: 1, unknown: 1 });
        assert_eq!(advisory_counts(&rows, 9, "6.0.0"), AdvisoryCounts { affected: 1, unknown: 1 });
    }
}
