//! Advisory ingestion, source identity, withdrawal and upgrade regressions.

use std::sync::Arc;
use std::time::Duration;
use std::{fs, slice};

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use rust_mcp::db::indexing::fetch_security_crates_page;
use rust_mcp::db::models::SecurityCrateRow;
use rust_mcp::db::tools;
use rust_mcp::mcp::advisories::{self, AdvisoryCounts};
use rust_mcp::mcp::server::McpServer;
use rust_mcp::mcp::{run_refresh_worker_for_tests, run_security_sync_scan_for_tests};
use rust_mcp_testing::fixtures::{seed_crate, seed_crate_version};
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::net::TcpListener;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tokio::time::{sleep, timeout};

use super::common;

const TARGET: &str = "security_target";

/// Mutable upstream responses for deterministic refresh and failure tests.
struct Responses {
    osv: Value,
    osv_status: StatusCode,
    ghsa: Value,
    ghsa_status: StatusCode,
}

/// Local OSV/GHSA fixture server, stopped when dropped.
struct SecurityServer {
    url: String,
    responses: Arc<RwLock<Responses>>,
    task: JoinHandle<()>,
}

impl Drop for SecurityServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl SecurityServer {
    /// Starts a local server with successful empty source responses.
    async fn start() -> Self {
        let responses = Arc::new(RwLock::new(Responses {
            osv: json!({"vulns": []}),
            osv_status: StatusCode::OK,
            ghsa: json!([]),
            ghsa_status: StatusCode::OK,
        }));
        let router = Router::new()
            .route(
                "/v1/query",
                post(|State(state): State<Arc<RwLock<Responses>>>| async move {
                    let state = state.read().await;
                    (state.osv_status, Json(state.osv.clone()))
                }),
            )
            .route(
                "/advisories",
                get(|State(state): State<Arc<RwLock<Responses>>>| async move {
                    let state = state.read().await;
                    (state.ghsa_status, Json(state.ghsa.clone()))
                }),
            )
            .with_state(responses.clone());
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("fixture listener");
        let url = format!(
            "http://{}",
            listener
                .local_addr()
                .expect("fixture address")
        );
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .await
                .expect("fixture server");
        });
        Self { url, responses, task }
    }
}

/// Seeds two stable releases and a prerelease, returning the crate and version
/// IDs.
async fn seed_target(db: &PgPool) -> (SecurityCrateRow, i64, i64) {
    let id = seed_crate(db, TARGET)
        .await
        .expect("target crate");
    seed_crate_version(db, id, "0.5.0", 10, Some("2024-01-01T00:00:00Z"))
        .await
        .expect("old release");
    let pre = seed_crate_version(db, id, "1.5.0-beta.1", 10, Some("2025-01-01T00:00:00Z"))
        .await
        .expect("prerelease");
    let stable = seed_crate_version(db, id, "2.0.0", 10, Some("2026-01-01T00:00:00Z"))
        .await
        .expect("release");
    sqlx::query("UPDATE crates SET next_check_at = NOW() + INTERVAL '30 days'")
        .execute(db)
        .await
        .expect("offline freshness");
    (SecurityCrateRow { id, name: TARGET.to_string() }, stable, pre)
}

/// Returns applicability counts for one selected version.
async fn counts(db: &PgPool, crate_id: i64, version_id: i64, version: &str) -> AdvisoryCounts {
    let rows = tools::fetch_advisory_rows(db, &[crate_id])
        .await
        .expect("advisories");
    advisories::advisory_counts(&rows, version_id, version)
}

