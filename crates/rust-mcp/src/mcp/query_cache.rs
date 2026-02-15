use serde_json::Value;
use tracing::warn;

use super::server::McpServer;

impl McpServer {
    pub(crate) async fn query_cache_get(
        &self,
        source: &str,
        key: &str,
    ) -> Result<Option<Value>, String> {
        let value = sqlx::query_scalar::<_, Value>(
            "SELECT value
             FROM query_cache
             WHERE key = $1 AND source = $2 AND expires_at > NOW()
             LIMIT 1",
        )
        .bind(key)
        .bind(source)
        .fetch_optional(&self.state.db)
        .await
        .map_err(|e| format!("query cache read failed for {source}:{key}: {e}"))?;

        if let Err(error) = sqlx::query(
            "INSERT INTO query_cache_events (source, hit, created_at)
             VALUES ($1, $2, NOW())",
        )
        .bind(source)
        .bind(value.is_some())
        .execute(&self.state.db)
        .await
        {
            warn!(%error, source, "failed to record query cache event");
        }

        Ok(value)
    }

    pub(crate) async fn query_cache_put(
        &self,
        source: &str,
        key: &str,
        value: &Value,
        ttl_seconds: i64,
    ) -> Result<(), String> {
        sqlx::query(
            "INSERT INTO query_cache (key, value, source, created_at, expires_at)
             VALUES ($1, $2, $3, NOW(), NOW() + ($4 * INTERVAL '1 second'))
             ON CONFLICT (key) DO UPDATE
             SET value = EXCLUDED.value,
                 source = EXCLUDED.source,
                 created_at = NOW(),
                 expires_at = EXCLUDED.expires_at",
        )
        .bind(key)
        .bind(value)
        .bind(source)
        .bind(ttl_seconds.max(1))
        .execute(&self.state.db)
        .await
        .map_err(|e| format!("query cache write failed for {source}:{key}: {e}"))?;

        Ok(())
    }
}
