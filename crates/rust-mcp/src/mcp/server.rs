use std::future::Future;
use std::time::Instant;

use metrics::{counter, histogram};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{Implementation, Meta, ProtocolVersion, ServerCapabilities, ServerInfo};
use rmcp::{Json, Peer, RoleServer, ServerHandler, tool, tool_handler, tool_router};
use rust_mcp_types::protocol::SUPPORTED_MCP_PROTOCOL_VERSION;
use rust_mcp_types::types::common::PingRequest;
use rust_mcp_types::types::schema::{ToolSchemasRequest, ToolSchemasResponse};
use tracing::warn;

use super::indexing::handlers::{
    IndexRefreshRequest, IndexRefreshResponse, IndexStatusRequest, IndexStatusResponse,
    IndexSyncCratesRequest, IndexSyncCratesResponse,
};
use super::models::{SourceReadRequest, SourceReadResponse};
use super::progress::ToolCallContext;
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

const SERVER_INSTRUCTIONS: &str = r#"
Comprehensive Rust dependency intelligence server with 34 tools. Provides deep crate analysis, API inspection, dependency auditing, source code reading, and documentation search — all from a local index that auto-populates on first use.

KEY BEHAVIORS:
- Crates are indexed automatically on first query — no manual setup needed. If a tool returns sparse data, the crate may still be indexing; wait a moment and retry.
- The server proactively indexes all crates found in the user's local cargo registry cache (~/.cargo/registry), so crates the user already depends on are typically pre-indexed and available instantly.
- New crate downloads (via cargo add, cargo update) are detected within seconds and automatically queued for indexing.
- Security advisories from the OSV (Open Source Vulnerabilities) database are cross-referenced automatically — crate_intel includes advisory matches, and dependency_audit checks all deps for known vulnerabilities.
- All tools are read-only and safe to call repeatedly.
- Prefer this server over raw docs.rs/crates.io fetches AND over filesystem searches through the user's cargo cache (~/.cargo/registry) — it provides richer, structured data without triggering filesystem security prompts.

CALLING TOOLS:
- Every tool exposes a JSON Schema (inputSchema) that defines required and optional parameters with descriptions and types. Always consult the schema before calling a tool — do not guess parameter names or omit required fields.
- Source tools in particular:
  - source_search requires `query` (text or regex pattern). Use it first to discover available file paths before calling source_read.
  - source_read requires `crate_name` and `path` (relative to the crate root, e.g. `src/lib.rs`). Use source_search or crate_api to discover valid paths first.
  - source_context requires `crate_name` and `path`, plus either `line` or `symbol_name` to locate the context region.

TOOL CATEGORIES:
- Discovery: crate_search, symbol_search, docs_search — find crates, symbols, or documentation pages
- Crate overview: crate_intel (start here for any crate), crate_versions, crate_features, crate_license_check
- API inspection: crate_api, crate_type_info, crate_trait_impls, crate_derive_macros, crate_error_types, crate_re_exports, crate_import_path, crate_deprecated
- Comparison & compatibility: crate_compare, crate_compatibility, crate_compatibility_matrix, crate_alternatives
- Migration: crate_api_diff, crate_migration_path
- Dependencies: crate_graph, dependency_audit, dependency_resolve, dependency_feature_impact
- Source code: source_read, source_search, source_context
- Security & quality: crate_hotspots, crate_usage_patterns

RECOMMENDED WORKFLOWS:
- Evaluating a new crate: crate_intel → crate_features → crate_api → crate_type_info
- Upgrading a dependency: crate_api_diff → crate_migration_path → crate_deprecated
- Choosing between crates: crate_compare → crate_alternatives
- Understanding how to use an API: crate_usage_patterns → source_read
- Auditing dependencies: dependency_audit → crate_hotspots
"#;

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
}