#[tokio::test]
async fn osv_scopes_ranges_versions_and_functions_to_the_queried_package() {
    let context = common::seeded_mcp_context()
        .await
        .expect("context");
    let (krate, stable, _) = seed_target(&context.state.db).await;
    let mock = SecurityServer::start().await;
    mock.responses
        .write()
        .await
        .osv = json!({"vulns": [{
        "id": "OSV-MULTI", "summary": "Two packages", "affected": [
            {"package": {"ecosystem": "crates.io", "name": TARGET},
             "ranges": [{"type": "SEMVER", "events": [{"introduced": "0"}, {"fixed": "1.0.0"}]}],
             "ecosystem_specific": {"functions": ["security_target::affected"]}},
            {"package": {"ecosystem": "crates.io", "name": "security_target-derive"},
             "versions": ["2.0.0"], "ranges": [{"type": "SEMVER", "events": [{"introduced": "0"}, {"fixed": "5.0.0"}]}],
             "ecosystem_specific": {"functions": ["other::affected"]}},
            {"package": {"ecosystem": "npm", "name": TARGET}, "ranges": [{"type": "GIT", "events": [{"introduced": "abc"}]}]}
        ]
    }]});
    let mut state = context.state.clone();
    state.config.osv_base_url = mock.url.clone();
    let server = McpServer::new(state);
    let outcome = server
        .sync_osv_security(slice::from_ref(&krate))
        .await
        .expect("OSV sync");
    assert!(outcome.errors.is_empty());
    let rows = tools::fetch_advisory_rows(&context.state.db, &[krate.id])
        .await
        .expect("rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].version_id, None, "the other package's explicit version must not match");
    assert_eq!(rows[0].fixed_versions, json!(["1.0.0"]));
    assert_eq!(rows[0].affected_functions, vec!["security_target::affected"]);
    assert_eq!(
        counts(&context.state.db, krate.id, stable, "2.0.0").await,
        AdvisoryCounts::default()
    );
    let response = context
        .mcp
        .call_tool("crate_intel", json!({"crate_name": TARGET, "version": "2.0.0"}))
        .await
        .expect("intel");
    let intel = common::structured_content(&response);
    assert_eq!(intel["advisories"], json!([]));
    assert_eq!(intel["advisories_not_affecting"], 1);
}

#[tokio::test]
async fn source_order_preserves_evidence_and_withdrawals_suppress_mirrors() {
    let context = common::seeded_mcp_context()
        .await
        .expect("context");
    let (krate, stable, _) = seed_target(&context.state.db).await;
    let mock = SecurityServer::start().await;
    mock.responses
        .write()
        .await
        .osv = json!({"vulns": [{
        "id": "RUSTSEC-2025-9999", "affected": [{"package": {"ecosystem": "crates.io", "name": TARGET},
        "versions": ["2.0.0"], "ranges": [{"type": "SEMVER", "events": [{"introduced": "0"}]}]}]
    }]});
    let root = context
        .registry_dir
        .join("advisory-db");
    let dir = root
        .join("crates")
        .join(TARGET);
    fs::create_dir_all(&dir).expect("advisory directory");
    let path = dir.join("RUSTSEC-2025-9999.md");
    let write_advisory = |withdrawn: &str| {
        fs::write(
            &path,
            format!(
                "+++\n[advisory]\nid = \"RUSTSEC-2025-9999\"\npackage = \
                 \"{TARGET}\"\n{withdrawn}\n[versions]\npatched = [\">= 3.0.0\"]\n+++\n# Fixture\n"
            ),
        )
        .expect("write advisory")
    };
    write_advisory("");
    let mut state = context.state.clone();
    state.config.osv_base_url = mock.url.clone();
    state.config.rustsec_db_dir = Some(root);
    let server = McpServer::new(state);
    for reversed in [false, true] {
        for osv in [reversed, !reversed] {
            let outcome = if osv {
                server
                    .sync_osv_security(slice::from_ref(&krate))
                    .await
            } else {
                server
                    .sync_rustsec_db_security(slice::from_ref(&krate))
                    .await
            }
            .expect("source sync");
            assert!(outcome.errors.is_empty());
        }
        let source_count: i64 = sqlx::query_scalar(
            "SELECT count(DISTINCT source) FROM advisory_matches WHERE crate_id=$1 AND \
             version_id=$2",
        )
        .bind(krate.id)
        .bind(stable)
        .fetch_one(&context.state.db)
        .await
        .expect("source count");
        assert_eq!(source_count, 2, "source evidence must survive either synchronization order");
        assert_eq!(
            counts(&context.state.db, krate.id, stable, "2.0.0")
                .await
                .affected,
            1
        );
    }
    write_advisory("withdrawn = \"2025-01-01\"");
    server
        .sync_rustsec_db_security(slice::from_ref(&krate))
        .await
        .expect("withdrawal sync");
    server
        .sync_osv_security(slice::from_ref(&krate))
        .await
        .expect("stale mirror sync");
    assert_eq!(
        counts(&context.state.db, krate.id, stable, "2.0.0").await,
        AdvisoryCounts::default()
    );

    // A malformed source refresh must retain the last complete source snapshot.
    fs::write(&path, "+++\ninvalid TOML\n+++\n").expect("malformed fixture");
    let outcome = server
        .sync_rustsec_db_security(slice::from_ref(&krate))
        .await
        .expect("failed parse outcome");
    assert!(!outcome.errors.is_empty());
    assert!(
        outcome
            .checked_crate_ids
            .is_empty()
    );
    assert_eq!(
        counts(&context.state.db, krate.id, stable, "2.0.0").await,
        AdvisoryCounts::default()
    );

    // Withdrawal supplied by OSV itself is also retained as a tombstone.
    mock.responses
        .write()
        .await
        .osv["vulns"][0]["withdrawn"] = json!("2025-01-01T00:00:00Z");
    server
        .sync_osv_security(slice::from_ref(&krate))
        .await
        .expect("OSV withdrawal");
    let osv_rows: Vec<(Option<i64>, String)> = sqlx::query_as(
        "SELECT version_id, affected_range FROM advisory_matches WHERE crate_id=$1 AND \
         source='rustsec_osv'",
    )
    .bind(krate.id)
    .fetch_all(&context.state.db)
    .await
    .expect("OSV tombstone");
    assert_eq!(osv_rows, vec![(None, "withdrawn=2025-01-01T00:00:00Z".to_string())]);
}

#[tokio::test]
async fn ghsa_prereleases_multiple_ranges_and_empty_refresh_are_consistent() {
    let context = common::seeded_mcp_context()
        .await
        .expect("context");
    let (krate, stable, pre) = seed_target(&context.state.db).await;
    let mock = SecurityServer::start().await;
    mock.responses
        .write()
        .await
        .ghsa = json!([{
        "ghsa_id": "GHSA-TEST", "vulnerabilities": [
            {"package": {"ecosystem": "npm", "name": TARGET}, "vulnerable_version_range": ">= 0.0.0"},
            {"package": {"ecosystem": "rust", "name": TARGET}, "vulnerable_version_range": "< 1.0.0"},
            {"package": {"ecosystem": "rust", "name": TARGET}, "vulnerable_version_range": ">= 1.1.0, < 2.0.0"}
        ]
    }]);
    let mut state = context.state.clone();
    state.config.github_base_url = mock.url.clone();
    let server = McpServer::new(state);
    server
        .sync_ghsa_security(slice::from_ref(&krate))
        .await
        .expect("GHSA sync");
    assert_eq!(
        counts(&context.state.db, krate.id, pre, "1.5.0-beta.1")
            .await
            .affected,
        1
    );
    assert_eq!(
        counts(&context.state.db, krate.id, stable, "2.0.0")
            .await
            .affected,
        0
    );
    let key = json!({"crate_name": TARGET}).to_string();
    server
        .query_cache_put("test", &key, &json!({"stale": true}), 600)
        .await
        .expect("cache fixture");
    mock.responses
        .write()
        .await
        .ghsa = json!([]);
    server
        .sync_ghsa_security(slice::from_ref(&krate))
        .await
        .expect("empty GHSA sync");
    assert_eq!(
        counts(&context.state.db, krate.id, pre, "1.5.0-beta.1").await,
        AdvisoryCounts::default()
    );
    assert_eq!(
        server
            .query_cache_get("test", &key)
            .await
            .expect("cache"),
        None
    );
}

#[tokio::test]
async fn upgrade_quarantines_old_matches_and_retries_failed_sources_without_skipping_crates() {
    let context = common::seeded_mcp_context()
        .await
        .expect("context");
    let (krate, stable, _) = seed_target(&context.state.db).await;
    let db = &context.state.db;
    // Reconstruct the pre-upgrade advisory schema in this disposable database.
    sqlx::raw_sql(
        "ALTER TABLE advisory_matches DROP COLUMN requires_resync;
        ALTER TABLE advisory_matches DROP CONSTRAINT advisory_matches_source_key;
        ALTER TABLE advisory_matches ADD CONSTRAINT \
         advisory_matches_crate_id_version_id_advisory_id_key UNIQUE (crate_id, version_id, \
         advisory_id);
        UPDATE crates SET security_checked_at=NOW();",
    )
    .execute(db)
    .await
    .expect("pre-upgrade schema");
    sqlx::query(
        "INSERT INTO advisory_matches (crate_id, version_id, advisory_id, title, affected_range, \
         source) VALUES ($1,$2,'LEGACY','Legacy match','< 3.0.0','ghsa')",
    )
    .bind(krate.id)
    .bind(stable)
    .execute(db)
    .await
    .expect("legacy explicit match");
    let cache = McpServer::new(context.state.clone());
    let key = json!({"crate_name": TARGET}).to_string();
    cache
        .query_cache_put("test", &key, &json!({"affected": true}), 600)
        .await
        .expect("legacy cache");
    sqlx::raw_sql(include_str!("../../../../../migrations/0024_advisory_source_evidence.sql"))
        .execute(db)
        .await
        .expect("upgrade");
    assert_eq!(
        counts(db, krate.id, stable, "2.0.0").await,
        AdvisoryCounts { affected: 0, unknown: 1 }
    );
    assert_eq!(
        cache
            .query_cache_get("test", &key)
            .await
            .expect("cache"),
        None
    );

    let mock = SecurityServer::start().await;
    mock.responses
        .write()
        .await
        .ghsa_status = StatusCode::INTERNAL_SERVER_ERROR;
    let mut state = context.state.clone();
    state.config.osv_base_url = mock.url.clone();
    state.config.github_base_url = mock.url.clone();
    state
        .config
        .security_sync_batch_size = 1;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM crates")
        .fetch_one(db)
        .await
        .expect("crate count");
    let failed = run_security_sync_scan_for_tests(&state).await;
    assert_eq!(failed.crates_processed, total as usize);
    assert!(failed.errors > 0);
    assert_eq!(
        counts(db, krate.id, stable, "2.0.0")
            .await
            .unknown,
        1,
        "failure must preserve unknown coverage"
    );
    assert!(
        fetch_security_crates_page(db, 100, 0, Some(86400))
            .await
            .expect("retry candidates")
            .iter()
            .any(|row| row.id == krate.id)
    );

    mock.responses
        .write()
        .await
        .ghsa_status = StatusCode::OK;
    let recovered = run_security_sync_scan_for_tests(&state).await;
    assert_eq!(recovered.errors, 0);
    assert_eq!(recovered.crates_processed, total as usize);
    assert_eq!(recovered.pages, total as usize, "one snapshot entry per batch");
    assert_eq!(counts(db, krate.id, stable, "2.0.0").await, AdvisoryCounts::default());
    assert!(
        fetch_security_crates_page(db, 100, 0, Some(86400))
            .await
            .expect("fresh crates")
            .is_empty()
    );
}

#[tokio::test]
async fn withdrawn_alias_cannot_withdraw_its_live_rustsec_owner() {
    let context = common::seeded_mcp_context()
        .await
        .expect("context");
    let (krate, stable, _) = seed_target(&context.state.db).await;
    let mock = SecurityServer::start().await;
    let mut state = context.state.clone();
    state.config.osv_base_url = mock.url.clone();
    let server = McpServer::new(state);
    // A previously imported alias tombstone must become unknown until refreshed.
    sqlx::query(
        "INSERT INTO advisory_matches (crate_id, advisory_id, title, affected_range, source) \
         VALUES ($1, 'RUSTSEC-2025-9999', 'Misattributed withdrawal', 'withdrawn=2025-01-01', \
         'rustsec_osv')",
    )
    .bind(krate.id)
    .execute(&context.state.db)
    .await
    .expect("old tombstone");
    sqlx::query("UPDATE crates SET security_checked_at=NOW() WHERE id=$1")
        .bind(krate.id)
        .execute(&context.state.db)
        .await
        .expect("old freshness");
    let key = json!({"crate_name": TARGET}).to_string();
    server
        .query_cache_put("test", &key, &json!({"affected": false}), 600)
        .await
        .expect("old cache");
    sqlx::raw_sql(include_str!("../../../../../migrations/0025_revalidate_osv_withdrawals.sql"))
        .execute(&context.state.db)
        .await
        .expect("revalidate old withdrawals");
    assert_eq!(
        counts(&context.state.db, krate.id, stable, "2.0.0").await,
        AdvisoryCounts { affected: 0, unknown: 1 }
    );
    assert!(
        server
            .query_cache_get("test", &key)
            .await
            .expect("cache")
            .is_none()
    );
    let stale: bool =
        sqlx::query_scalar("SELECT security_checked_at IS NULL FROM crates WHERE id=$1")
            .bind(krate.id)
            .fetch_one(&context.state.db)
            .await
            .expect("freshness");
    assert!(stale);
    let withdrawn = json!({
        "id": "GHSA-DUPLICATE", "aliases": ["RUSTSEC-2025-9999"],
        "withdrawn": "2025-01-01T00:00:00Z", "affected": []
    });
    for explicit in [false, true] {
        let live = json!({
            "id": "RUSTSEC-2025-9999", "affected": [{
                "package": {"ecosystem": "crates.io", "name": TARGET},
                "versions": if explicit { vec!["2.0.0"] } else { vec![] },
                "ranges": [{"type": "SEMVER", "events": [{"introduced": "0"}]}]
            }]
        });
        for reversed in [false, true] {
            let records = if reversed {
                vec![withdrawn.clone(), live.clone()]
            } else {
                vec![live.clone(), withdrawn.clone()]
            };
            mock.responses
                .write()
                .await
                .osv = json!({"vulns": records});
            server
                .sync_osv_security(slice::from_ref(&krate))
                .await
                .expect("OSV sync");
            let rows = tools::fetch_advisory_rows(&context.state.db, &[krate.id])
                .await
                .expect("rows");
            let applicable = advisories::advisories_for_version(&rows, stable, "2.0.0");
            assert_eq!(applicable.len(), 1, "explicit={explicit}, reversed={reversed}: {rows:?}");
            assert_eq!(applicable[0].row.advisory_id, "RUSTSEC-2025-9999");
            assert_eq!(applicable[0].applicability, advisories::Applicability::Affected);
            assert!(rows.iter().any(|row| {
                row.advisory_id == "GHSA-DUPLICATE"
                    && row
                        .affected_range
                        .starts_with("withdrawn=")
            }));
        }
    }
    let response = context
        .mcp
        .call_tool("crate_intel", json!({"crate_name": TARGET, "version": "2.0.0"}))
        .await
        .expect("intel");
    let intel = common::structured_content(&response);
    assert_eq!(intel["advisories"][0]["advisory_id"], "RUSTSEC-2025-9999");
    assert_eq!(intel["advisories"][0]["applicability"], "affected");
}

/// Waits for a queued security job's first failure or successful completion.
async fn wait_for_security_job(db: &PgPool, job_id: i64) -> (String, i32, Option<String>) {
    timeout(Duration::from_secs(20), async {
        loop {
            let row: (String, i32, Option<String>) =
                sqlx::query_as("SELECT status, attempts, last_error FROM refresh_jobs WHERE id=$1")
                    .bind(job_id)
                    .fetch_one(db)
                    .await
                    .expect("job state");
            if row.0 == "finished" || row.0 == "failed" || (row.0 == "pending" && row.2.is_some()) {
                return row;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("security job completed or retried")
}

#[tokio::test]
async fn queued_security_failures_retry_and_recover() {
    let context = common::seeded_mcp_context()
        .await
        .expect("context");
    let mock = SecurityServer::start().await;
    let mut state = context.state.clone();
    state.config.osv_base_url = mock.url.clone();
    state.config.github_base_url = mock.url.clone();
    for fail_osv in [true, false] {
        {
            let mut upstream = mock.responses.write().await;
            upstream.osv_status =
                if fail_osv { StatusCode::SERVICE_UNAVAILABLE } else { StatusCode::OK };
            upstream.ghsa_status = StatusCode::SERVICE_UNAVAILABLE;
        }
        let job_id: i64 = sqlx::query_scalar(
            "INSERT INTO refresh_jobs (crate_name, scope, priority, status, include_dependencies, \
             payload, requested_at) VALUES ('*', 'security', 10, 'pending', false, \
             '{\"per_page\":100}'::jsonb, NOW()) RETURNING id",
        )
        .fetch_one(&state.db)
        .await
        .expect("queue security job");
        let worker = tokio::spawn(run_refresh_worker_for_tests(state.clone()));
        let (status, attempts, error) = wait_for_security_job(&state.db, job_id).await;
        worker.abort();
        let _ = worker.await;
        assert_eq!(status, "pending", "failed source must retry: {error:?}");
        assert_eq!(attempts, 1);
        assert!(
            error
                .as_deref()
                .is_some_and(|error| error.contains("GHSA fetch failed")),
            "{error:?}"
        );
        if fail_osv {
            assert!(
                error
                    .as_deref()
                    .is_some_and(|error| error.contains("OSV")),
                "{error:?}"
            );
        }
        {
            let mut upstream = mock.responses.write().await;
            upstream.osv_status = StatusCode::OK;
            upstream.ghsa_status = StatusCode::OK;
        }
        sqlx::query("UPDATE refresh_jobs SET requested_at=NOW() WHERE id=$1")
            .bind(job_id)
            .execute(&state.db)
            .await
            .expect("retry now");
        let worker = tokio::spawn(run_refresh_worker_for_tests(state.clone()));
        // Await the retry before inspecting its terminal state.
        let recovered = timeout(Duration::from_secs(20), async {
            loop {
                let row: (String, i32, Option<String>) = sqlx::query_as(
                    "SELECT status, attempts, last_error FROM refresh_jobs WHERE id=$1",
                )
                .bind(job_id)
                .fetch_one(&state.db)
                .await
                .expect("retry state");
                if row.0 == "finished" || row.0 == "failed" {
                    break row;
                }
                sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("retry completion");
        worker.abort();
        let _ = worker.await;
        assert_eq!(recovered, ("finished".to_string(), 2, None));
    }
}
