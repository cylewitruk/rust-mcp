use std::future::Future;
use std::time::Instant;

use metrics::{counter, histogram};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    Meta, ProgressNotificationParam, ProtocolVersion, ServerCapabilities, ServerInfo,
};
use rmcp::{Json, Peer, RoleServer, ServerHandler, tool, tool_handler, tool_router};
use rust_mcp_types::protocol::SUPPORTED_MCP_PROTOCOL_VERSION;
use rust_mcp_types::types::common::PingRequest;
use rust_mcp_types::types::schema::{ToolSchemasRequest, ToolSchemasResponse};
use tokio::time::Duration;
use tracing::warn;

use super::indexing::handlers::{
    IndexRefreshRequest, IndexRefreshResponse, IndexStatusRequest, IndexStatusResponse,
    IndexSyncCratesRequest, IndexSyncCratesResponse,
};
use super::models::{SourceReadRequest, SourceReadResponse};
use super::tools::dependency::audit::{DependencyAuditRequest, DependencyAuditResponse};
use super::tools::dependency::feature_impact::{
    DependencyFeatureImpactRequest, DependencyFeatureImpactResponse,
};
use super::tools::dependency::resolve::{DependencyResolveRequest, DependencyResolveResponse};
use super::tools::docs::{DocsSearchRequest, DocsSearchResponse};
use super::tools::krate::alternatives::{CrateAlternativesRequest, CrateAlternativesResponse};
use super::tools::krate::api_diff::{CrateApiDiffRequest, CrateApiDiffResponse};
use super::tools::krate::api_surface::{CrateApiRequest, CrateApiResponse};
use super::tools::krate::compare::{CrateCompareRequest, CrateCompareResponse};
use super::tools::krate::compatibility::{
    CrateCompatibilityMatrixRequest, CrateCompatibilityMatrixResponse, CrateCompatibilityRequest,
    CrateCompatibilityResponse,
};
use super::tools::krate::deprecated::{CrateDeprecatedRequest, CrateDeprecatedResponse};
use super::tools::krate::derive_macros::{CrateDeriveMacrosRequest, CrateDeriveMacrosResponse};
use super::tools::krate::error_types::{CrateErrorTypesRequest, CrateErrorTypesResponse};
use super::tools::krate::features::{CrateFeaturesRequest, CrateFeaturesResponse};
use super::tools::krate::graph::{CrateGraphRequest, CrateGraphResponse};
use super::tools::krate::hotspots::{CrateHotspotsRequest, CrateHotspotsResponse};
use super::tools::krate::import_path::{CrateImportPathRequest, CrateImportPathResponse};
use super::tools::krate::intel::{CrateIntelRequest, CrateIntelResponse};
use super::tools::krate::license::{CrateLicenseCheckRequest, CrateLicenseCheckResponse};
use super::tools::krate::migration_path::{CrateMigrationPathRequest, CrateMigrationPathResponse};
use super::tools::krate::re_exports::{CrateReExportsRequest, CrateReExportsResponse};
use super::tools::krate::search::{CrateSearchRequest, CrateSearchResponse};
use super::tools::krate::trait_impls::{CrateTraitImplsRequest, CrateTraitImplsResponse};
use super::tools::krate::type_info::{CrateTypeInfoRequest, CrateTypeInfoResponse};
use super::tools::krate::usage_patterns::{CrateUsagePatternsRequest, CrateUsagePatternsResponse};
use super::tools::krate::versions::{CrateVersionsRequest, CrateVersionsResponse};
use super::tools::source::context::{SourceContextRequest, SourceContextResponse};
use super::tools::source::search::{SourceSearchRequest, SourceSearchResponse};
use super::tools::symbol::{SymbolSearchRequest, SymbolSearchResponse};
use crate::state::AppState;

/// MCP protocol server that registers and dispatches tool calls.
#[derive(Debug, Clone)]
pub struct McpServer {
    /// Shared application state.
    pub state: AppState,
    tool_router: ToolRouter<Self>,
}

impl McpServer {
    /// Creates a new server instance with tool routes initialized.
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
            let mut heartbeat = tokio::time::interval(Duration::from_secs(5));
            heartbeat.tick().await; // consume the immediate first tick

