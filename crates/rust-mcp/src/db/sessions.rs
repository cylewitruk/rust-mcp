/// Database operations for MCP session persistence.
use sqlx::PgPool;

/// Insert a session ID, ignoring duplicates.
pub async fn insert_session(db: &PgPool, session_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO sessions (session_id) VALUES ($1) ON CONFLICT DO NOTHING")
        .bind(session_id)
        .execute(db)
        .await?;
    Ok(())
}

/// Check whether a session exists in the database.
pub async fn session_exists(db: &PgPool, session_id: &str) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sessions WHERE session_id = $1)")
        .bind(session_id)
        .fetch_one(db)
        .await
}

/// Update `last_seen_at` to the current time.
pub async fn touch_session(db: &PgPool, session_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE sessions SET last_seen_at = NOW() WHERE session_id = $1")
        .bind(session_id)
        .execute(db)
        .await?;
    Ok(())
}

/// Delete a session from the database.
pub async fn delete_session(db: &PgPool, session_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM sessions WHERE session_id = $1")
        .bind(session_id)
        .execute(db)
        .await?;
    Ok(())
}

/// Delete sessions idle for longer than `max_age_secs` and return the
/// number of rows removed.
pub async fn delete_stale_sessions(db: &PgPool, max_age_secs: i64) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        "DELETE FROM sessions
         WHERE last_seen_at < NOW() - make_interval(secs => $1::DOUBLE PRECISION)",
    )
    .bind(max_age_secs)
    .execute(db)
    .await?;
    Ok(result.rows_affected())
}
