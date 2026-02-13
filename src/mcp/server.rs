use rmcp::{
    Json, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
};

use crate::state::AppState;

use super::models::{
    CrateIntelRequest, CrateIntelResponse, CrateSearchRequest, CrateSearchResponse,
    IndexRefreshRequest, IndexRefreshResponse, IndexStatusRequest, IndexStatusResponse,
    IndexSyncCratesRequest, IndexSyncCratesResponse, PingRequest,
};

#[derive(Debug, Clone)]
pub struct McpServer {
    pub(super) state: AppState,
    tool_router: ToolRouter<Self>,
}

impl McpServer {
    pub fn new(state: AppState) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router(router = tool_router)]
impl McpServer {
    #[tool(
        name = "ping",
        description = "Check MCP connectivity and basic DB readiness."
    )]
    async fn ping(&self, Parameters(request): Parameters<PingRequest>) -> String {
        let suffix = request.message.unwrap_or_else(|| "ok".to_string());
        let db_state = match self.state.readiness_check().await {
            Ok(()) => "db_ready",
            Err(_) => "db_unavailable",
        };

        format!("pong ({db_state}) {suffix}")
    }

    #[tool(
        name = "index.sync_crates",
        description = "Fetch crate metadata from crates.io and upsert it into the local Postgres index."
    )]
    async fn index_sync_crates(
        &self,
        Parameters(request): Parameters<IndexSyncCratesRequest>,
    ) -> Result<Json<IndexSyncCratesResponse>, String> {
        self.handle_index_sync_crates(request).await
    }

    #[tool(
        name = "index.status",
        description = "Return index freshness, coverage, and queue state."
    )]
    async fn index_status(
        &self,
        Parameters(request): Parameters<IndexStatusRequest>,
    ) -> Result<Json<IndexStatusResponse>, String> {
        self.handle_index_status(request).await
    }

    #[tool(
        name = "index.refresh",
        description = "Trigger index refresh for a scope and return job-style status."
    )]
    async fn index_refresh(
        &self,
        Parameters(request): Parameters<IndexRefreshRequest>,
    ) -> Result<Json<IndexRefreshResponse>, String> {
        self.handle_index_refresh(request).await
    }

    #[tool(
        name = "crate.search",
        description = "Search locally indexed crates by name, category, keyword, and description."
    )]
    async fn crate_search(
        &self,
        Parameters(request): Parameters<CrateSearchRequest>,
    ) -> Result<Json<CrateSearchResponse>, String> {
        self.handle_crate_search(request).await
    }

    #[tool(
        name = "crate.intel",
        description = "Return dense crate intelligence including versions, dependencies, dependents, and advisory matches."
    )]
    async fn crate_intel(
        &self,
        Parameters(request): Parameters<CrateIntelRequest>,
    ) -> Result<Json<CrateIntelResponse>, String> {
        self.handle_crate_intel(request).await
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(
                "Local Rust dependency intelligence MCP server. Use index.sync_crates to ingest crates.io data, then crate.search and crate.intel for local fast lookup."
                    .to_string(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}
