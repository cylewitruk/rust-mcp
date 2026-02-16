use std::time::Duration;

use super::{
    json, mock_index_sync_context_with_rustdoc_dir, run_refresh_worker_for_tests,
    run_startup_rustdoc_json_refresh_for_tests, seed_crate_release, wait_for_refresh_job_terminal,
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

#[tokio::test]
async fn startup_rustdoc_json_refresh_processes_all_pages() {
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
        .expect("failed to seed crate release for startup rustdoc worker test");
        assert!(seeded.version_id > 0);
    }

    run_startup_rustdoc_json_refresh_for_tests(context.state.clone(), 1).await;

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
        .expect("failed to count rustdoc symbols written by startup refresh");
        assert!(
            rustdoc_symbol_count >= 1,
            "expected rustdoc symbols for {crate_name}@{crate_version}"
        );
    }
}
