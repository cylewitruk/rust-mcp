use rust_mcp::db::indexing::{
    fetch_github_metadata, fetch_github_releases, update_git_probe_data, upsert_github_metadata,
    upsert_github_releases,
};
use rust_mcp::db::models::GitHubMetadataInsert;
use rust_mcp_testing::fixtures::{seed_crate_version, seed_source_file, seed_symbol};
use serde_json::Value;

use super::common;

/// Verifies that upsert_github_metadata writes a row and fetch_github_metadata
/// reads it back correctly (including TIMESTAMPTZ round-trip as text).
#[tokio::test]
async fn github_metadata_upsert_and_fetch_round_trip() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded context");

    let crate_id = context
        .fixture
        .dependent
        .crate_id;

    // No metadata initially.
    let before = fetch_github_metadata(&context.state.db, crate_id)
        .await
        .expect("fetch should not error");
    assert!(before.is_none(), "no GitHub metadata should exist yet");

    // Upsert metadata.
    let insert = GitHubMetadataInsert {
        owner: "serde-rs",
        repo: "json",
        stargazers_count: 4200,
        forks_count: 800,
        open_issues_count: 42,
        archived: false,
        pushed_at: Some("2026-03-01T12:00:00Z"),
        license_spdx: Some("MIT"),
        contributor_count: Some(150),
    };
    upsert_github_metadata(&context.state.db, crate_id, &insert)
        .await
        .expect("upsert should succeed");

    // Fetch it back.
    let row = fetch_github_metadata(&context.state.db, crate_id)
        .await
        .expect("fetch should not error")
        .expect("metadata should exist after upsert");

    assert_eq!(row.owner, "serde-rs");
    assert_eq!(row.repo, "json");
    assert_eq!(row.stargazers_count, 4200);
    assert_eq!(row.forks_count, 800);
    assert_eq!(row.open_issues_count, 42);
    assert!(!row.archived);
    assert!(row.pushed_at.is_some(), "pushed_at should be set");
    assert_eq!(row.license_spdx.as_deref(), Some("MIT"));
    assert_eq!(row.contributor_count, Some(150));
    assert!(!row.fetched_at.is_empty(), "fetched_at should be set");

    // Upsert again with updated values to verify ON CONFLICT.
    let update = GitHubMetadataInsert {
        owner: "serde-rs",
        repo: "json",
        stargazers_count: 5000,
        forks_count: 900,
        open_issues_count: 30,
        archived: true,
        pushed_at: Some("2026-03-05T08:00:00Z"),
        license_spdx: Some("Apache-2.0"),
        contributor_count: Some(200),
    };
    upsert_github_metadata(&context.state.db, crate_id, &update)
        .await
        .expect("second upsert should succeed");

    let updated = fetch_github_metadata(&context.state.db, crate_id)
        .await
        .expect("fetch should not error")
        .expect("metadata should exist after second upsert");

    assert_eq!(updated.stargazers_count, 5000);
    assert!(updated.archived);
    assert_eq!(
        updated
            .license_spdx
            .as_deref(),
        Some("Apache-2.0")
    );
    assert_eq!(updated.contributor_count, Some(200));
}

/// Verifies that crate_intel surfaces a `github` section when metadata is
/// present in the database.
#[tokio::test]
async fn crate_intel_surfaces_github_metadata() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded context");

    let crate_id = context
        .fixture
        .dependent
        .crate_id;

    // Seed GitHub metadata directly.
    let insert = GitHubMetadataInsert {
        owner: "serde-rs",
        repo: "json",
        stargazers_count: 4200,
        forks_count: 800,
        open_issues_count: 42,
        archived: false,
        pushed_at: Some("2026-03-01T12:00:00Z"),
        license_spdx: Some("MIT"),
        contributor_count: Some(150),
    };
    upsert_github_metadata(&context.state.db, crate_id, &insert)
        .await
        .expect("upsert should succeed");

    // Call crate_intel via MCP and check for the github section.
    let response = context
        .mcp
        .call_tool("crate_intel", serde_json::json!({"crate_name": "serde_json"}))
        .await
        .expect("crate_intel call failed");
    let payload = common::structured_content(&response);

    let github = payload
        .get("github")
        .expect("github section should be present in crate_intel response");

    assert_eq!(
        github
            .get("owner")
            .and_then(Value::as_str),
        Some("serde-rs")
    );
    assert_eq!(
        github
            .get("repo")
            .and_then(Value::as_str),
        Some("json")
    );
    assert_eq!(
        github
            .get("stars")
            .and_then(Value::as_u64),
        Some(4200)
    );
    assert_eq!(
        github
            .get("forks")
            .and_then(Value::as_u64),
        Some(800)
    );
    assert_eq!(
        github
            .get("archived")
            .and_then(Value::as_bool),
        Some(false)
    );
    assert_eq!(
        github
            .get("license")
            .and_then(Value::as_str),
        Some("MIT")
    );
    assert_eq!(
        github
            .get("contributors")
            .and_then(Value::as_u64),
        Some(150)
    );
}

