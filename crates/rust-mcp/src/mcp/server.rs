use std::future::Future;
use std::time::Instant;

use metrics::{counter, histogram};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{Meta, ProgressNotificationParam, ServerCapabilities, ServerInfo};
use rmcp::{Json, Peer, RoleServer, ServerHandler, tool, tool_handler, tool_router};
use tokio::time::{Duration, sleep};
use tracing::warn;

use super::models::{
    CrateAlternativesRequest, CrateAlternativesResponse, CrateApiDiffRequest, CrateApiDiffResponse,
    CrateApiRequest, CrateApiResponse, CrateCompareRequest, CrateCompareResponse,
    CrateCompatibilityMatrixRequest, CrateCompatibilityMatrixResponse, CrateCompatibilityRequest,
    CrateCompatibilityResponse, CrateDeriveMacrosRequest, CrateDeriveMacrosResponse,
    CrateErrorTypesRequest, CrateErrorTypesResponse, CrateFeaturesRequest, CrateFeaturesResponse,
    CrateGraphRequest, CrateGraphResponse, CrateHotspotsRequest, CrateHotspotsResponse,
    CrateIntelRequest, CrateIntelResponse, CrateLicenseCheckRequest, CrateLicenseCheckResponse,
    CrateMigrationPathRequest, CrateMigrationPathResponse, CrateReExportsRequest,
    CrateReExportsResponse, CrateSearchRequest, CrateSearchResponse, CrateTraitImplsRequest,
    CrateTraitImplsResponse, CrateTypeInfoRequest, CrateTypeInfoResponse,
    CrateUsagePatternsRequest, CrateUsagePatternsResponse, CrateVersionsRequest,
    CrateVersionsResponse, DependencyAuditRequest, DependencyAuditResponse,
    DependencyFeatureImpactRequest, DependencyFeatureImpactResponse, DependencyResolveRequest,
    DependencyResolveResponse, DocsSearchRequest, DocsSearchResponse, IndexRefreshRequest,
    IndexRefreshResponse, IndexStatusRequest, IndexStatusResponse, IndexSyncCratesRequest,
    IndexSyncCratesResponse, PingRequest, SourceContextRequest, SourceContextResponse,
    SourceReadRequest, SourceReadResponse, SourceSearchRequest, SourceSearchResponse,
    SymbolSearchRequest, SymbolSearchResponse,
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

    async fn instrument_tool_with_progress<T, F>(
        &self,
        tool_name: &str,
        meta: &Meta,
        client: &Peer<RoleServer>,
        future: F,
    ) -> Result<T, String>
    where
        F: Future<Output = Result<T, String>>,
    {
        let progress_token = meta.get_progress_token();

        if let Some(token) = progress_token.clone() {
            let _ = client
                .notify_progress(ProgressNotificationParam {
                    progress_token: token,
                    progress: 0.0,
                    total: None,
                    message: Some(format!("{tool_name}: started")),
                })
                .await;
        }

        let tool_future = self.instrument_tool(tool_name, future);
        tokio::pin!(tool_future);

        let result = if let Some(token) = progress_token.clone() {
            let heartbeat = sleep(Duration::from_secs(5));
            tokio::pin!(heartbeat);

            tokio::select! {
                result = &mut tool_future => result,
                _ = &mut heartbeat => {
                    let _ = client
                        .notify_progress(ProgressNotificationParam {
                            progress_token: token,
                            progress: 0.5,
                            total: None,
                            message: Some(format!("{tool_name}: still running")),
                        })
                        .await;
                    tool_future.await
                }
            }
        } else {
            tool_future.await
        };

        if let Some(token) = progress_token {
            let _ = client
                .notify_progress(ProgressNotificationParam {
                    progress_token: token,
                    progress: 1.0,
                    total: Some(1.0),
                    message: Some(format!("{tool_name}: completed")),
                })
                .await;
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
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<IndexSyncCratesRequest>,
    ) -> Result<Json<IndexSyncCratesResponse>, String> {
        self.instrument_tool_with_progress(
            "index.sync_crates",
            &meta,
            &client,
            self.handle_index_sync_crates(request),
        )
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
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<IndexRefreshRequest>,
    ) -> Result<Json<IndexRefreshResponse>, String> {
        self.instrument_tool_with_progress(
            "index.refresh",
            &meta,
            &client,
            self.handle_index_refresh(request),
        )
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
        name = "crate.api",
        description = "Return indexed public API symbols for a crate version with optional \
                       kind/path filters."
    )]
    async fn crate_api(
        &self,
        Parameters(request): Parameters<CrateApiRequest>,
    ) -> Result<Json<CrateApiResponse>, String> {
        self.instrument_tool("crate.api", self.handle_crate_api(request))
            .await
    }

    #[tool(
        name = "crate.type_info",
        description = "Return indexed type definition metadata and associated impl details for a \
                       crate type."
    )]
    async fn crate_type_info(
        &self,
        Parameters(request): Parameters<CrateTypeInfoRequest>,
    ) -> Result<Json<CrateTypeInfoResponse>, String> {
        self.instrument_tool("crate.type_info", self.handle_crate_type_info(request))
            .await
    }

    #[tool(
        name = "crate.trait_impls",
        description = "Return indexed trait/type implementation relationships with optional trait \
                       or type filtering."
    )]
    async fn crate_trait_impls(
        &self,
        Parameters(request): Parameters<CrateTraitImplsRequest>,
    ) -> Result<Json<CrateTraitImplsResponse>, String> {
        self.instrument_tool("crate.trait_impls", self.handle_crate_trait_impls(request))
            .await
    }

    #[tool(
        name = "crate.re_exports",
        description = "Return public re-export mappings to canonical import paths for an indexed \
                       crate version."
    )]
    async fn crate_re_exports(
        &self,
        Parameters(request): Parameters<CrateReExportsRequest>,
    ) -> Result<Json<CrateReExportsResponse>, String> {
        self.instrument_tool("crate.re_exports", self.handle_crate_re_exports(request))
            .await
    }

    #[tool(
        name = "crate.error_types",
        description = "Return indexed error-type metadata, conversion impls, and functions \
                       returning each error type."
    )]
    async fn crate_error_types(
        &self,
        Parameters(request): Parameters<CrateErrorTypesRequest>,
    ) -> Result<Json<CrateErrorTypesResponse>, String> {
        self.instrument_tool("crate.error_types", self.handle_crate_error_types(request))
            .await
    }

    #[tool(
        name = "crate.derive_macros",
        description = "Return indexed proc-macro exports (derive, attribute, and function-like) \
                       for a crate version."
    )]
    async fn crate_derive_macros(
        &self,
        Parameters(request): Parameters<CrateDeriveMacrosRequest>,
    ) -> Result<Json<CrateDeriveMacrosResponse>, String> {
        self.instrument_tool("crate.derive_macros", self.handle_crate_derive_macros(request))
            .await
    }

    #[tool(
        name = "crate.compare",
        description = "Compare two crates across adoption, risk, and maintenance signals and \
                       return a recommendation."
    )]
    async fn crate_compare(
        &self,
        Parameters(request): Parameters<CrateCompareRequest>,
    ) -> Result<Json<CrateCompareResponse>, String> {
        self.instrument_tool("crate.compare", self.handle_crate_compare(request))
            .await
    }

    #[tool(
        name = "crate.compatibility",
        description = "Check pairwise dependency compatibility between two crates using the \
                       indexed resolver."
    )]
    async fn crate_compatibility(
        &self,
        Parameters(request): Parameters<CrateCompatibilityRequest>,
    ) -> Result<Json<CrateCompatibilityResponse>, String> {
        self.instrument_tool("crate.compatibility", self.handle_crate_compatibility(request))
            .await
    }

    #[tool(
        name = "crate.compatibility_matrix",
        description = "Evaluate compatibility across multiple version pairs between two crates \
                       using indexed resolver data."
    )]
    async fn crate_compatibility_matrix(
        &self,
        Parameters(request): Parameters<CrateCompatibilityMatrixRequest>,
    ) -> Result<Json<CrateCompatibilityMatrixResponse>, String> {
        self.instrument_tool(
            "crate.compatibility_matrix",
            self.handle_crate_compatibility_matrix(request),
        )
        .await
    }

    #[tool(
        name = "crate.migration_path",
        description = "Summarize migration actions for a crate upgrade using indexed API diff \
                       breaking changes."
    )]
    async fn crate_migration_path(
        &self,
        Parameters(request): Parameters<CrateMigrationPathRequest>,
    ) -> Result<Json<CrateMigrationPathResponse>, String> {
        self.instrument_tool("crate.migration_path", self.handle_crate_migration_path(request))
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
        name = "dependency.audit",
        description = "Audit a Cargo.toml dependency set for yanked versions, advisories, \
                       outdated requirements, and MSRV conflicts."
    )]
    async fn dependency_audit(
        &self,
        Parameters(request): Parameters<DependencyAuditRequest>,
    ) -> Result<Json<DependencyAuditResponse>, String> {
        self.instrument_tool("dependency.audit", self.handle_dependency_audit(request))
            .await
    }

    #[tool(
        name = "dependency.resolve",
        description = "Run a best-effort compatibility simulation for proposed dependencies and \
                       report resolvable versions or conflicts."
    )]
    async fn dependency_resolve(
        &self,
        Parameters(request): Parameters<DependencyResolveRequest>,
    ) -> Result<Json<DependencyResolveResponse>, String> {
        self.instrument_tool("dependency.resolve", self.handle_dependency_resolve(request))
            .await
    }

    #[tool(
        name = "dependency.feature_impact",
        description = "Estimate additional dependency surface introduced by selected crate \
                       feature flags."
    )]
    async fn dependency_feature_impact(
        &self,
        Parameters(request): Parameters<DependencyFeatureImpactRequest>,
    ) -> Result<Json<DependencyFeatureImpactResponse>, String> {
        self.instrument_tool(
            "dependency.feature_impact",
            self.handle_dependency_feature_impact(request),
        )
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
        name = "source.context",
        description = "Return semantic source context around a file location, including module \
                       path, imports, containing impl, and nearby types."
    )]
    async fn source_context(
        &self,
        Parameters(request): Parameters<SourceContextRequest>,
    ) -> Result<Json<SourceContextResponse>, String> {
        self.instrument_tool("source.context", self.handle_source_context(request))
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

    #[tool(
        name = "crate.usage_patterns",
        description = "Return real source snippets from indexed dependent crates that use a \
                       target symbol."
    )]
    async fn crate_usage_patterns(
        &self,
        Parameters(request): Parameters<CrateUsagePatternsRequest>,
    ) -> Result<Json<CrateUsagePatternsResponse>, String> {
        self.instrument_tool("crate.usage_patterns", self.handle_crate_usage_patterns(request))
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
