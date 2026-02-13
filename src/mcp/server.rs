use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{ServerCapabilities, ServerInfo};
use rmcp::{Json, ServerHandler, tool, tool_handler, tool_router};

use super::models::{
    CrateGraphRequest, CrateGraphResponse, CrateIntelRequest, CrateIntelResponse,
    CrateSearchRequest, CrateSearchResponse, CrateVersionsRequest, CrateVersionsResponse,
    IndexRefreshRequest, IndexRefreshResponse, IndexStatusRequest, IndexStatusResponse,
    IndexSyncCratesRequest, IndexSyncCratesResponse, PingRequest, SourceReadRequest,
    SourceReadResponse, SourceSearchRequest, SourceSearchResponse, SymbolSearchRequest,
    SymbolSearchResponse,
};
use crate::state::AppState;

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
    #[tool(name = "ping", description = "Check MCP connectivity and basic DB readiness.")]
    async fn ping(&self, Parameters(request): Parameters<PingRequest>) -> String {
        let suffix = request
            .message
            .unwrap_or_else(|| "ok".to_string());
        let db_state = match self
            .state
            .readiness_check()
            .await
        {
            Ok(()) => "db_ready",
            Err(_) => "db_unavailable",
        };

        format!("pong ({db_state}) {suffix}")
    }

    #[tool(
        name = "index.sync_crates",
        description = "Fetch crate metadata from crates.io and upsert it into the local Postgres \
                       index."
    )]
    async fn index_sync_crates(
        &self,
        Parameters(request): Parameters<IndexSyncCratesRequest>,
    ) -> Result<Json<IndexSyncCratesResponse>, String> {
        self.handle_index_sync_crates(request)
            .await
    }

    #[tool(
        name = "index.status",
        description = "Return index freshness, coverage, and queue state."
    )]
    async fn index_status(
        &self,
        Parameters(request): Parameters<IndexStatusRequest>,
    ) -> Result<Json<IndexStatusResponse>, String> {
        self.handle_index_status(request)
            .await
    }

    #[tool(
        name = "index.refresh",
        description = "Trigger index refresh for a scope and return job-style status."
    )]
    async fn index_refresh(
        &self,
        Parameters(request): Parameters<IndexRefreshRequest>,
    ) -> Result<Json<IndexRefreshResponse>, String> {
        self.handle_index_refresh(request)
            .await
    }

    #[tool(
        name = "crate.search",
        description = "Search locally indexed crates by name, category, keyword, and description."
    )]
    async fn crate_search(
        &self,
        Parameters(request): Parameters<CrateSearchRequest>,
    ) -> Result<Json<CrateSearchResponse>, String> {
        self.handle_crate_search(request)
            .await
    }

    #[tool(
        name = "crate.intel",
        description = "Return dense crate intelligence including versions, dependencies, \
                       dependents, and advisory matches."
    )]
    async fn crate_intel(
        &self,
        Parameters(request): Parameters<CrateIntelRequest>,
    ) -> Result<Json<CrateIntelResponse>, String> {
        self.handle_crate_intel(request)
            .await
    }

    #[tool(
        name = "crate.versions",
        description = "Return a normalized crate version timeline with yanked/security/adoption \
                       markers."
    )]
    async fn crate_versions(
        &self,
        Parameters(request): Parameters<CrateVersionsRequest>,
    ) -> Result<Json<CrateVersionsResponse>, String> {
        self.handle_crate_versions(request)
            .await
    }

    #[tool(
        name = "crate.graph",
        description = "Return depth-bounded dependency/dependent graph edges and nodes for a \
                       crate."
    )]
    async fn crate_graph(
        &self,
        Parameters(request): Parameters<CrateGraphRequest>,
    ) -> Result<Json<CrateGraphResponse>, String> {
        self.handle_crate_graph(request)
            .await
    }

    #[tool(
        name = "source.search",
        description = "Search indexed source files by text/regex with optional crate/version/path \
                       filters."
    )]
    async fn source_search(
        &self,
        Parameters(request): Parameters<SourceSearchRequest>,
    ) -> Result<Json<SourceSearchResponse>, String> {
        self.handle_source_search(request)
            .await
    }

    #[tool(
        name = "source.read",
        description = "Read a line range from an indexed source file for a crate (optionally \
                       pinned to a version)."
    )]
    async fn source_read(
        &self,
        Parameters(request): Parameters<SourceReadRequest>,
    ) -> Result<Json<SourceReadResponse>, String> {
        self.handle_source_read(request)
            .await
    }

    #[tool(
        name = "symbol.search",
        description = "Search indexed symbols by name with optional crate/version/kind filters."
    )]
    async fn symbol_search(
        &self,
        Parameters(request): Parameters<SymbolSearchRequest>,
    ) -> Result<Json<SymbolSearchResponse>, String> {
        self.handle_symbol_search(request)
            .await
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(
                "Local Rust dependency intelligence MCP server. Use index.sync_crates to ingest \
                 crates.io data, then crate.search and crate.intel for local fast lookup."
                    .to_string(),
            ),
            capabilities: ServerCapabilities::builder()
                .enable_tools()
                .build(),
            ..Default::default()
        }
    }
}