/// Verifies that upsert_github_releases writes rows and fetch_github_releases
/// reads them back in descending published_at order.
#[tokio::test]
async fn github_releases_upsert_and_fetch_round_trip() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded context");

    let crate_id = context
        .fixture
        .dependent
        .crate_id;

    // No releases initially.
    let before = fetch_github_releases(&context.state.db, crate_id, 10)
        .await
        .expect("fetch should not error");
    assert!(before.is_empty(), "no releases should exist yet");

    // Upsert releases.
    let releases = vec![
        (
            "v1.0.0".to_string(),
            Some("1.0.0".to_string()),
            Some("Initial release".to_string()),
            Some("2026-01-15T10:00:00Z".to_string()),
            false,
        ),
        (
            "v1.1.0".to_string(),
            Some("1.1.0".to_string()),
            Some("Bug fixes and improvements".to_string()),
            Some("2026-02-20T14:00:00Z".to_string()),
            false,
        ),
        (
            "v2.0.0-rc1".to_string(),
            Some("2.0.0 Release Candidate 1".to_string()),
            Some("Major version pre-release".to_string()),
            Some("2026-03-01T08:00:00Z".to_string()),
            true,
        ),
    ];
    upsert_github_releases(&context.state.db, crate_id, &releases)
        .await
        .expect("upsert should succeed");

    // Fetch them back — should be ordered by published_at DESC.
    let rows = fetch_github_releases(&context.state.db, crate_id, 10)
        .await
        .expect("fetch should not error");

    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].tag_name, "v2.0.0-rc1");
    assert!(rows[0].prerelease);
    assert_eq!(rows[1].tag_name, "v1.1.0");
    assert!(!rows[1].prerelease);
    assert_eq!(rows[2].tag_name, "v1.0.0");
    assert_eq!(rows[2].body.as_deref(), Some("Initial release"));

    // Upsert again with different set — should replace all.
    let updated_releases = vec![(
        "v2.0.0".to_string(),
        Some("2.0.0".to_string()),
        Some("Major version release".to_string()),
        Some("2026-03-05T12:00:00Z".to_string()),
        false,
    )];
    upsert_github_releases(&context.state.db, crate_id, &updated_releases)
        .await
        .expect("second upsert should succeed");

    let updated_rows = fetch_github_releases(&context.state.db, crate_id, 10)
        .await
        .expect("fetch should not error");

    assert_eq!(updated_rows.len(), 1);
    assert_eq!(updated_rows[0].tag_name, "v2.0.0");
    assert_eq!(
        updated_rows[0]
            .body
            .as_deref(),
        Some("Major version release")
    );
}

/// Verifies that `crate_api_diff` includes GitHub release notes from the DB
/// in its response.
#[tokio::test]
async fn crate_api_diff_surfaces_github_release_notes() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded context");

    let crate_id = context
        .fixture
        .dependent
        .crate_id;

    // Seed a second version so api_diff has two versions to compare.
    let previous_version_id = seed_crate_version(
        &context.state.db,
        crate_id,
        "1.0.144",
        100_000,
        Some("2025-12-31T00:00:00Z"),
    )
    .await
    .expect("failed to seed previous crate version");

    let previous_source_id = seed_source_file(
        &context.state.db,
        previous_version_id,
        "src/lib.rs",
        Some("rust"),
        "pub fn old_fn() {}",
    )
    .await
    .expect("failed to seed source file");

    seed_symbol(
        &context.state.db,
        previous_version_id,
        previous_source_id,
        "old_fn",
        "function",
        1,
        1,
    )
    .await
    .expect("failed to seed symbol");

    // Seed release notes in the DB.
    let releases = vec![(
        "v1.0.145".to_string(),
        Some("1.0.145".to_string()),
        Some("Fixed a critical parsing bug".to_string()),
        Some("2026-01-15T10:00:00Z".to_string()),
        false,
    )];
    upsert_github_releases(&context.state.db, crate_id, &releases)
        .await
        .expect("upsert releases should succeed");

    // Call crate_api_diff.
    let response = context
        .mcp
        .call_tool(
            "crate_api_diff",
            serde_json::json!({
                "crate_name": "serde_json",
                "from_version": "1.0.144",
                "to_version": "1.0.145"
            }),
        )
        .await
        .expect("crate_api_diff call failed");
    let payload = common::structured_content(&response);

    // Verify release_notes appears in the response.
    let release_notes = payload
        .get("release_notes")
        .expect("release_notes should be present in crate_api_diff response");

    let notes_array = release_notes
        .as_array()
        .expect("release_notes should be an array");
    assert!(!notes_array.is_empty(), "release_notes should not be empty");

    let first = &notes_array[0];
    assert_eq!(
        first
            .get("tag")
            .and_then(Value::as_str),
        Some("v1.0.145")
    );
    assert_eq!(
        first
            .get("body")
            .and_then(Value::as_str),
        Some("Fixed a critical parsing bug")
    );
}

