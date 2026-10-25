/// Database operations for legacy-protocol MCP session persistence.
use serde_json::Value;
use sqlx::PgPool;
use sqlx::types::Json;

/// Insert or replace a session's persisted state.
pub async fn upsert_session(
    db: &PgPool,
    session_id: &str,
    state: &Value,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO sessions (session_id, state) VALUES ($1, $2)
         ON CONFLICT (session_id) DO UPDATE SET state = EXCLUDED.state, last_seen_at = NOW()",
    )
    .bind(session_id)
    .bind(Json(state))
    .execute(db)
    .await?;
    Ok(())
}

/// Load a session's persisted state, marking it as seen.
pub async fn load_session(db: &PgPool, session_id: &str) -> Result<Option<Value>, sqlx::Error> {
    let state: Option<Json<Value>> = sqlx::query_scalar(
        "UPDATE sessions SET last_seen_at = NOW() WHERE session_id = $1 RETURNING state",
    )
    .bind(session_id)
    .fetch_optional(db)
    .await?;
    Ok(state.map(|Json(state)| state))
}

/// Mark the given sessions as seen.
pub async fn touch_sessions(db: &PgPool, session_ids: &[String]) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE sessions SET last_seen_at = NOW() WHERE session_id = ANY($1)")
        .bind(session_ids)
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
