//! Advisory applicability and dependency coverage across crate tools.

use std::collections::HashMap;
use std::fs;

use rust_mcp::db::indexing::fetch_security_crates_page;
use rust_mcp::mcp::server::McpServer;
use rust_mcp_testing::fixtures::{seed_crate, seed_crate_version};
use serde_json::{Value, json};
use sqlx::PgPool;

use super::common;

const ADVISORY_CRATE: &str = "advisory_demo";

/// Seeds four releases with fixed, open, explicit, unknown and withdrawn
/// advisories, plus conflicting sources. Returns version string → version ID.
async fn seed_advisory_crate(db: &PgPool) -> HashMap<&'static str, i64> {
    let crate_id = seed_crate(db, ADVISORY_CRATE)
        .await
        .expect("seed crate");
    let mut versions = HashMap::new();
    for (version, published_at) in [
        ("1.0.0", "2025-01-01T00:00:00Z"),
        ("2.0.0", "2025-02-01T00:00:00Z"),
        ("2.2.0", "2025-03-01T00:00:00Z"),
        ("3.0.0", "2025-04-01T00:00:00Z"),
    ] {
        let id = seed_crate_version(db, crate_id, version, 10, Some(published_at))
            .await
            .expect("seed version");
        versions.insert(version, id);
    }

    for (advisory_id, version_id, range) in [
        ("ADV-FIXED", None, "SEMVER(introduced=0,fixed=2.0.0)"),
        ("ADV-OPEN", None, "SEMVER(introduced=2.5.0)"),
        ("ADV-EXPLICIT", Some(versions["2.0.0"]), "SEMVER(introduced=2.0.0,fixed=2.0.1)"),
        ("ADV-UNKNOWN", None, "osv_affected_unknown"),
    ] {
        sqlx::query(
            "INSERT INTO advisory_matches (
                crate_id, version_id, advisory_id, severity, title, url, affected_range,
                fixed_versions, source, details
             ) VALUES ($1, $2, $3, 'high', $3, NULL, $4, '[]'::jsonb, 'osv', $5)",
        )
        .bind(crate_id)
        .bind(version_id)
        .bind(advisory_id)
        .bind(range)
        .bind(format!("{advisory_id} full write-up"))
        .execute(db)
        .await
        .expect("seed advisory");
    }

    // The earlier-sorting source must not hide an affected OSV range.
    sqlx::query(
        "INSERT INTO advisory_matches (crate_id, advisory_id, title, affected_range, \
         fixed_versions, source)
        VALUES ($1, 'ADV-FIXED', 'Conflicting source', '>= 99.0.0', '[]'::jsonb, 'ghsa')",
    )
    .bind(crate_id)
    .execute(db)
    .await
    .expect("seed conflicting source");
    // Existing databases can retain explicit matches for withdrawn advisories.
    sqlx::query(
        "INSERT INTO advisory_matches (crate_id, version_id, advisory_id, title, affected_range, \
         fixed_versions, source)
        VALUES ($1, $2, 'ADV-WITHDRAWN', 'Withdrawn', 'patched=>= 4.0.0; withdrawn=2025-01-01', \
         '[]'::jsonb, 'rustsec_db')",
    )
    .bind(crate_id)
    .bind(versions["3.0.0"])
    .execute(db)
    .await
    .expect("seed withdrawn match");

    skip_freshness_probes(db).await;
    versions
}

/// Keeps tool calls offline, as `seeded_mcp_context` does for its own crates.
async fn skip_freshness_probes(db: &PgPool) {
    sqlx::query(
        "UPDATE crates SET last_checked_at = NOW(), next_check_at = NOW() + INTERVAL '30 days'",
    )
    .execute(db)
    .await
    .expect("disable freshness probes");
}