/// Verifies that update_git_probe_data persists commit liveness columns
/// and fetch_github_metadata reads them back.
#[tokio::test]
async fn git_probe_data_round_trip() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded context");

    let crate_id = context
        .fixture
        .dependent
        .crate_id;

    // First, upsert base metadata (the row must exist before updating probe data).
    let insert = GitHubMetadataInsert {
        owner: "serde-rs",
        repo: "json",
        stargazers_count: 4200,
        forks_count: 800,
        open_issues_count: 42,
        archived: false,
        pushed_at: Some("2026-03-01T12:00:00Z"),
        license_spdx: Some("MIT"),
        contributor_count: Some(150),
    };
    upsert_github_metadata(&context.state.db, crate_id, &insert)
        .await
        .expect("upsert should succeed");

    // Verify commit liveness columns are initially NULL.
    let before = fetch_github_metadata(&context.state.db, crate_id)
        .await
        .expect("fetch should not error")
        .expect("metadata should exist");
    assert!(
        before
            .last_commit_at
            .is_none()
    );
    assert!(
        before
            .last_commit_message
            .is_none()
    );
    assert!(
        before
            .recent_commit_count
            .is_none()
    );

    // Update git probe data.
    update_git_probe_data(
        &context.state.db,
        crate_id,
        Some("2026-03-05T14:30:00+00:00"),
        Some("fix: handle edge case in parser"),
        42,
    )
    .await
    .expect("update_git_probe_data should succeed");

    // Fetch and verify.
    let after = fetch_github_metadata(&context.state.db, crate_id)
        .await
        .expect("fetch should not error")
        .expect("metadata should exist");
    assert!(after.last_commit_at.is_some(), "last_commit_at should be set");
    assert_eq!(
        after
            .last_commit_message
            .as_deref(),
        Some("fix: handle edge case in parser")
    );
    assert_eq!(after.recent_commit_count, Some(42));
}

/// Verifies that crate_intel surfaces git probe liveness data.
#[tokio::test]
async fn crate_intel_surfaces_git_probe_liveness() {
    let context = common::seeded_mcp_context()
        .await
        .expect("failed to build seeded context");

    let crate_id = context
        .fixture
        .dependent
        .crate_id;

    // Seed GitHub metadata + git probe data.
    let insert = GitHubMetadataInsert {
        owner: "serde-rs",
        repo: "json",
        stargazers_count: 4200,
        forks_count: 800,
        open_issues_count: 42,
        archived: false,
        pushed_at: Some("2026-03-01T12:00:00Z"),
        license_spdx: Some("MIT"),
        contributor_count: Some(150),
    };
    upsert_github_metadata(&context.state.db, crate_id, &insert)
        .await
        .expect("upsert should succeed");
    update_git_probe_data(
        &context.state.db,
        crate_id,
        Some("2026-03-05T14:30:00+00:00"),
        Some("chore: bump deps"),
        25,
    )
    .await
    .expect("update_git_probe_data should succeed");

    // Call crate_intel via MCP and check for liveness fields.
    let response = context
        .mcp
        .call_tool("crate_intel", serde_json::json!({"crate_name": "serde_json"}))
        .await
        .expect("crate_intel call failed");
    let payload = common::structured_content(&response);

    let github = payload
        .get("github")
        .expect("github section should be present");

    assert!(
        github
            .get("last_commit_at")
            .and_then(Value::as_str)
            .is_some(),
        "last_commit_at should be present"
    );
    assert_eq!(
        github
            .get("last_commit_message")
            .and_then(Value::as_str),
        Some("chore: bump deps")
    );
    assert_eq!(
        github
            .get("recent_commit_count")
            .and_then(Value::as_u64),
        Some(25)
    );
}
