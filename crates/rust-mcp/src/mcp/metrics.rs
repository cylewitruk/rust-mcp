use super::server::McpServer;

impl McpServer {
    pub(crate) async fn record_tool_invocation(
        &self,
        tool_name: &str,
        success: bool,
        latency_ms: i64,
    ) -> Result<(), String> {
        sqlx::query(
            "INSERT INTO tool_invocations (tool_name, success, latency_ms, created_at)
             VALUES ($1, $2, $3, NOW())",
        )
        .bind(tool_name)
        .bind(success)
        .bind(latency_ms.max(0))
        .execute(&self.state.db)
        .await
        .map_err(|e| format!("failed to record tool invocation for {tool_name}: {e}"))?;

        Ok(())
    }
}
