use serde_json::Value;
use tracing::warn;

use super::server::McpServer;
use crate::db::tools;

impl McpServer {
    /// Reads a cached query result, returning `None` on cache miss.
    pub async fn query_cache_get(&self, source: &str, key: &str) -> Result<Option<Value>, String> {
        let value = tools::fetch_query_cache_value(&self.state.db, key, source)
            .await
            .map_err(|e| format!("query cache read failed for {source}:{key}: {e}"))?;

        if let Err(error) =
            tools::insert_query_cache_event(&self.state.db, source, value.is_some()).await
        {
            warn!(%error, source, "failed to record query cache event");
        }

        Ok(value)
    }

    /// Writes a query result into the cache with the given TTL.
    pub async fn query_cache_put(
        &self,
        source: &str,
        key: &str,
        value: &Value,
        ttl_seconds: i64,
    ) -> Result<(), String> {
        tools::upsert_query_cache_value(&self.state.db, key, source, value, ttl_seconds)
            .await
            .map_err(|e| format!("query cache write failed for {source}:{key}: {e}"))?;

        Ok(())
    }
}
