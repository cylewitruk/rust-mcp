use super::server::McpServer;
use crate::db::tools;

impl McpServer {
    pub(crate) async fn record_tool_invocation(
        &self,
        tool_name: &str,
        success: bool,
        latency_ms: i64,
    ) -> Result<(), String> {
        tools::insert_tool_invocation(&self.state.db, tool_name, success, latency_ms)
            .await
            .map_err(|e| format!("failed to record tool invocation for {tool_name}: {e}"))?;

        Ok(())
    }
}