fn advisory_summary(intel: &Value) -> Vec<(String, String)> {
    intel["advisories"]
        .as_array()
        .expect("advisories array")
        .iter()
        .map(|advisory| {
            (
                advisory["advisory_id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                advisory["applicability"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

#[tokio::test]
async fn crate_intel_lists_only_applicable_advisories_with_compact_output() {
    let context = common::seeded_mcp_context()
        .await
        .expect("seeded context");
    seed_advisory_crate(&context.state.db).await;

    let response = context
        .mcp
        .call_tool("crate_intel", json!({"crate_name": ADVISORY_CRATE, "version": "3.0.0"}))
        .await
        .expect("crate_intel");
    let intel = common::structured_content(&response);

    assert_eq!(
        advisory_summary(intel),
        vec![
            ("ADV-OPEN".to_string(), "affected".to_string()),
            ("ADV-UNKNOWN".to_string(), "unknown".to_string()),
        ]
    );
    assert_eq!(intel["advisories_not_affecting"], json!(3));
    assert!(
        intel["advisories"]
            .as_array()
            .unwrap()
            .iter()
            .all(|advisory| advisory
                .get("details")
                .is_none()),
        "details must be opt-in"
    );

    let detailed = context
        .mcp
        .call_tool(
            "crate_intel",
            json!({"crate_name": ADVISORY_CRATE, "version": "3.0.0", "include_advisory_details": true}),
        )
        .await
        .expect("crate_intel with details");
    assert_eq!(
        common::structured_content(&detailed)["advisories"][0]["details"],
        json!("ADV-OPEN full write-up")
    );
}

#[tokio::test]
async fn advisory_signals_agree_across_tools() {
    let context = common::seeded_mcp_context()
        .await
        .expect("seeded context");
    seed_advisory_crate(&context.state.db).await;
    let affected = |version: &str| version != "2.2.0";

    // crate_intel version history.
    let response = context
        .mcp
        .call_tool("crate_intel", json!({"crate_name": ADVISORY_CRATE}))
        .await
        .expect("crate_intel");
    for entry in common::structured_content(&response)["version_history"]
        .as_array()
        .unwrap()
    {
        let version = entry["version"]
            .as_str()
            .unwrap();
        assert_eq!(entry["has_advisory"], json!(affected(version)), "history for {version}");
        assert_eq!(entry["advisory_unknown_count"], 1);
    }

    // crate_versions markers and counts.
    let response = context
        .mcp
        .call_tool("crate_versions", json!({"crate_name": ADVISORY_CRATE}))
        .await
        .expect("crate_versions");
    assert_eq!(common::structured_content(&response)["confidence"], "medium");
    for entry in common::structured_content(&response)["versions"]
        .as_array()
        .unwrap()
    {
        let version = entry["version"]
            .as_str()
            .unwrap();
        let markers = entry["markers"]
            .as_array()
            .unwrap();
        assert_eq!(
            markers.contains(&json!("security_advisory")),
            affected(version),
            "markers for {version}: {markers:?}"
        );
        assert!(markers.contains(&json!("security_advisory_unknown")), "{version}");
        assert_eq!(entry["advisory_count"], json!(i64::from(affected(version))), "{version}");
    }

    // dependency_audit on a pinned requirement.
    for (requirement, expected_affected) in [("=2.2.0", false), ("=3.0.0", true)] {
        let cargo_toml = format!(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\n{ADVISORY_CRATE} = \
             \"{requirement}\"\n"
        );
        let response = context
            .mcp
            .call_tool("dependency_audit", json!({"cargo_toml": cargo_toml}))
            .await
            .expect("dependency_audit");
        let audit = common::structured_content(&response);
        assert_eq!(audit["confidence"], "medium");
        let dependency = audit["dependencies"]
            .as_array()
            .unwrap()
            .iter()
            .find(|dep| dep["dependency_name"] == json!(ADVISORY_CRATE))
            .unwrap_or_else(|| panic!("audited dependency missing: {audit}"));
        let markers = dependency["status_markers"]
            .as_array()
            .unwrap();
        assert_eq!(markers.contains(&json!("has_advisory")), expected_affected, "{requirement}");
        assert!(markers.contains(&json!("advisory_applicability_unknown")), "{requirement}");
        assert_eq!(dependency["advisory_count"], json!(i64::from(expected_affected)));
    }
}

#[tokio::test]
async fn crate_intel_distinguishes_missing_from_empty_dependencies() {
    let context = common::seeded_mcp_context()
        .await
        .expect("seeded context");
    let db = &context.state.db;
    let crate_id = seed_crate(db, "coverage_demo")
        .await
        .expect("seed crate");
    let version_id = seed_crate_version(db, crate_id, "1.0.0", 10, Some("2025-01-01T00:00:00Z"))
        .await
        .expect("seed version");
    skip_freshness_probes(db).await;

    let intel = |label: &'static str| {
        let mcp = &context.mcp;
        async move {
            let response = mcp
                .call_tool("crate_intel", json!({"crate_name": "coverage_demo"}))
                .await
                .unwrap_or_else(|e| panic!("crate_intel ({label}): {e}"));
            common::structured_content(&response).clone()
        }
    };

    let not_synced = intel("not synced").await;
    assert_eq!(not_synced["dependency_coverage"], json!("not_synced"));
    assert_eq!(not_synced["confidence"], json!("medium"));
    assert_eq!(not_synced["suggested_next_tools"][0], json!("index_crates"));

    sqlx::query("UPDATE crate_versions SET dependencies_synced_at = NOW() WHERE id = $1")
        .bind(version_id)
        .execute(db)
        .await
        .expect("mark dependencies synced");

    let synced = intel("synced").await;
    assert_eq!(synced["dependency_coverage"], json!("synced"));
    assert_eq!(synced["dependencies"], json!([]));
    assert_eq!(synced["confidence"], json!("high"));
    assert!(
        !synced["suggested_next_tools"]
            .as_array()
            .unwrap()
            .contains(&json!("index_crates")),
        "no indexing suggestion once coverage is complete: {synced}"
    );
}

#[tokio::test]
async fn rankings_report_unknown_advisory_applicability() {
    let context = common::seeded_mcp_context()
        .await
        .expect("seeded context");
    seed_advisory_crate(&context.state.db).await;
    sqlx::query(
        "UPDATE crates SET categories = ARRAY['serialization'] WHERE name IN ($1, 'serde_json')",
    )
    .bind(ADVISORY_CRATE)
    .execute(&context.state.db)
    .await
    .expect("shared category");
    let response = context
        .mcp
        .call_tool(
            "crate_compare",
            json!({
                "left_crate": ADVISORY_CRATE, "right_crate": "serde_json"
            }),
        )
        .await
        .expect("comparison");
    let compare = common::structured_content(&response);
    assert_eq!(compare["left"]["advisory_count"], 1);
    assert_eq!(compare["left"]["advisory_unknown_count"], 1);
    assert_eq!(compare["confidence"], "medium");
    let reasons = compare["recommendation_reasons"].to_string();
    assert!(reasons.contains("unknown advisories"), "{compare}");
    assert!(!reasons.contains("fewer"), "{compare}");

    let response = context
        .mcp
        .call_tool(
            "crate_alternatives",
            json!({
                "crate_name": "serde_json", "limit": 50
            }),
        )
        .await
        .expect("alternatives");
    let alternatives = common::structured_content(&response);
    let candidate = alternatives["alternatives"]
        .as_array()
        .unwrap()
        .iter()
        .find(|candidate| candidate["crate_name"] == ADVISORY_CRATE)
        .expect("advisory candidate");
    assert_eq!(candidate["advisory_count"], 1);
    assert_eq!(candidate["advisory_unknown_count"], 1);
    assert!(
        candidate["rank_reasons"]
            .to_string()
            .contains("advisory_applicability_unknown")
    );
    assert_eq!(alternatives["confidence"], "medium");
}

#[tokio::test]
async fn rustsec_sync_preserves_withdrawal_and_unknown_ranges() {
    let context = common::seeded_mcp_context()
        .await
        .expect("seeded context");
    seed_advisory_crate(&context.state.db).await;
    let root = context
        .registry_dir
        .join("rustsec");
    let crate_dir = root
        .join("crates")
        .join(ADVISORY_CRATE);
    fs::create_dir_all(&crate_dir).expect("advisory directory");
    for (id, extra, patched) in [
        ("RUSTSEC-2025-0001", "withdrawn = \"2025-01-01\"", ">= 4.0.0"),
        ("RUSTSEC-2025-0002", "", "invalid"),
        ("RUSTSEC-2025-0003", "", ">= 4.0.0"),
    ] {
        let text = format!(
            "+++\n[advisory]\nid = \"{id}\"\npackage = \
             \"{ADVISORY_CRATE}\"\n{extra}\n[versions]\npatched = [\"{patched}\"]\n+++\n# Test \
             advisory\n"
        );
        fs::write(crate_dir.join(format!("{id}.md")), text).expect("write advisory");
    }
    let mut state = context.state.clone();
    state.config.rustsec_db_dir = Some(root);
    let crates = fetch_security_crates_page(&state.db, 100, 0, None)
        .await
        .expect("crates");
    let outcome = McpServer::new(state)
        .sync_rustsec_db_security(&crates)
        .await
        .expect("RustSec sync");
    assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
    let rows: Vec<(String, Option<i64>)> = sqlx::query_as(
        "SELECT advisory_id, version_id FROM advisory_matches WHERE source = 'rustsec_db' ORDER \
         BY advisory_id",
    )
    .fetch_all(&context.state.db)
    .await
    .expect("stored advisories");
    assert_eq!(rows.len(), 6, "{rows:?}");
    assert_eq!(rows[0], ("RUSTSEC-2025-0001".to_string(), None));
    assert_eq!(rows[1], ("RUSTSEC-2025-0002".to_string(), None));
    assert!(
        rows[2..]
            .iter()
            .all(|(_, version)| version.is_some())
    );
    let response = context
        .mcp
        .call_tool("crate_intel", json!({"crate_name": ADVISORY_CRATE}))
        .await
        .expect("intel");
    let summary = advisory_summary(common::structured_content(&response));
    assert!(
        !summary
            .iter()
            .any(|(id, _)| id == "RUSTSEC-2025-0001")
    );
    assert!(summary.contains(&("RUSTSEC-2025-0002".to_string(), "unknown".to_string())));
    assert!(summary.contains(&("RUSTSEC-2025-0003".to_string(), "affected".to_string())));
}
