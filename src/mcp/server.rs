use std::future::Future;
use std::time::Instant;

use metrics::{counter, histogram};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{ServerCapabilities, ServerInfo};
use rmcp::{Json, ServerHandler, tool, tool_handler, tool_router};
use tracing::warn;

use super::models::{
    CrateAlternativesRequest, CrateAlternativesResponse, CrateApiDiffRequest, CrateApiDiffResponse,
    CrateFeaturesRequest, CrateFeaturesResponse, CrateGraphRequest, CrateGraphResponse,
    CrateHotspotsRequest, CrateHotspotsResponse, CrateIntelRequest, CrateIntelResponse,
    CrateLicenseCheckRequest, CrateLicenseCheckResponse, CrateSearchRequest, CrateSearchResponse,
    CrateVersionsRequest, CrateVersionsResponse, DocsSearchRequest, DocsSearchResponse,
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

    async fn instrument_tool<T, F>(&self, tool_name: &str, future: F) -> Result<T, String>
    where
        F: Future<Output = Result<T, String>>,
    {
        let started = Instant::now();
        let result = future.await;
        let latency_ms = started.elapsed().as_millis() as i64;
        let success_label = if result.is_ok() { "true" } else { "false" };

        counter!(
            "rust_mcp_tool_invocations_total",
            "tool" => tool_name.to_string(),
            "success" => success_label.to_string(),
        )
        .increment(1);
        histogram!(
            "rust_mcp_tool_latency_ms",
            "tool" => tool_name.to_string(),
        )
        .record(latency_ms.max(0) as f64);

        if let Err(error) = self
            .record_tool_invocation(tool_name, result.is_ok(), latency_ms)
            .await
        {
            warn!(%error, tool_name, "failed to persist tool invocation metrics");
        }
        result
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
        self.instrument_tool("index.sync_crates", self.handle_index_sync_crates(request))
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
        self.instrument_tool("index.status", self.handle_index_status(request))
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
        self.instrument_tool("index.refresh", self.handle_index_refresh(request))
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
        self.instrument_tool("crate.search", self.handle_crate_search(request))
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
        self.instrument_tool("crate.intel", self.handle_crate_intel(request))
            .await
    }

    #[tool(
        name = "crate.features",
        description = "Return indexed crate feature flags, defaults, and transitive feature \
                       enables."
    )]
    async fn crate_features(
        &self,
        Parameters(request): Parameters<CrateFeaturesRequest>,
    ) -> Result<Json<CrateFeaturesResponse>, String> {
        self.instrument_tool("crate.features", self.handle_crate_features(request))
            .await
    }

    #[tool(
        name = "crate.api_diff",
        description = "Compare indexed public symbols between two crate versions and report \
                       added, removed, and changed API entries."
    )]
    async fn crate_api_diff(
        &self,
        Parameters(request): Parameters<CrateApiDiffRequest>,
    ) -> Result<Json<CrateApiDiffResponse>, String> {
        self.instrument_tool("crate.api_diff", self.handle_crate_api_diff(request))
            .await
    }

    #[tool(
        name = "crate.license_check",
        description = "Return indexed license metadata for a crate version and evaluate optional \
                       allow/deny policy lists."
    )]
    async fn crate_license_check(
        &self,
        Parameters(request): Parameters<CrateLicenseCheckRequest>,
    ) -> Result<Json<CrateLicenseCheckResponse>, String> {
        self.instrument_tool("crate.license_check", self.handle_crate_license_check(request))
            .await
    }

    #[tool(
        name = "crate.alternatives",
        description = "Suggest ranked alternative crates using taxonomy overlap, adoption/risk \
                       signals, and optional license policy filters."
    )]
    async fn crate_alternatives(
        &self,
        Parameters(request): Parameters<CrateAlternativesRequest>,
    ) -> Result<Json<CrateAlternativesResponse>, String> {
        self.instrument_tool("crate.alternatives", self.handle_crate_alternatives(request))
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
        self.instrument_tool("crate.versions", self.handle_crate_versions(request))
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
        self.instrument_tool("crate.graph", self.handle_crate_graph(request))
            .await
    }

    #[tool(
        name = "crate.hotspots",
        description = "Detect unsafe and concurrency hotspots in indexed crate source for a \
                       selected version."
    )]
    async fn crate_hotspots(
        &self,
        Parameters(request): Parameters<CrateHotspotsRequest>,
    ) -> Result<Json<CrateHotspotsResponse>, String> {
        self.instrument_tool("crate.hotspots", self.handle_crate_hotspots(request))
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
        self.instrument_tool("source.search", self.handle_source_search(request))
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
        self.instrument_tool("source.read", self.handle_source_read(request))
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
        self.instrument_tool("symbol.search", self.handle_symbol_search(request))
            .await
    }

    #[tool(
        name = "docs.search",
        description = "Search indexed docs.rs pages by query with optional crate/version/path \
                       filters."
    )]
    async fn docs_search(
        &self,
        Parameters(request): Parameters<DocsSearchRequest>,
    ) -> Result<Json<DocsSearchResponse>, String> {
        self.instrument_tool("docs.search", self.handle_docs_search(request))
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