            loop {
                tokio::select! {
                    result = &mut tool_future => break result,
                    _ = heartbeat.tick() => {
                        let _ = client
                            .notify_progress(ProgressNotificationParam {
                                progress_token: token.clone(),
                                progress: 0.5,
                                total: None,
                                message: Some(format!("{tool_name}: still running")),
                            })
                            .await;
                    }
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
        name = "schema_get",
        description = "Return request/response JSON Schemas for one MCP tool or for the full tool \
                       catalog."
    )]
    async fn schema_get(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<ToolSchemasRequest>,
    ) -> Result<Json<ToolSchemasResponse>, String> {
        self.instrument_tool_with_progress("schema_get", &meta, &client, async move {
            crate::contracts::tool_schemas_response(request.tool_name).map(Json)
        })
        .await
    }

    #[tool(
        name = "index_sync_crates",
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
            "index_sync_crates",
            &meta,
            &client,
            self.handle_index_sync_crates(request),
        )
        .await
    }

    #[tool(
        name = "index_status",
        description = "Return index freshness, coverage, and queue state."
    )]
    async fn index_status(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<IndexStatusRequest>,
    ) -> Result<Json<IndexStatusResponse>, String> {
        self.instrument_tool_with_progress(
            "index_status",
            &meta,
            &client,
            self.handle_index_status(request),
        )
        .await
    }

    #[tool(
        name = "index_refresh",
        description = "Trigger index refresh for a scope and return job-style status."
    )]
    async fn index_refresh(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<IndexRefreshRequest>,
    ) -> Result<Json<IndexRefreshResponse>, String> {
        self.instrument_tool_with_progress(
            "index_refresh",
            &meta,
            &client,
            self.handle_index_refresh(request),
        )
        .await
    }

    #[tool(
        name = "crate_search",
        description = "Search locally indexed crates by name, category, keyword, and description."
    )]
    async fn crate_search(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<CrateSearchRequest>,
    ) -> Result<Json<CrateSearchResponse>, String> {
        self.instrument_tool_with_progress(
            "crate_search",
            &meta,
            &client,
            self.handle_crate_search(request),
        )
        .await
    }

    #[tool(
        name = "crate_intel",
        description = "Return dense crate intelligence including versions, dependencies, \
                       dependents, and advisory matches."
    )]
    async fn crate_intel(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<CrateIntelRequest>,
    ) -> Result<Json<CrateIntelResponse>, String> {
        self.instrument_tool_with_progress(
            "crate_intel",
            &meta,
            &client,
            self.handle_crate_intel(request),
        )
        .await
    }

    #[tool(
        name = "crate_features",
        description = "Return indexed crate feature flags, defaults, and transitive feature \
                       enables."
    )]
    async fn crate_features(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<CrateFeaturesRequest>,
    ) -> Result<Json<CrateFeaturesResponse>, String> {
        self.instrument_tool_with_progress(
            "crate_features",
            &meta,
            &client,
            self.handle_crate_features(request),
        )
        .await
    }

    #[tool(
        name = "crate_api_diff",
        description = "Compare indexed public symbols between two crate versions and report \
                       added, removed, and changed API entries."
    )]
    async fn crate_api_diff(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<CrateApiDiffRequest>,
    ) -> Result<Json<CrateApiDiffResponse>, String> {
        self.instrument_tool_with_progress(
            "crate_api_diff",
            &meta,
            &client,
            self.handle_crate_api_diff(request),
        )
        .await
    }

    #[tool(
        name = "crate_api",
        description = "Return indexed public API symbols for a crate version with optional \
                       kind/path filters."
    )]
    async fn crate_api(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<CrateApiRequest>,
    ) -> Result<Json<CrateApiResponse>, String> {
        self.instrument_tool_with_progress(
            "crate_api",
            &meta,
            &client,
            self.handle_crate_api(request),
        )
        .await
    }

    #[tool(
        name = "crate_type_info",
        description = "Return indexed type definition metadata and associated impl details for a \
                       crate type."
    )]
    async fn crate_type_info(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<CrateTypeInfoRequest>,
    ) -> Result<Json<CrateTypeInfoResponse>, String> {
        self.instrument_tool_with_progress(
            "crate_type_info",
            &meta,
            &client,
            self.handle_crate_type_info(request),
        )
        .await
    }

    #[tool(
        name = "crate_trait_impls",
        description = "Return indexed trait/type implementation relationships with optional trait \
                       or type filtering."
    )]
    async fn crate_trait_impls(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<CrateTraitImplsRequest>,
    ) -> Result<Json<CrateTraitImplsResponse>, String> {
        self.instrument_tool_with_progress(
            "crate_trait_impls",
            &meta,
            &client,
            self.handle_crate_trait_impls(request),
        )
        .await
    }

    #[tool(
        name = "crate_re_exports",
        description = "Return public re-export mappings to canonical import paths for an indexed \
                       crate version."
    )]
    async fn crate_re_exports(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<CrateReExportsRequest>,
    ) -> Result<Json<CrateReExportsResponse>, String> {
        self.instrument_tool_with_progress(
            "crate_re_exports",
            &meta,
            &client,
            self.handle_crate_re_exports(request),
        )
        .await
    }

    #[tool(
        name = "crate_import_path",
        description = "Resolve best-known public import paths for a crate symbol from indexed \
                       metadata."
    )]
    async fn crate_import_path(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<CrateImportPathRequest>,
    ) -> Result<Json<CrateImportPathResponse>, String> {
        self.instrument_tool_with_progress(
            "crate_import_path",
            &meta,
            &client,
            self.handle_crate_import_path(request),
        )
        .await
    }

    #[tool(
        name = "crate_error_types",
        description = "Return indexed error-type metadata, conversion impls, and functions \
                       returning each error type."
    )]
    async fn crate_error_types(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<CrateErrorTypesRequest>,
    ) -> Result<Json<CrateErrorTypesResponse>, String> {
        self.instrument_tool_with_progress(
            "crate_error_types",
            &meta,
            &client,
            self.handle_crate_error_types(request),
        )
        .await
    }

    #[tool(
        name = "crate_deprecated",
        description = "Return all deprecated symbols and types in a crate version, with \
                       deprecation notes and suggested replacements where available."
    )]
    async fn crate_deprecated(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<CrateDeprecatedRequest>,
    ) -> Result<Json<CrateDeprecatedResponse>, String> {
        self.instrument_tool_with_progress(
            "crate_deprecated",
            &meta,
            &client,
            self.handle_crate_deprecated(request),
        )
        .await
    }

    #[tool(
        name = "crate_derive_macros",
        description = "Return indexed proc-macro exports (derive, attribute, and function-like) \
                       for a crate version."
    )]
    async fn crate_derive_macros(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<CrateDeriveMacrosRequest>,
    ) -> Result<Json<CrateDeriveMacrosResponse>, String> {
        self.instrument_tool_with_progress(
            "crate_derive_macros",
            &meta,
            &client,
            self.handle_crate_derive_macros(request),
        )
        .await
    }

    #[tool(
        name = "crate_compare",
        description = "Compare two crates across adoption, risk, and maintenance signals and \
                       return a recommendation."
    )]
    async fn crate_compare(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<CrateCompareRequest>,
    ) -> Result<Json<CrateCompareResponse>, String> {
        self.instrument_tool_with_progress(
            "crate_compare",
            &meta,
            &client,
            self.handle_crate_compare(request),
        )
        .await
    }

    #[tool(
        name = "crate_compatibility",
        description = "Check pairwise dependency compatibility between two crates using the \
                       indexed resolver."
    )]
    async fn crate_compatibility(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<CrateCompatibilityRequest>,
    ) -> Result<Json<CrateCompatibilityResponse>, String> {
        self.instrument_tool_with_progress(
            "crate_compatibility",
            &meta,
            &client,
            self.handle_crate_compatibility(request),
        )
        .await
    }

    #[tool(
        name = "crate_compatibility_matrix",
        description = "Evaluate compatibility across multiple version pairs between two crates \
                       using indexed resolver data."
    )]
    async fn crate_compatibility_matrix(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<CrateCompatibilityMatrixRequest>,
    ) -> Result<Json<CrateCompatibilityMatrixResponse>, String> {
        self.instrument_tool_with_progress(
            "crate_compatibility_matrix",
            &meta,
            &client,
            self.handle_crate_compatibility_matrix(request),
        )
        .await
    }

    #[tool(
        name = "crate_migration_path",
        description = "Summarize migration actions for a crate upgrade using indexed API diff \
                       breaking changes."
    )]
    async fn crate_migration_path(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<CrateMigrationPathRequest>,
    ) -> Result<Json<CrateMigrationPathResponse>, String> {
        self.instrument_tool_with_progress(
            "crate_migration_path",
            &meta,
            &client,
            self.handle_crate_migration_path(request),
        )
        .await
    }

    #[tool(
        name = "crate_license_check",
        description = "Return indexed license metadata for a crate version and evaluate optional \
                       allow/deny policy lists."
    )]
    async fn crate_license_check(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<CrateLicenseCheckRequest>,
    ) -> Result<Json<CrateLicenseCheckResponse>, String> {
        self.instrument_tool_with_progress(
            "crate_license_check",
            &meta,
            &client,
            self.handle_crate_license_check(request),
        )
        .await
    }

    #[tool(
        name = "crate_alternatives",
        description = "Suggest ranked alternative crates using taxonomy overlap, adoption/risk \
                       signals, and optional license policy filters."
    )]
    async fn crate_alternatives(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<CrateAlternativesRequest>,
    ) -> Result<Json<CrateAlternativesResponse>, String> {
        self.instrument_tool_with_progress(
            "crate_alternatives",
            &meta,
            &client,
            self.handle_crate_alternatives(request),
        )
        .await
    }

    #[tool(
        name = "crate_versions",
        description = "Return a normalized crate version timeline with yanked/security/adoption \
                       markers."
    )]
    async fn crate_versions(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<CrateVersionsRequest>,
    ) -> Result<Json<CrateVersionsResponse>, String> {
        self.instrument_tool_with_progress(
            "crate_versions",
            &meta,
            &client,
            self.handle_crate_versions(request),
        )
        .await
    }

    #[tool(
        name = "crate_graph",
        description = "Return depth-bounded dependency/dependent graph edges and nodes for a \
                       crate."
    )]
    async fn crate_graph(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<CrateGraphRequest>,
    ) -> Result<Json<CrateGraphResponse>, String> {
        self.instrument_tool_with_progress(
            "crate_graph",
            &meta,
            &client,
            self.handle_crate_graph(request),
        )
        .await
    }

    #[tool(
        name = "crate_hotspots",
        description = "Detect unsafe and concurrency hotspots in indexed crate source for a \
                       selected version."
    )]
    async fn crate_hotspots(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<CrateHotspotsRequest>,
    ) -> Result<Json<CrateHotspotsResponse>, String> {
        self.instrument_tool_with_progress(
            "crate_hotspots",
            &meta,
            &client,
            self.handle_crate_hotspots(request),
        )
        .await
    }

    #[tool(
        name = "dependency_audit",
        description = "Audit a Cargo.toml dependency set for yanked versions, advisories, \
                       outdated requirements, and MSRV conflicts. Pass raw Cargo.toml manifest \
                       text in the cargo_toml parameter."
    )]
    async fn dependency_audit(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<DependencyAuditRequest>,
    ) -> Result<Json<DependencyAuditResponse>, String> {
        self.instrument_tool_with_progress(
            "dependency_audit",
            &meta,
            &client,
            self.handle_dependency_audit(request),
        )
        .await
    }

    #[tool(
        name = "dependency_resolve",
        description = "Run a best-effort compatibility simulation for proposed dependencies and \
                       report resolvable versions or conflicts. Optionally pass raw Cargo.toml \
                       manifest text in the cargo_toml parameter to extract dependency inputs."
    )]
    async fn dependency_resolve(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<DependencyResolveRequest>,
    ) -> Result<Json<DependencyResolveResponse>, String> {
        self.instrument_tool_with_progress(
            "dependency_resolve",
            &meta,
            &client,
            self.handle_dependency_resolve(request),
        )
        .await
    }

    #[tool(
        name = "dependency_feature_impact",
        description = "Estimate additional dependency surface introduced by selected crate \
                       feature flags."
    )]
    async fn dependency_feature_impact(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<DependencyFeatureImpactRequest>,
    ) -> Result<Json<DependencyFeatureImpactResponse>, String> {
        self.instrument_tool_with_progress(
            "dependency_feature_impact",
            &meta,
            &client,
            self.handle_dependency_feature_impact(request),
        )
        .await
    }

    #[tool(
        name = "source_search",
        description = "Search indexed source files by text/regex with optional crate/version/path \
                       filters."
    )]
    async fn source_search(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<SourceSearchRequest>,
    ) -> Result<Json<SourceSearchResponse>, String> {
        self.instrument_tool_with_progress(
            "source_search",
            &meta,
            &client,
            self.handle_source_search(request),
        )
        .await
    }

    #[tool(
        name = "source_read",
        description = "Read a line range from an indexed source file for a crate (optionally \
                       pinned to a version)."
    )]
    async fn source_read(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<SourceReadRequest>,
    ) -> Result<Json<SourceReadResponse>, String> {
        self.instrument_tool_with_progress(
            "source_read",
            &meta,
            &client,
            self.handle_source_read(request),
        )
        .await
    }

    #[tool(
        name = "source_context",
        description = "Return semantic source context around a file location, including module \
                       path, imports, containing impl, and nearby types."
    )]
    async fn source_context(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<SourceContextRequest>,
    ) -> Result<Json<SourceContextResponse>, String> {
        self.instrument_tool_with_progress(
            "source_context",
            &meta,
            &client,
            self.handle_source_context(request),
        )
        .await
    }

    #[tool(
        name = "symbol_search",
        description = "Search indexed symbols by name with optional crate/version/kind filters."
    )]
    async fn symbol_search(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<SymbolSearchRequest>,
    ) -> Result<Json<SymbolSearchResponse>, String> {
        self.instrument_tool_with_progress(
            "symbol_search",
            &meta,
            &client,
            self.handle_symbol_search(request),
        )
        .await
    }

    #[tool(
        name = "docs_search",
        description = "Search indexed docs.rs pages by query with optional crate/version/path \
                       filters."
    )]
    async fn docs_search(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<DocsSearchRequest>,
    ) -> Result<Json<DocsSearchResponse>, String> {
        self.instrument_tool_with_progress(
            "docs_search",
            &meta,
            &client,
            self.handle_docs_search(request),
        )
        .await
    }

    #[tool(
        name = "crate_usage_patterns",
        description = "Return real source snippets from indexed dependent crates that use a \
                       target symbol."
    )]
    async fn crate_usage_patterns(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<CrateUsagePatternsRequest>,
    ) -> Result<Json<CrateUsagePatternsResponse>, String> {
        self.instrument_tool_with_progress(
            "crate_usage_patterns",
            &meta,
            &client,
            self.handle_crate_usage_patterns(request),
        )
        .await
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerInfo {
        // Construct ProtocolVersion from the canonical constant.  The inner field of
        // ProtocolVersion is private (rmcp crate), so we go through serde; unknown
        // version strings are accepted and wrapped in Cow::Owned by the Deserialize
        // impl.
        let protocol_version = serde_json::from_value::<ProtocolVersion>(
            serde_json::Value::String(SUPPORTED_MCP_PROTOCOL_VERSION.to_string()),
        )
        .unwrap_or(ProtocolVersion::V_2025_06_18);

        ServerInfo {
            protocol_version,
            instructions: Some(
                "Local Rust dependency intelligence MCP server. Crates are indexed on-demand when \
                 first requested. Use crate_search and crate_intel for fast lookup. The index_* \
                 tools are available for explicit bulk sync and status inspection."
                    .to_string(),
            ),
            capabilities: ServerCapabilities::builder()
                .enable_tools()
                .build(),
            ..Default::default()
        }
    }
}