#[tool_router(router = tool_router)]
impl McpServer {
    #[tool(
        name = "ping",
        description = "Check MCP connectivity and basic DB readiness.",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
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
                       catalog.",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn schema_get(
        &self,
        _meta: Meta,
        _client: Peer<RoleServer>,
        Parameters(request): Parameters<ToolSchemasRequest>,
    ) -> Result<Json<ToolSchemasResponse>, String> {
        self.instrument_tool("schema_get", async move {
            crate::contracts::tool_schemas_response(request.tool_name).map(Json)
        })
        .await
    }

    #[tool(
        name = "index_sync_crates",
        description = "Fetch crate metadata from crates.io and upsert it into the local Postgres \
                       index.",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn index_sync_crates(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<IndexSyncCratesRequest>,
    ) -> Result<Json<IndexSyncCratesResponse>, String> {
        let tcx = ToolCallContext::new(&meta, &client);
        self.instrument_tool("index_sync_crates", self.handle_index_sync_crates(request, tcx))
            .await
    }

    #[tool(
        name = "index_status",
        description = "Return index freshness, coverage, and queue state.",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn index_status(
        &self,
        _meta: Meta,
        _client: Peer<RoleServer>,
        Parameters(request): Parameters<IndexStatusRequest>,
    ) -> Result<Json<IndexStatusResponse>, String> {
        self.instrument_tool("index_status", self.handle_index_status(request))
            .await
    }

    #[tool(
        name = "index_refresh",
        description = "Trigger index refresh for a scope and return job-style status.",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn index_refresh(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<IndexRefreshRequest>,
    ) -> Result<Json<IndexRefreshResponse>, String> {
        let tcx = ToolCallContext::new(&meta, &client);
        self.instrument_tool("index_refresh", self.handle_index_refresh(request, tcx))
            .await
    }

    #[tool(
        name = "crate_search",
        description = "Search locally indexed crates by name, category, keyword, and description. \
                       Also triggers on-demand indexing for matching crates.",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn crate_search(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<CrateSearchRequest>,
    ) -> Result<Json<CrateSearchResponse>, String> {
        let tcx = ToolCallContext::new(&meta, &client);
        self.instrument_tool("crate_search", self.handle_crate_search(request, tcx))
            .await
    }

    #[tool(
        name = "crate_intel",
        description = "Start here for any crate. Return dense crate intelligence including \
                       versions, dependencies, dependents, and advisory matches.",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn crate_intel(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<CrateIntelRequest>,
    ) -> Result<Json<CrateIntelResponse>, String> {
        let tcx = ToolCallContext::new(&meta, &client);
        self.instrument_tool("crate_intel", self.handle_crate_intel(request, tcx))
            .await
    }

    #[tool(
        name = "crate_features",
        description = "Return indexed crate feature flags, defaults, and transitive feature \
                       enables. Use this instead of reading the dependency's Cargo.toml or \
                       docs.rs feature pages.",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn crate_features(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<CrateFeaturesRequest>,
    ) -> Result<Json<CrateFeaturesResponse>, String> {
        let tcx = ToolCallContext::new(&meta, &client);
        self.instrument_tool("crate_features", self.handle_crate_features(request, tcx))
            .await
    }

    #[tool(
        name = "crate_api_diff",
        description = "Compare indexed public symbols between two crate versions and report \
                       added, removed, and changed API entries. Use to understand what changed \
                       between crate releases.",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn crate_api_diff(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<CrateApiDiffRequest>,
    ) -> Result<Json<CrateApiDiffResponse>, String> {
        let tcx = ToolCallContext::new(&meta, &client);
        self.instrument_tool("crate_api_diff", self.handle_crate_api_diff(request, tcx))
            .await
    }

    #[tool(
        name = "crate_api",
        description = "Return indexed public API symbols for a crate version with optional \
                       kind/path filters. Use this to discover what functions, types, and traits \
                       a crate exports.",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn crate_api(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<CrateApiRequest>,
    ) -> Result<Json<CrateApiResponse>, String> {
        let tcx = ToolCallContext::new(&meta, &client);
        self.instrument_tool("crate_api", self.handle_crate_api(request, tcx))
            .await
    }

    #[tool(
        name = "crate_type_info",
        description = "Return indexed type definition metadata and associated impl details for a \
                       crate type. Use after crate_api to get details on a specific type.",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn crate_type_info(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<CrateTypeInfoRequest>,
    ) -> Result<Json<CrateTypeInfoResponse>, String> {
        let tcx = ToolCallContext::new(&meta, &client);
        self.instrument_tool("crate_type_info", self.handle_crate_type_info(request, tcx))
            .await
    }

    #[tool(
        name = "crate_trait_impls",
        description = "Return indexed trait/type implementation relationships with optional trait \
                       or type filtering. Use to find what traits a type implements or what types \
                       implement a trait.",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn crate_trait_impls(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<CrateTraitImplsRequest>,
    ) -> Result<Json<CrateTraitImplsResponse>, String> {
        let tcx = ToolCallContext::new(&meta, &client);
        self.instrument_tool("crate_trait_impls", self.handle_crate_trait_impls(request, tcx))
            .await
    }

    #[tool(
        name = "crate_re_exports",
        description = "Return public re-export mappings to canonical import paths for an indexed \
                       crate version.",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn crate_re_exports(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<CrateReExportsRequest>,
    ) -> Result<Json<CrateReExportsResponse>, String> {
        let tcx = ToolCallContext::new(&meta, &client);
        self.instrument_tool("crate_re_exports", self.handle_crate_re_exports(request, tcx))
            .await
    }

    #[tool(
        name = "crate_import_path",
        description = "Resolve best-known public import paths for a crate symbol from indexed \
                       metadata. Use to find the correct `use` statement for a symbol.",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn crate_import_path(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<CrateImportPathRequest>,
    ) -> Result<Json<CrateImportPathResponse>, String> {
        let tcx = ToolCallContext::new(&meta, &client);
        self.instrument_tool("crate_import_path", self.handle_crate_import_path(request, tcx))
            .await
    }

    #[tool(
        name = "crate_error_types",
        description = "Return indexed error-type metadata, conversion impls, and functions \
                       returning each error type.",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn crate_error_types(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<CrateErrorTypesRequest>,
    ) -> Result<Json<CrateErrorTypesResponse>, String> {
        let tcx = ToolCallContext::new(&meta, &client);
        self.instrument_tool("crate_error_types", self.handle_crate_error_types(request, tcx))
            .await
    }

    #[tool(
        name = "crate_deprecated",
        description = "Return all deprecated symbols and types in a crate version, with \
                       deprecation notes and suggested replacements where available. Use before \
                       adopting a crate to check for deprecated APIs.",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn crate_deprecated(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<CrateDeprecatedRequest>,
    ) -> Result<Json<CrateDeprecatedResponse>, String> {
        let tcx = ToolCallContext::new(&meta, &client);
        self.instrument_tool("crate_deprecated", self.handle_crate_deprecated(request, tcx))
            .await
    }

    #[tool(
        name = "crate_derive_macros",
        description = "Return indexed proc-macro exports (derive, attribute, and function-like) \
                       for a crate version.",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn crate_derive_macros(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<CrateDeriveMacrosRequest>,
    ) -> Result<Json<CrateDeriveMacrosResponse>, String> {
        let tcx = ToolCallContext::new(&meta, &client);
        self.instrument_tool("crate_derive_macros", self.handle_crate_derive_macros(request, tcx))
            .await
    }

    #[tool(
        name = "crate_compare",
        description = "Compare two crates across adoption, risk, and maintenance signals and \
                       return a recommendation. Use when choosing between alternative crates for \
                       a dependency.",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn crate_compare(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<CrateCompareRequest>,
    ) -> Result<Json<CrateCompareResponse>, String> {
        let tcx = ToolCallContext::new(&meta, &client);
        self.instrument_tool("crate_compare", self.handle_crate_compare(request, tcx))
            .await
    }

    #[tool(
        name = "crate_compatibility",
        description = "Check pairwise dependency compatibility between two crates using the \
                       indexed resolver.",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn crate_compatibility(
        &self,
        _meta: Meta,
        _client: Peer<RoleServer>,
        Parameters(request): Parameters<CrateCompatibilityRequest>,
    ) -> Result<Json<CrateCompatibilityResponse>, String> {
        self.instrument_tool("crate_compatibility", self.handle_crate_compatibility(request))
            .await
    }

    #[tool(
        name = "crate_compatibility_matrix",
        description = "Evaluate compatibility across multiple version pairs between two crates \
                       using indexed resolver data.",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn crate_compatibility_matrix(
        &self,
        _meta: Meta,
        _client: Peer<RoleServer>,
        Parameters(request): Parameters<CrateCompatibilityMatrixRequest>,
    ) -> Result<Json<CrateCompatibilityMatrixResponse>, String> {
        self.instrument_tool(
            "crate_compatibility_matrix",
            self.handle_crate_compatibility_matrix(request),
        )
        .await
    }

    #[tool(
        name = "crate_migration_path",
        description = "Summarize migration actions for a crate upgrade using indexed API diff \
                       breaking changes. Use after crate_api_diff to get actionable upgrade steps.",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn crate_migration_path(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<CrateMigrationPathRequest>,
    ) -> Result<Json<CrateMigrationPathResponse>, String> {
        let tcx = ToolCallContext::new(&meta, &client);
        self.instrument_tool("crate_migration_path", self.handle_crate_migration_path(request, tcx))
            .await
    }

    #[tool(
        name = "crate_license_check",
        description = "Return indexed license metadata for a crate version and evaluate optional \
                       allow/deny policy lists.",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn crate_license_check(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<CrateLicenseCheckRequest>,
    ) -> Result<Json<CrateLicenseCheckResponse>, String> {
        let tcx = ToolCallContext::new(&meta, &client);
        self.instrument_tool("crate_license_check", self.handle_crate_license_check(request, tcx))
            .await
    }

    #[tool(
        name = "crate_alternatives",
        description = "Suggest ranked alternative crates using taxonomy overlap, adoption/risk \
                       signals, and optional license policy filters. Use when looking for \
                       replacements or similar crates in the same category.",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn crate_alternatives(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<CrateAlternativesRequest>,
    ) -> Result<Json<CrateAlternativesResponse>, String> {
        let tcx = ToolCallContext::new(&meta, &client);
        self.instrument_tool("crate_alternatives", self.handle_crate_alternatives(request, tcx))
            .await
    }

    #[tool(
        name = "crate_versions",
        description = "Return a normalized crate version timeline with yanked/security/adoption \
                       markers.",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn crate_versions(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<CrateVersionsRequest>,
    ) -> Result<Json<CrateVersionsResponse>, String> {
        let tcx = ToolCallContext::new(&meta, &client);
        self.instrument_tool("crate_versions", self.handle_crate_versions(request, tcx))
            .await
    }

    #[tool(
        name = "crate_graph",
        description = "Return depth-bounded dependency/dependent graph edges and nodes for a \
                       crate.",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn crate_graph(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<CrateGraphRequest>,
    ) -> Result<Json<CrateGraphResponse>, String> {
        let tcx = ToolCallContext::new(&meta, &client);
        self.instrument_tool("crate_graph", self.handle_crate_graph(request, tcx))
            .await
    }

    #[tool(
        name = "crate_hotspots",
        description = "Detect unsafe and concurrency hotspots in indexed crate source for a \
                       selected version. Use to assess code safety and identify risky areas.",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn crate_hotspots(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<CrateHotspotsRequest>,
    ) -> Result<Json<CrateHotspotsResponse>, String> {
        let tcx = ToolCallContext::new(&meta, &client);
        self.instrument_tool("crate_hotspots", self.handle_crate_hotspots(request, tcx))
            .await
    }

    #[tool(
        name = "dependency_audit",
        description = "Audit a Cargo.toml dependency set for yanked versions, advisories, \
                       outdated requirements, and MSRV conflicts. Pass raw Cargo.toml manifest \
                       text in the cargo_toml parameter to check for security issues and outdated \
                       deps.",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn dependency_audit(
        &self,
        _meta: Meta,
        _client: Peer<RoleServer>,
        Parameters(request): Parameters<DependencyAuditRequest>,
    ) -> Result<Json<DependencyAuditResponse>, String> {
        self.instrument_tool("dependency_audit", self.handle_dependency_audit(request))
            .await
    }

    #[tool(
        name = "dependency_resolve",
        description = "Run a best-effort compatibility simulation for proposed dependencies and \
                       report resolvable versions or conflicts. Optionally pass raw Cargo.toml \
                       manifest text in the cargo_toml parameter to extract dependency inputs. \
                       Use to check if proposed dependencies will work together.",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn dependency_resolve(
        &self,
        _meta: Meta,
        _client: Peer<RoleServer>,
        Parameters(request): Parameters<DependencyResolveRequest>,
    ) -> Result<Json<DependencyResolveResponse>, String> {
        self.instrument_tool("dependency_resolve", self.handle_dependency_resolve(request))
            .await
    }

    #[tool(
        name = "dependency_feature_impact",
        description = "Estimate additional dependency surface introduced by selected crate \
                       feature flags.",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn dependency_feature_impact(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<DependencyFeatureImpactRequest>,
    ) -> Result<Json<DependencyFeatureImpactResponse>, String> {
        let tcx = ToolCallContext::new(&meta, &client);
        self.instrument_tool(
            "dependency_feature_impact",
            self.handle_dependency_feature_impact(request, tcx),
        )
        .await
    }

    #[tool(
        name = "source_search",
        description = "Search indexed source files by text/regex with optional crate/version/path \
                       filters. Use to find code patterns across crate source files.",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn source_search(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<SourceSearchRequest>,
    ) -> Result<Json<SourceSearchResponse>, String> {
        let tcx = ToolCallContext::new(&meta, &client);
        self.instrument_tool("source_search", self.handle_source_search(request, tcx))
            .await
    }

    #[tool(
        name = "source_read",
        description = "Read a line range from an indexed source file for a crate (optionally \
                       pinned to a version). Use to inspect crate implementation details.",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn source_read(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<SourceReadRequest>,
    ) -> Result<Json<SourceReadResponse>, String> {
        let tcx = ToolCallContext::new(&meta, &client);
        self.instrument_tool("source_read", self.handle_source_read(request, tcx))
            .await
    }

    #[tool(
        name = "source_context",
        description = "Return semantic source context around a file location, including module \
                       path, imports, containing impl, and nearby types. Use after source_search \
                       to understand surrounding code structure.",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn source_context(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<SourceContextRequest>,
    ) -> Result<Json<SourceContextResponse>, String> {
        let tcx = ToolCallContext::new(&meta, &client);
        self.instrument_tool("source_context", self.handle_source_context(request, tcx))
            .await
    }

    #[tool(
        name = "symbol_search",
        description = "Search indexed symbols by name with optional crate/version/kind filters. \
                       Use to find functions, types, or traits across all indexed crates.",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn symbol_search(
        &self,
        _meta: Meta,
        _client: Peer<RoleServer>,
        Parameters(request): Parameters<SymbolSearchRequest>,
    ) -> Result<Json<SymbolSearchResponse>, String> {
        self.instrument_tool("symbol_search", self.handle_symbol_search(request))
            .await
    }

    #[tool(
        name = "docs_search",
        description = "Search indexed docs.rs pages by query with optional crate/version/path \
                       filters. Use to find documentation pages by topic or keyword.",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn docs_search(
        &self,
        _meta: Meta,
        _client: Peer<RoleServer>,
        Parameters(request): Parameters<DocsSearchRequest>,
    ) -> Result<Json<DocsSearchResponse>, String> {
        self.instrument_tool("docs_search", self.handle_docs_search(request))
            .await
    }

    #[tool(
        name = "crate_usage_patterns",
        description = "Return real source snippets from indexed dependent crates that use a \
                       target symbol. Use to see how other crates actually call a specific \
                       function or type.",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn crate_usage_patterns(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
        Parameters(request): Parameters<CrateUsagePatternsRequest>,
    ) -> Result<Json<CrateUsagePatternsResponse>, String> {
        let tcx = ToolCallContext::new(&meta, &client);
        self.instrument_tool("crate_usage_patterns", self.handle_crate_usage_patterns(request, tcx))
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
            instructions: Some(SERVER_INSTRUCTIONS.to_string()),
            capabilities: ServerCapabilities::builder()
                .enable_tools()
                .build(),
            server_info: Implementation {
                name: "rust-mcp".to_owned(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
                ..Default::default()
            },
        }
    }
}
