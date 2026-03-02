use std::time::Duration;

use super::{
    json, mock_index_sync_context_with_rustdoc_dir, run_enrichment_maintenance_scan_for_tests,
    run_refresh_worker_for_tests, seed_crate_release, wait_for_refresh_job_terminal,
    write_rustdoc_fixture_file, write_rustdoc_fixture_files,
};

#[tokio::test]
async fn background_refresh_worker_processes_rustdoc_json_job() {
    let (rustdoc_dir, crate_name, crate_version) = write_rustdoc_fixture_file();
    let context = mock_index_sync_context_with_rustdoc_dir(Some(rustdoc_dir))
        .await
        .expect("failed to build rustdoc index context");

    let seeded = seed_crate_release(
        &context.state.db,
        &crate_name,
        &crate_version,
        42,
        Some("2026-01-01T00:00:00Z"),
    )
    .await
    .expect("failed to seed crate release for rustdoc worker test");
    assert!(seeded.version_id > 0);

    let job_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO refresh_jobs (
            crate_name,
            scope,
            priority,
            status,
            include_dependencies,
            payload,
            requested_at
         ) VALUES (
            $1,
            'rustdoc_json',
            10,
            'pending',
            true,
            $2::JSONB,
            NOW()
         )
         RETURNING id",
    )
    .bind("*")
    .bind(json!({ "crate_name": crate_name, "page": 1, "per_page": 20 }))
    .fetch_one(&context.state.db)
    .await
    .expect("failed to insert refresh_jobs row");

    let worker_handle = tokio::spawn(run_refresh_worker_for_tests(context.state.clone()));
    let (status, last_error) =
        wait_for_refresh_job_terminal(&context.state.db, job_id, Duration::from_secs(20)).await;
    worker_handle.abort();

    assert_eq!(status, "finished", "background rustdoc job failed with last_error={last_error:?}");

    let rustdoc_symbol_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*)::BIGINT
         FROM symbols s
         JOIN crate_versions cv ON cv.id = s.crate_version_id
         JOIN crates c ON c.id = cv.crate_id
         WHERE c.name = $1
           AND cv.version = $2
           AND s.index_source = 'rustdoc_json'",
    )
    .bind("demo-rustdoc")
    .bind("1.2.3")
    .fetch_one(&context.state.db)
    .await
    .expect("failed to count rustdoc symbols written by worker");
    assert!(rustdoc_symbol_count >= 1);
}

/// The enrichment maintenance scan enqueues rustdoc_json jobs for locally-
/// present versions that haven't been enriched yet, and the worker processes
/// them.
#[tokio::test]
async fn enrichment_maintenance_enqueues_rustdoc_jobs_for_unenriched_versions() {
    let (rustdoc_dir, crate_name, crate_versions) =
        write_rustdoc_fixture_files(&["1.2.3", "1.2.4"]);
    let context = mock_index_sync_context_with_rustdoc_dir(Some(rustdoc_dir))
        .await
        .expect("failed to build rustdoc index context");

    for (idx, crate_version) in crate_versions
        .iter()
        .enumerate()
    {
        let seeded = seed_crate_release(
            &context.state.db,
            &crate_name,
            crate_version,
            100 + idx as i64,
            Some("2026-01-01T00:00:00Z"),
        )
        .await
        .expect("failed to seed crate release for maintenance test");
        assert!(seeded.version_id > 0);
    }

    // Mark seeded versions as locally present so the maintenance scan picks
    // them up.
    sqlx::query(
        "UPDATE crate_versions SET locally_present = TRUE
         WHERE crate_id = (SELECT id FROM crates WHERE name = $1)",
    )
    .bind(&crate_name)
    .execute(&context.state.db)
    .await
    .expect("failed to mark seeded versions as locally present");

    // Run the maintenance scan — should enqueue a rustdoc_json job.
    let outcome = run_enrichment_maintenance_scan_for_tests(&context.state).await;
    assert!(outcome.scanned > 0, "maintenance scan should find unenriched crate names");
    assert!(outcome.enqueued > 0, "maintenance scan should enqueue at least one job");

    // Verify that a pending rustdoc_json job was actually enqueued.
    let pending_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*)::BIGINT
         FROM refresh_jobs
         WHERE crate_name = $1
           AND scope = 'rustdoc_json'
           AND status = 'pending'",
    )
    .bind(&crate_name)
    .fetch_one(&context.state.db)
    .await
    .expect("failed to count pending rustdoc_json jobs");
    assert!(pending_count >= 1, "expected at least one pending rustdoc_json job for {crate_name}");

    // Now let the worker process the enqueued job.
    let job_id = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM refresh_jobs
         WHERE crate_name = $1 AND scope = 'rustdoc_json' AND status = 'pending'
         ORDER BY id LIMIT 1",
    )
    .bind(&crate_name)
    .fetch_one(&context.state.db)
    .await
    .expect("failed to fetch enqueued job id");

    let worker_handle = tokio::spawn(run_refresh_worker_for_tests(context.state.clone()));
    let (status, last_error) =
        wait_for_refresh_job_terminal(&context.state.db, job_id, Duration::from_secs(20)).await;
    worker_handle.abort();

    assert_eq!(status, "finished", "enrichment job failed with last_error={last_error:?}");

    // Verify symbols were written for at least one version.
    for crate_version in &crate_versions {
        let rustdoc_symbol_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*)::BIGINT
             FROM symbols s
             JOIN crate_versions cv ON cv.id = s.crate_version_id
             JOIN crates c ON c.id = cv.crate_id
             WHERE c.name = $1
               AND cv.version = $2
               AND s.index_source = 'rustdoc_json'",
        )
        .bind(&crate_name)
        .bind(crate_version)
        .fetch_one(&context.state.db)
        .await
        .expect("failed to count rustdoc symbols written by worker");
        assert!(
            rustdoc_symbol_count >= 1,
            "expected rustdoc symbols for {crate_name}@{crate_version}"
        );
    }
}
