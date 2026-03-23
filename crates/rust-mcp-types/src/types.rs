use rmcp::schemars;
use serde::{Deserialize, Serialize};

/// Core/shared contracts reused by multiple MCP tools.
pub mod common {
    use super::*;

    /// Request payload for the `ping` tool.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct PingRequest {
        /// Optional pong message suffix.
        pub message: Option<String>,
    }

    /// Freshness provenance attached to responses that perform freshness
    /// checks.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct ResponseFreshnessSource {
        /// Upstream or local source name for freshness probing.
        pub source: String,
        /// Freshness status for the source.
        pub status: String,
        /// RFC3339 timestamp when freshness was checked.
        pub checked_at: Option<String>,
    }

    /// Coarse confidence level used across tool responses.
    #[derive(Debug, Clone, Copy, Deserialize, Serialize, schemars::JsonSchema)]
    #[serde(rename_all = "lowercase")]
    pub enum ConfidenceLevel {
        /// High confidence.
        High,
        /// Medium confidence.
        Medium,
        /// Low confidence.
        Low,
    }

    impl ConfidenceLevel {
        /// Returns the wire-format lowercase string value.
        pub const fn as_str(self) -> &'static str {
            match self {
                Self::High => "high",
                Self::Medium => "medium",
                Self::Low => "low",
            }
        }
    }

    /// Structured confidence assessment included in tool responses.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct ConfidenceAssessment {
        /// Coarse confidence level.
        pub level: ConfidenceLevel,
        /// Human-readable rationale for the confidence level.
        pub reason: String,
    }

    impl Default for ConfidenceAssessment {
        fn default() -> Self {
            Self {
                level: ConfidenceLevel::Low,
                reason: "confidence assessment unavailable in cached legacy response".to_string(),
            }
        }
    }

    /// License policy evaluation result.
    #[derive(Debug, Clone, Copy, Deserialize, Serialize, schemars::JsonSchema)]
    #[serde(rename_all = "snake_case")]
    pub enum LicensePolicyResult {
        /// Allowed.
        Allowed,
        /// Denied.
        Denied,
        /// Unknown.
        Unknown,
    }
}

/// Contracts for schema introspection MCP tools.
pub mod schema {
    use super::*;

    /// Request payload for `schema_get`.
    #[derive(Debug, Clone, Default, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct ToolSchemasRequest {
        /// Tool name filter (omit for all).
        pub tool_name: Option<String>,
    }

    /// Request/response JSON Schemas for one MCP tool.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct ToolSchemaContract {
        /// MCP tool name (for example `crate_search`).
        pub tool_name: String,
        /// JSON Schema for the tool request payload.
        pub request: schemars::Schema,
        /// JSON Schema for the tool response payload.
        pub response: schemars::Schema,
    }

    /// Response payload for `schema_get`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct ToolSchemasResponse {
        /// Echoed filter for tool selection, if provided.
        pub tool_name: Option<String>,
        /// Number of schema entries returned.
        pub total_tools: usize,
        /// Schema entries for the selected tools.
        pub schemas: Vec<ToolSchemaContract>,
    }
}

/// Contracts for `index.*` MCP tools.
pub mod index {
    use super::common::ResponseFreshnessSource;
    use super::*;

    /// A single crate to index.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct IndexCrateEntry {
        /// Crate name (e.g. `serde`).
        pub name: String,
        /// Version to index (default: latest).
        pub version: Option<String>,
    }

    /// Request payload for `index_crates`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct IndexCratesRequest {
        /// Crates to index.
        pub crates: Vec<IndexCrateEntry>,
        /// Also fetch dependency metadata.
        pub include_dependencies: Option<bool>,
    }

    /// Per-crate result from `index_crates`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct IndexCrateResult {
        /// Crate name.
        pub name: String,
        /// Version that was indexed (e.g. `"1.0.204"`).
        pub version: Option<String>,
        /// Number of versions synchronized.
        pub synced_versions: usize,
        /// Number of dependency edges synchronized.
        pub synced_dependencies: usize,
        /// Error message if this crate failed to index.
        pub error: Option<String>,
    }

    /// Response payload for `index_crates`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct IndexCratesResponse {
        /// Number of crates requested.
        pub requested: usize,
        /// Number of crates successfully indexed.
        pub succeeded: usize,
        /// Number of crates that failed to index.
        pub failed: usize,
        /// Per-crate results.
        pub results: Vec<IndexCrateResult>,
        /// Freshness sources.
        pub freshness: Vec<ResponseFreshnessSource>,
        /// Data origin.
        pub provenance: String,
    }

    /// Refresh scope for `index_refresh`.
    #[derive(Debug, Clone, Copy, Deserialize, Serialize, schemars::JsonSchema)]
    #[serde(rename_all = "snake_case")]
    pub enum IndexRefreshScope {
        /// Crates.io metadata.
        Crate,
        /// All pipelines.
        All,
        /// Security advisories.
        Security,
        /// Docs.rs pages.
        Docs,
        /// Local cache symbols.
        LocalCache,
        /// Rustdoc JSON data.
        RustdocJson,
    }

    /// Request payload for `index_refresh`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct IndexRefreshRequest {
        /// Refresh scope.
        pub scope: Option<IndexRefreshScope>,
        /// Filter by crate.
        pub crate_name: Option<String>,
        /// Search query for candidates.
        pub query: Option<String>,
        /// Page (1-based).
        pub page: Option<u32>,
        /// Page size.
        pub per_page: Option<u32>,
        /// Also sync dependency metadata.
        pub include_dependencies: Option<bool>,
    }

    /// Response payload for `index_refresh`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct IndexRefreshResponse {
        /// Durable job identifier for the refresh request.
        pub job_id: String,
        /// Scope the job is executing.
        pub scope: IndexRefreshScope,
        /// Whether the request was accepted.
        pub accepted: bool,
        /// Current job status.
        pub status: String,
        /// Human-readable status message.
        pub message: String,
        /// Estimated total job duration in seconds.
        pub estimated_seconds: Option<u32>,
        /// Estimated remaining duration in seconds.
        pub estimated_seconds_remaining: Option<u32>,
        /// Epoch milliseconds when processing started.
        pub started_at_epoch_ms: u128,
        /// Epoch milliseconds when processing finished.
        pub finished_at_epoch_ms: Option<u128>,
        /// Job result when processing is complete.
        pub result: Option<IndexRefreshResult>,
        /// Freshness sources.
        pub freshness: Vec<ResponseFreshnessSource>,
        /// Data origin.
        pub provenance: String,
    }

    /// Result payload for completed `index_refresh` jobs.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct IndexRefreshResult {
        /// Number of crates synchronized by the job.
        pub synced_crates: usize,
        /// Number of crate versions synchronized by the job.
        pub synced_versions: usize,
        /// Number of dependency edges synchronized by the job.
        pub synced_dependencies: usize,
        /// Selected crate versions in `crate@version` format.
        pub selected_versions: Vec<String>,
        /// Errors encountered by the job.
        pub errors: Vec<String>,
        /// Rustdoc-only type rows written.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub synced_types: Option<usize>,
        /// Rustdoc-only impl rows written.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub synced_impls: Option<usize>,
        /// Rustdoc-only trait rows written.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub synced_traits: Option<usize>,
    }

    /// Request payload for `index_status`.
    #[derive(Debug, Clone, Default, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct IndexStatusRequest {}

    /// Response payload for `index_status`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct IndexStatusResponse {
        /// Snapshot of source freshness timestamps.
        pub freshness: IndexFreshness,
        /// Coverage counters for major indexed entities.
        pub coverage: IndexCoverage,
        /// Query/latency/error metrics for recent traffic.
        pub operational_metrics: IndexOperationalMetrics,
        /// Queue depth and state counters.
        pub queue: IndexQueue,
        /// Retry-attempt breakdown for active and failed jobs.
        pub retry_distribution: IndexRetryDistribution,
        /// Failures grouped by refresh scope.
        pub failures_by_scope: Vec<IndexFailureByScope>,
        /// Most recent refresh worker errors.
        pub last_errors: Vec<String>,
        /// Data origin.
        pub provenance: String,
    }

    /// Operational metrics from the recent observation window.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct IndexOperationalMetrics {
        /// Window descriptor.
        pub window: String,
        /// Total query count in the window.
        pub query_count: i64,
        /// Average query latency in milliseconds.
        pub average_latency_ms: Option<f64>,
        /// Fraction of failed queries.
        pub error_rate: Option<f64>,
        /// Fraction of cache hits.
        pub cache_hit_rate: Option<f64>,
        /// Estimated lag between upstream and local index.
        pub index_lag_seconds: Option<i64>,
    }

    /// Freshness timestamps for major indexed assets.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct IndexFreshness {
        /// Last crates metadata update timestamp.
        pub crates_updated_at: Option<String>,
        /// Last source indexing timestamp.
        pub source_indexed_at: Option<String>,
        /// Last symbol indexing timestamp.
        pub symbols_indexed_at: Option<String>,
        /// Last docs indexing timestamp.
        pub docs_indexed_at: Option<String>,
        /// Last advisory synchronization timestamp.
        pub advisories_updated_at: Option<String>,
    }

    /// Coverage counters for indexed entities.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct IndexCoverage {
        /// Total crate rows.
        pub crates: i64,
        /// Total crate version rows.
        pub crate_versions: i64,
        /// Total dependency edge rows.
        pub dependency_edges: i64,
        /// Total advisory-match rows.
        pub advisory_matches: i64,
        /// Total source file rows.
        pub source_files: i64,
        /// Total symbol rows.
        pub symbols: i64,
        /// Total docs page rows.
        pub docs_pages: i64,
    }

    /// Refresh queue state counters.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct IndexQueue {
        /// Jobs pending execution.
        pub pending_jobs: i64,
        /// Jobs delayed for retry or scheduling.
        pub delayed_jobs: i64,
        /// Jobs currently retrying.
        pub retrying_jobs: i64,
        /// Jobs currently running.
        pub running_jobs: i64,
        /// Jobs that failed terminally.
        pub failed_jobs: i64,
    }

    /// Retry-attempt distribution for refresh jobs.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct IndexRetryDistribution {
        /// In-flight jobs on attempt 1.
        pub inflight_attempt_1: i64,
        /// In-flight jobs on attempt 2.
        pub inflight_attempt_2: i64,
        /// In-flight jobs on attempt 3+.
        pub inflight_attempt_3_plus: i64,
        /// Failed jobs on attempt 1.
        pub failed_attempt_1: i64,
        /// Failed jobs on attempt 2.
        pub failed_attempt_2: i64,
        /// Failed jobs on attempt 3+.
        pub failed_attempt_3_plus: i64,
    }

    /// Failed job count for a single refresh scope.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct IndexFailureByScope {
        /// Scope identifier.
        pub scope: String,
        /// Number of failed jobs for the scope.
        pub failed_jobs: i64,
    }
}

/// Contracts for `source.*` MCP tools.
pub mod source {
    use super::common::{ConfidenceAssessment, ResponseFreshnessSource};
    use super::*;

    /// Search mode for source content matching.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema)]
    #[serde(rename_all = "lowercase")]
    pub enum SourceSearchMode {
        /// Substring match.
        #[serde(alias = "contains")]
        Text,
        /// Regex match.
        Regex,
    }

    /// Request payload for `source_search`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct SourceSearchRequest {
        /// Text or regex pattern.
        pub query: String,
        /// Filter by crate.
        pub crate_name: Option<String>,
        /// Filter by version.
        pub version: Option<String>,
        /// Path glob filter.
        pub path_glob: Option<String>,
        /// Matching mode.
        pub mode: Option<SourceSearchMode>,
        /// Paging cursor.
        pub cursor: Option<String>,
        /// Page (1-based).
        pub page: Option<u32>,
        /// Max results.
        pub limit: Option<u32>,
    }

    /// Response payload for `source_search`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct SourceSearchResponse {
        /// Effective query value.
        pub query: String,
        /// Effective crate filter.
        pub crate_name: Option<String>,
        /// Effective version filter.
        pub version: Option<String>,
        /// Effective path glob filter.
        pub path_glob: Option<String>,
        /// Effective matching mode.
        pub mode: SourceSearchMode,
        /// Current cursor.
        pub cursor: Option<String>,
        /// Next page cursor.
        pub next_cursor: Option<String>,
        /// Page.
        pub page: u32,
        /// Effective result limit.
        pub limit: u32,
        /// More results available.
        pub has_more: bool,
        /// Truncated by pagination.
        pub truncated: bool,
        /// Returned hit count.
        pub count: usize,
        /// Confidence level.
        pub confidence: String,
        /// Confidence details.
        pub confidence_assessment: ConfidenceAssessment,
        /// Suggested next tools.
        pub suggested_next_tools: Vec<String>,
        /// Data origin.
        pub provenance: String,
        /// Matched source hits.
        pub hits: Vec<SourceSearchHit>,
    }

    /// One `source_search` result.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct SourceSearchHit {
        /// Crate name containing the match.
        pub crate_name: String,
        /// Crate version containing the match.
        pub version: String,
        /// Source file path containing the match.
        pub path: String,
        /// Timestamp when the source was indexed.
        pub indexed_at: String,
        /// Best-effort matching line number.
        pub match_line: Option<u32>,
        /// Snippet around the match.
        pub snippet: String,
    }

    /// Request payload for `source_read`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct SourceReadRequest {
        /// Crate name.
        pub crate_name: String,
        /// Version (default: latest).
        pub version: Option<String>,
        /// Source path.
        pub path: String,
        /// Start line (inclusive).
        pub start_line: Option<u32>,
        /// End line (inclusive).
        pub end_line: Option<u32>,
    }

    /// Response payload for `source_read`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct SourceReadResponse {
        /// Crate name resolved for the read.
        pub crate_name: String,
        /// Crate version resolved for the read.
        pub version: String,
        /// Source path that was read.
        pub path: String,
        /// Inclusive start line in the response content.
        pub start_line: u32,
        /// Inclusive end line in the response content.
        pub end_line: u32,
        /// Total line count in the full source file.
        pub total_lines: u32,
        /// Source text content for the requested range.
        pub content: String,
        /// Confidence level.
        pub confidence: String,
        /// Confidence details.
        pub confidence_assessment: ConfidenceAssessment,
        /// Suggested next tools.
        pub suggested_next_tools: Vec<String>,
        /// Data origin.
        pub provenance: String,
    }

    /// Request payload for `source_context`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct SourceContextRequest {
        /// Crate name.
        pub crate_name: String,
        /// Version (default: latest).
        pub version: Option<String>,
        /// Source path.
        pub path: String,
        /// Anchor line.
        pub line: Option<u32>,
        /// Symbol name to resolve anchor.
        pub symbol_name: Option<String>,
    }

    /// Response payload for `source_context`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct SourceContextResponse {
        /// Crate name resolved for context extraction.
        pub crate_name: String,
        /// Version selected for context extraction.
        pub selected_version: String,
        /// Latest indexed crate version.
        pub latest_version: String,
        /// Source path inspected.
        pub path: String,
        /// Effective anchor line used for context extraction.
        pub line: u32,
        /// Optional symbol name used to resolve the anchor line.
        pub symbol_name: Option<String>,
        /// Module path derived from the source path.
        pub module_path: String,
        /// `use` statements in scope before the anchor line.
        pub imports_in_scope: Vec<String>,
        /// Containing impl block around the anchor line.
        pub containing_impl: Option<SourceContextImplBlock>,
        /// Surrounding type declarations near the anchor line.
        pub surrounding_types: Vec<SourceContextTypeContext>,
        /// Freshness check done.
        pub freshness_check_performed: bool,
        /// Freshness result.
        pub freshness_check_result: String,
        /// Refresh enqueued.
        pub refresh_enqueued: bool,
        /// Refresh job ID.
        pub refresh_job_id: Option<String>,
        /// Freshness sources.
        pub freshness: Vec<ResponseFreshnessSource>,
        /// Confidence level.
        pub confidence: String,
        /// Confidence details.
        pub confidence_assessment: ConfidenceAssessment,
        /// Suggested next tools.
        pub suggested_next_tools: Vec<String>,
        /// Data origin.
        pub provenance: String,
    }

    /// Impl block context around a source line.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct SourceContextImplBlock {
        /// Impl target type name.
        pub type_name: String,
        /// Optional rendered impl target type.
        pub type_name_display: Option<String>,
        /// Optional trait name implemented by the block.
        pub trait_name: Option<String>,
        /// Optional rendered trait name.
        pub trait_name_display: Option<String>,
        /// Impl kind label.
        pub impl_kind: String,
        /// Source line where the impl block starts.
        pub source_line: i32,
    }

    /// Type declaration context around a source line.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct SourceContextTypeContext {
        /// Type name.
        pub type_name: String,
        /// Type kind label.
        pub kind: String,
        /// Source line where the declaration starts.
        pub source_line: i32,
    }
}

/// Contracts for `symbol_search`.
pub mod symbol {
    use super::common::ConfidenceAssessment;
    use super::*;

    /// Request payload for `symbol_search`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct SymbolSearchRequest {
        /// Query string.
        pub query: String,
        /// Filter by crate.
        pub crate_name: Option<String>,
        /// Filter by version.
        pub version: Option<String>,
        /// Filter by kind.
        pub kind: Option<String>,
        /// Include all versions.
        pub include_all_versions: Option<bool>,
        /// Collapse duplicate paths.
        pub collapse_by_canonical: Option<bool>,
        /// Paging cursor.
        pub cursor: Option<String>,
        /// Page (1-based).
        pub page: Option<u32>,
        /// Max results.
        pub limit: Option<u32>,
    }

    /// Response payload for `symbol_search`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct SymbolSearchResponse {
        /// Effective query string.
        pub query: String,
        /// Effective crate filter.
        pub crate_name: Option<String>,
        /// Effective version filter.
        pub version: Option<String>,
        /// Effective kind filter.
        pub kind: Option<String>,
        /// Effective all-versions behavior.
        pub include_all_versions: bool,
        /// Effective canonical-path collapsing behavior.
        pub collapse_by_canonical: bool,
        /// Current cursor.
        pub cursor: Option<String>,
        /// Next page cursor.
        pub next_cursor: Option<String>,
        /// Page.
        pub page: u32,
        /// Effective page size.
        pub limit: u32,
        /// Total number of matching rows before pagination.
        pub total_count: usize,
        /// More results available.
        pub has_more: bool,
        /// Number of hits returned in this page.
        pub count: usize,
        /// Confidence level.
        pub confidence: String,
        /// Confidence details.
        #[serde(default)]
        pub confidence_assessment: ConfidenceAssessment,
        /// Suggested next tools.
        pub suggested_next_tools: Vec<String>,
        /// Data origin.
        pub provenance: String,
        /// Matched symbol hits.
        pub hits: Vec<SymbolSearchHit>,
    }

    /// One `symbol_search` result.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct SymbolSearchHit {
        /// Crate name containing the symbol.
        pub crate_name: String,
        /// Crate version containing the symbol.
        pub version: String,
        /// Source file path where the symbol was indexed.
        pub source_path: String,
        /// Symbol name.
        pub name: String,
        /// Symbol kind.
        pub kind: String,
        /// Optional rendered signature.
        pub signature: Option<String>,
        /// Optional visibility label.
        pub visibility: Option<String>,
        /// Source start line.
        pub start_line: i32,
        /// Source end line.
        pub end_line: i32,
        /// Index source label.
        pub index_source: String,
        /// Timestamp when the symbol was indexed.
        pub indexed_at: String,
    }
}

/// Contracts for `docs_search`.
pub mod docs {
    use super::common::ConfidenceAssessment;
    use super::*;

    /// Request payload for `docs_search`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct DocsSearchRequest {
        /// Query string.
        pub query: String,
        /// Filter by crate.
        pub crate_name: Option<String>,
        /// Filter by version.
        pub version: Option<String>,
        /// Path prefix filter.
        pub path_prefix: Option<String>,
        /// Paging cursor.
        pub cursor: Option<String>,
        /// Page (1-based).
        pub page: Option<u32>,
        /// Max results.
        pub limit: Option<u32>,
    }

    /// Response payload for `docs_search`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct DocsSearchResponse {
        /// Effective query string.
        pub query: String,
        /// Effective crate filter.
        pub crate_name: Option<String>,
        /// Effective version filter.
        pub version: Option<String>,
        /// Effective path prefix filter.
        pub path_prefix: Option<String>,
        /// Current cursor.
        pub cursor: Option<String>,
        /// Next page cursor.
        pub next_cursor: Option<String>,
        /// Page.
        pub page: u32,
        /// Effective result limit.
        pub limit: u32,
        /// More results available.
        pub has_more: bool,
        /// Truncated by pagination.
        pub truncated: bool,
        /// Number of hits returned.
        pub count: usize,
        /// Confidence level.
        pub confidence: String,
        /// Confidence details.
        #[serde(default)]
        pub confidence_assessment: ConfidenceAssessment,
        /// Suggested next tools.
        pub suggested_next_tools: Vec<String>,
        /// Data origin.
        pub provenance: String,
        /// Matched docs page hits.
        pub hits: Vec<DocsSearchHit>,
    }

    /// One `docs_search` result.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct DocsSearchHit {
        /// Crate name containing the docs page.
        pub crate_name: String,
        /// Crate version containing the docs page.
        pub version: String,
        /// Indexed docs page path.
        pub path: String,
        /// Optional page title extracted from HTML.
        pub title: Option<String>,
        /// Optional source URL for the page.
        pub source_url: Option<String>,
        /// Timestamp when the page was indexed.
        pub indexed_at: String,
        /// Snippet around the query match.
        pub snippet: String,
    }
}

/// Contracts for `dependency.*` MCP tools.
pub mod dependency {
    use super::common::{ConfidenceAssessment, ResponseFreshnessSource};
    use super::*;

    /// Request payload for `dependency_audit`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct DependencyAuditRequest {
        /// Cargo.toml manifest content.
        pub cargo_toml: String,
    }

    /// Issue category emitted by `dependency_audit`.
    #[derive(
        Debug,
        Clone,
        Copy,
        Deserialize,
        Serialize,
        schemars::JsonSchema,
        PartialEq,
        Eq,
        PartialOrd,
        Ord,
    )]
    #[serde(rename_all = "snake_case")]
    pub enum DependencyAuditIssueCategory {
        /// Yanked version.
        Yanked,
        /// Advisory match.
        Advisory,
        /// Outdated version.
        Outdated,
        /// MSRV conflict.
        MsrvConflict,
        /// Unresolved.
        Unresolved,
    }

    /// Severity emitted by `dependency_audit`.
    #[derive(Debug, Clone, Copy, Deserialize, Serialize, schemars::JsonSchema, PartialEq, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum DependencyAuditSeverity {
        /// Low.
        Low,
        /// Medium.
        Medium,
        /// High.
        High,
    }

    /// Response payload for `dependency_audit`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct DependencyAuditResponse {
        /// Package name declared in the manifest.
        pub package_name: Option<String>,
        /// Package rust-version declared in the manifest.
        pub package_rust_version: Option<String>,
        /// Number of dependencies evaluated.
        pub dependency_count: usize,
        /// Number of issues detected.
        pub issue_count: usize,
        /// Per-dependency audit details.
        pub dependencies: Vec<DependencyAuditDependency>,
        /// Issues detected across dependencies.
        pub issues: Vec<DependencyAuditIssue>,
        /// Freshness sources.
        pub freshness: Vec<ResponseFreshnessSource>,
        /// Confidence level.
        pub confidence: String,
        /// Confidence details.
        pub confidence_assessment: ConfidenceAssessment,
        /// Suggested next tools.
        pub suggested_next_tools: Vec<String>,
        /// Data origin.
        pub provenance: String,
    }

    /// Per-dependency audit details.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct DependencyAuditDependency {
        /// Dependency crate name.
        pub dependency_name: String,
        /// Version requirement from the manifest.
        pub requirement: Option<String>,
        /// Selected version resolved from index data.
        pub selected_version: Option<String>,
        /// Latest available indexed version.
        pub latest_version: Option<String>,
        /// rust-version for the selected version.
        pub selected_rust_version: Option<String>,
        /// Whether the selected version is yanked.
        pub yanked: bool,
        /// Number of advisory matches for the selected version.
        pub advisory_count: i64,
        /// Status markers describing notable conditions.
        pub status_markers: Vec<String>,
    }

    /// One `dependency_audit` issue.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct DependencyAuditIssue {
        /// Dependency crate name.
        pub dependency_name: String,
        /// Issue category.
        pub category: DependencyAuditIssueCategory,
        /// Issue severity.
        pub severity: DependencyAuditSeverity,
        /// Human-readable issue summary.
        pub message: String,
        /// Selected version involved in the issue.
        pub selected_version: Option<String>,
        /// Latest version involved in the issue.
        pub latest_version: Option<String>,
    }

    /// Request payload for `dependency_resolve`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct DependencyResolveRequest {
        /// Dependency inputs.
        pub dependencies: Option<Vec<DependencyResolveInputDependency>>,
        /// Cargo.toml manifest text to extract deps from.
        #[serde(default)]
        pub cargo_toml: Option<String>,
        /// Extra dependencies over manifest inputs.
        pub additions: Option<Vec<DependencyResolveInputDependency>>,
        /// Check feature unification.
        pub check_features: Option<bool>,
        /// Expansion depth limit.
        pub limit: Option<u32>,
    }

    /// Dependency input.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct DependencyResolveInputDependency {
        /// Crate name.
        pub name: String,
        /// Semver requirement.
        pub version_req: Option<String>,
    }

    /// Response payload for `dependency_resolve`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct DependencyResolveResponse {
        /// Normalized dependency inputs used by resolution.
        pub input_dependencies: Vec<DependencyResolveResolvedDependency>,
        /// Whether resolution succeeded without conflicts.
        pub resolvable: bool,
        /// Selected versions for resolved dependencies.
        pub resolved_versions: Vec<DependencyResolveResolvedVersion>,
        /// Conflicts encountered during resolution.
        pub conflicts: Vec<DependencyResolveConflict>,
        /// Whether feature-unification analysis was enabled.
        pub check_features: bool,
        /// Optional feature-unification summary.
        pub feature_unification_summary: Option<DependencyResolveFeatureSummary>,
        /// Confidence level.
        pub confidence: String,
        /// Confidence details.
        pub confidence_assessment: ConfidenceAssessment,
        /// Suggested next tools.
        pub suggested_next_tools: Vec<String>,
        /// Data origin.
        pub provenance: String,
    }

    /// Normalized dependency input used by resolution.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct DependencyResolveResolvedDependency {
        /// Dependency crate name.
        pub name: String,
        /// Optional semver requirement.
        pub version_req: Option<String>,
        /// Input source label (`explicit`, `cargo_toml`, or `addition`).
        pub source: String,
    }

    /// One resolved dependency version.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct DependencyResolveResolvedVersion {
        /// Dependency crate name.
        pub name: String,
        /// Selected version.
        pub version: String,
        /// Whether the selected version is yanked.
        pub yanked: bool,
    }

    /// Conflict discovered during resolution.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct DependencyResolveConflict {
        /// Dependency crate name.
        pub dependency_name: String,
        /// Requirement that could not be satisfied.
        pub requirement: Option<String>,
        /// Human-readable conflict summary.
        pub message: String,
    }

    /// Optional feature-unification summary for `dependency_resolve`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct DependencyResolveFeatureSummary {
        /// Number of dependency edges inspected.
        pub dependency_edges_evaluated: usize,
        /// Number of unique optional dependencies referenced.
        pub unique_optional_dependencies: usize,
        /// Number of unique feature flags referenced.
        pub unique_feature_flags_referenced: usize,
    }

    /// Request payload for `dependency_feature_impact`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct DependencyFeatureImpactRequest {
        /// Crate name.
        pub crate_name: String,
        /// Version (default: latest).
        pub version: Option<String>,
        /// Features to evaluate.
        pub features: Vec<String>,
        /// Heavy feature threshold.
        pub heavy_threshold: Option<u32>,
    }

    /// Response payload for `dependency_feature_impact`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct DependencyFeatureImpactResponse {
        /// Target crate name.
        pub crate_name: String,
        /// Version selected for analysis.
        pub selected_version: String,
        /// Latest indexed crate version.
        pub latest_version: String,
        /// Effective feature set evaluated.
        pub features: Vec<String>,
        /// Effective heavy-feature threshold.
        pub heavy_threshold: u32,
        /// Baseline dependency count with no extra features.
        pub baseline_dependency_count: usize,
        /// Combined dependency count across evaluated features.
        pub combined_dependency_count: usize,
        /// Per-feature impact details.
        pub per_feature: Vec<DependencyFeatureImpactEntry>,
        /// Features classified as heavy.
        pub heavy_features: Vec<String>,
        /// Freshness check done.
        pub freshness_check_performed: bool,
        /// Freshness result.
        pub freshness_check_result: String,
        /// Refresh enqueued.
        pub refresh_enqueued: bool,
        /// Refresh job ID.
        pub refresh_job_id: Option<String>,
        /// Freshness sources.
        pub freshness: Vec<ResponseFreshnessSource>,
        /// Confidence level.
        pub confidence: String,
        /// Confidence details.
        pub confidence_assessment: ConfidenceAssessment,
        /// Suggested next tools.
        pub suggested_next_tools: Vec<String>,
        /// Data origin.
        pub provenance: String,
    }

    /// Impact details for one feature.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct DependencyFeatureImpactEntry {
        /// Feature name.
        pub feature: String,
        /// Additional dependency count compared to baseline.
        pub additional_dependency_count: usize,
        /// Additional dependency names compared to baseline.
        pub additional_dependencies: Vec<String>,
    }
}

/// Contracts for selected `crate.*` MCP tools.
pub mod krate {
    use super::common::{ConfidenceAssessment, LicensePolicyResult, ResponseFreshnessSource};
    use super::dependency::{
        DependencyResolveConflict, DependencyResolveFeatureSummary,
        DependencyResolveResolvedVersion,
    };
    use super::*;

    /// Sort mode for `crate_search`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema)]
    #[serde(rename_all = "lowercase")]
    pub enum CrateSearchSort {
        /// By relevance.
        Relevance,
        /// By downloads.
        Downloads,
        /// By recency.
        Recent,
    }

    /// Request payload for `crate_search`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateSearchRequest {
        /// Search query.
        pub query: Option<String>,
        /// Category filter.
        pub category: Option<String>,
        /// Keyword filter.
        pub keyword: Option<String>,
        /// Sort mode.
        pub sort: Option<CrateSearchSort>,
        /// Paging cursor.
        pub cursor: Option<String>,
        /// Page (1-based).
        pub page: Option<u32>,
        /// Max results.
        pub limit: Option<u32>,
    }

    /// Response payload for `crate_search`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateSearchResponse {
        /// Effective free-text query.
        pub query: Option<String>,
        /// Effective category filter.
        pub category: Option<String>,
        /// Effective keyword filter.
        pub keyword: Option<String>,
        /// Effective sort mode.
        pub sort: CrateSearchSort,
        /// Current cursor.
        pub cursor: Option<String>,
        /// Next page cursor.
        pub next_cursor: Option<String>,
        /// Page.
        pub page: u32,
        /// Effective hit limit.
        pub limit: u32,
        /// More results available.
        pub has_more: bool,
        /// Truncated by pagination.
        pub truncated: bool,
        /// Returned hit count.
        pub count: usize,
        /// Number of freshness checks performed across top hits.
        pub freshness_checks_performed: usize,
        /// Number of refresh jobs enqueued during freshness probing.
        pub refresh_jobs_enqueued: usize,
        /// Freshness sources.
        pub freshness: Vec<ResponseFreshnessSource>,
        /// Confidence level.
        pub confidence: String,
        /// Confidence details.
        pub confidence_assessment: ConfidenceAssessment,
        /// Suggested next tools.
        pub suggested_next_tools: Vec<String>,
        /// Data origin.
        pub provenance: String,
        /// Ranked crate hits.
        pub hits: Vec<CrateSearchHit>,
    }

    /// One `crate_search` result.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateSearchHit {
        /// Crate name.
        pub name: String,
        /// Crate description.
        pub description: Option<String>,
        /// Repository URL.
        pub repository_url: Option<String>,
        /// Documentation URL.
        pub docs_url: Option<String>,
        /// Homepage URL.
        pub homepage_url: Option<String>,
        /// Category labels.
        pub categories: Vec<String>,
        /// Keyword labels.
        pub keywords: Vec<String>,
        /// Aggregated download count.
        pub total_downloads: i64,
        /// Latest publication timestamp.
        pub latest_published_at: Option<String>,
        /// Number of dependent crates.
        pub dependent_crates: i64,
        /// Numeric rank score.
        pub rank_score: f64,
        /// Match reasons used for ranking/explanation.
        pub match_reasons: Vec<String>,
    }

    /// Request payload for `crate_intel`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateIntelRequest {
        /// Crate name.
        pub crate_name: String,
        /// Version (default: latest).
        pub version: Option<String>,
        /// Max version history entries.
        pub versions_limit: Option<u32>,
        /// Max dependents entries.
        pub dependents_limit: Option<u32>,
        /// Max readme characters.
        pub readme_max_chars: Option<u32>,
    }

    /// Response payload for `crate_intel`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateIntelResponse {
        /// Target crate name.
        pub crate_name: String,
        /// Selected version for the response.
        pub selected_version: String,
        /// rust-version for selected version.
        pub selected_rust_version: Option<String>,
        /// Publication timestamp for selected version.
        pub selected_version_published_at: Option<String>,
        /// Latest indexed version.
        pub latest_version: String,
        /// rust-version for latest indexed version.
        pub latest_rust_version: Option<String>,
        /// Aggregated download count.
        pub total_downloads: i64,
        /// Last update timestamp.
        pub last_updated_at: Option<String>,
        /// Crate description.
        pub description: Option<String>,
        /// Repository URL.
        pub repository_url: Option<String>,
        /// Documentation URL.
        pub docs_url: Option<String>,
        /// Homepage URL.
        pub homepage_url: Option<String>,
        /// Category labels.
        pub categories: Vec<String>,
        /// Keyword labels.
        pub keywords: Vec<String>,
        /// Optional readme content excerpt.
        pub readme: Option<String>,
        /// Whether readme content was truncated.
        pub readme_truncated: bool,
        /// Version history entries.
        pub version_history: Vec<CrateIntelVersion>,
        /// Dependency entries for selected version.
        pub dependencies: Vec<CrateIntelDependency>,
        /// Dependent crate entries.
        pub dependents: Vec<CrateIntelDependent>,
        /// Total dependent crate count.
        pub dependent_crate_count: i64,
        /// GitHub repository health metadata (if available).
        #[serde(skip_serializing_if = "Option::is_none")]
        pub github: Option<CrateIntelGitHub>,
        /// Advisory matches for selected version.
        pub advisories: Vec<CrateIntelAdvisory>,
        /// Freshness check done.
        pub freshness_check_performed: bool,
        /// Freshness result.
        pub freshness_check_result: String,
        /// Refresh enqueued.
        pub refresh_enqueued: bool,
        /// Refresh job ID.
        pub refresh_job_id: Option<String>,
        /// Freshness sources.
        pub freshness: Vec<ResponseFreshnessSource>,
        /// Confidence level.
        pub confidence: String,
        /// Confidence details.
        pub confidence_assessment: ConfidenceAssessment,
        /// Suggested next tools.
        pub suggested_next_tools: Vec<String>,
        /// Data origin.
        pub provenance: String,
    }

    /// Version entry in `crate_intel`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateIntelVersion {
        /// Version string.
        pub version: String,
        /// rust-version for this version.
        pub rust_version: Option<String>,
        /// Publication timestamp.
        pub published_at: Option<String>,
        /// Whether version is yanked.
        pub yanked: bool,
        /// Download count for this version.
        pub downloads: i64,
        /// Whether advisories were matched.
        pub has_advisory: bool,
    }

    /// Dependency entry in `crate_intel`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateIntelDependency {
        /// Dependency crate name.
        pub crate_name: String,
        /// Dependency semver requirement.
        pub requirement: String,
        /// Dependency kind (`normal`, `dev`, `build`, etc).
        pub dependency_kind: String,
        /// Whether dependency is optional.
        pub optional: bool,
        /// Enabled feature list.
        pub features: Vec<String>,
    }

    /// Dependent crate entry in `crate_intel`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateIntelDependent {
        /// Dependent crate name.
        pub crate_name: String,
        /// Latest indexed version for the dependent crate.
        pub latest_version: Option<String>,
        /// Aggregated download count for the dependent crate.
        pub total_downloads: i64,
    }

    /// Advisory entry in `crate_intel`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateIntelAdvisory {
        /// Advisory identifier.
        pub advisory_id: String,
        /// Advisory title.
        pub title: String,
        /// Advisory severity label.
        pub severity: Option<String>,
        /// Advisory URL.
        pub url: Option<String>,
        /// Affected version range expression.
        pub affected_range: String,
        /// Fixed version list.
        pub fixed_versions: Vec<String>,
        /// Advisory source label.
        pub source: String,
        /// Full vulnerability description (markdown). Use for detailed
        /// analysis; may be long.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub details: Option<String>,
        /// Affected function paths from the advisory. When non-empty, the
        /// client can check whether its codebase calls any of these
        /// functions to determine reachability.
        #[serde(skip_serializing_if = "Vec::is_empty", default)]
        pub affected_functions: Vec<String>,
        /// CWE identifiers associated with the advisory (e.g.
        /// `["CWE-787"]`).
        #[serde(skip_serializing_if = "Vec::is_empty", default)]
        pub cwe_ids: Vec<String>,
    }

    /// GitHub repository metadata surfaced in `crate_intel`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateIntelGitHub {
        /// GitHub owner (user or org).
        pub owner: String,
        /// GitHub repository name.
        pub repo: String,
        /// Star count.
        pub stars: u64,
        /// Fork count.
        pub forks: u64,
        /// Open issue + PR count.
        pub open_issues: u64,
        /// Whether the repository is archived.
        pub archived: bool,
        /// Last push timestamp (ISO 8601).
        #[serde(skip_serializing_if = "Option::is_none")]
        pub last_push: Option<String>,
        /// SPDX license identifier from the GitHub repo.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub license: Option<String>,
        /// Total contributor count (including anonymous).
        #[serde(skip_serializing_if = "Option::is_none")]
        pub contributors: Option<u64>,
        /// ISO 8601 timestamp of the most recent commit on the default branch.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub last_commit_at: Option<String>,
        /// Subject line of the most recent commit.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub last_commit_message: Option<String>,
        /// Number of commits in the last 90 days (from git history).
        #[serde(skip_serializing_if = "Option::is_none")]
        pub recent_commit_count: Option<u64>,
    }

    /// A GitHub release note entry surfaced in API diff and migration tools.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct GitHubReleaseNote {
        /// Git tag name (e.g. `v1.2.0`).
        pub tag: String,
        /// Release title.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub name: Option<String>,
        /// Release body (markdown).
        #[serde(skip_serializing_if = "Option::is_none")]
        pub body: Option<String>,
        /// Publication timestamp (ISO 8601).
        #[serde(skip_serializing_if = "Option::is_none")]
        pub published_at: Option<String>,
    }

    /// Request payload for `crate_versions`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateVersionsRequest {
        /// Crate name.
        pub crate_name: String,
        /// Paging cursor.
        pub cursor: Option<String>,
        /// Page (1-based).
        pub page: Option<u32>,
        /// Max results.
        pub limit: Option<u32>,
    }

    /// Response payload for `crate_versions`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateVersionsResponse {
        /// Target crate name.
        pub crate_name: String,
        /// Current cursor.
        pub cursor: Option<String>,
        /// Next page cursor.
        pub next_cursor: Option<String>,
        /// Page.
        pub page: u32,
        /// Effective page size.
        pub limit: u32,
        /// More results available.
        pub has_more: bool,
        /// Truncated by pagination.
        pub truncated: bool,
        /// rust-version from the latest indexed version.
        pub latest_rust_version: Option<String>,
        /// Number of versions returned.
        pub count: usize,
        /// Version timeline entries.
        pub versions: Vec<CrateVersionTimelineItem>,
        /// Freshness check done.
        pub freshness_check_performed: bool,
        /// Freshness result.
        pub freshness_check_result: String,
        /// Refresh enqueued.
        pub refresh_enqueued: bool,
        /// Refresh job ID.
        pub refresh_job_id: Option<String>,
        /// Freshness sources.
        pub freshness: Vec<ResponseFreshnessSource>,
        /// Confidence level.
        pub confidence: String,
        /// Confidence details.
        pub confidence_assessment: ConfidenceAssessment,
        /// Suggested next tools.
        pub suggested_next_tools: Vec<String>,
        /// Data origin.
        pub provenance: String,
    }

    /// One entry in the crate version timeline.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateVersionTimelineItem {
        /// Version string.
        pub version: String,
        /// rust-version declared for this version.
        pub rust_version: Option<String>,
        /// Publication timestamp.
        pub published_at: Option<String>,
        /// Whether version is yanked.
        pub yanked: bool,
        /// Download count for this version.
        pub downloads: i64,
        /// Number of advisory matches.
        pub advisory_count: i64,
        /// Age in days since publication.
        pub release_age_days: Option<i64>,
        /// Whether this entry is the latest indexed version.
        pub is_latest: bool,
        /// Adoption signal derived from download/yank state.
        pub adoption_signal: String,
        /// Marker labels for noteworthy conditions.
        pub markers: Vec<String>,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateImplMethod {
        pub name: String,
        pub signature: Option<String>,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateTraitAssociatedType {
        pub name: String,
        pub bounds: Vec<String>,
        pub default: Option<String>,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateTraitDefinition {
        pub trait_name: String,
        pub is_auto: bool,
        pub is_unsafe: bool,
        pub is_dyn_compatible: bool,
        pub supertraits: Vec<String>,
        pub required_methods: Vec<CrateImplMethod>,
        pub provided_methods: Vec<CrateImplMethod>,
        pub associated_types: Vec<CrateTraitAssociatedType>,
        pub generic_params: Vec<String>,
        pub index_source: String,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateApiRequest {
        pub crate_name: String,
        pub version: Option<String>,
        /// Glob filter for path or symbol name (e.g. `*connection*`).
        pub path_glob: Option<String>,
        pub kinds: Option<Vec<String>>,
        /// Paging cursor.
        pub cursor: Option<String>,
        /// Page (1-based).
        pub page: Option<u32>,
        pub limit: Option<u32>,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateApiResponse {
        pub crate_name: String,
        pub selected_version: String,
        pub latest_version: String,
        pub path_glob: Option<String>,
        pub kinds: Vec<String>,
        /// Current cursor.
        pub cursor: Option<String>,
        /// Next page cursor.
        pub next_cursor: Option<String>,
        /// Page.
        pub page: u32,
        pub limit: u32,
        /// More results available.
        pub has_more: bool,
        /// Truncated by pagination.
        pub truncated: bool,
        pub count: usize,
        pub symbols: Vec<CrateApiSymbol>,
        pub freshness_check_performed: bool,
        pub freshness_check_result: String,
        pub refresh_enqueued: bool,
        pub refresh_job_id: Option<String>,
        pub freshness: Vec<ResponseFreshnessSource>,
        pub confidence: String,
        pub confidence_assessment: ConfidenceAssessment,
        pub suggested_next_tools: Vec<String>,
        pub provenance: String,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateApiSymbol {
        pub name: String,
        pub kind: String,
        pub signature: Option<String>,
        pub visibility: Option<String>,
        pub source_path: String,
        pub start_line: i32,
        pub end_line: i32,
        pub index_source: String,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateApiDiffRequest {
        pub crate_name: String,
        pub from_version: String,
        pub to_version: String,
        pub limit: Option<u32>,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateApiDiffResponse {
        pub crate_name: String,
        pub from_version: String,
        pub to_version: String,
        pub added_count: usize,
        pub removed_count: usize,
        pub changed_count: usize,
        pub breaking_changes_detected: bool,
        pub changes: Vec<CrateApiDiffChange>,
        pub truncated: bool,
        /// GitHub release notes for versions in the diff range (newest first).
        #[serde(skip_serializing_if = "Vec::is_empty", default)]
        pub release_notes: Vec<GitHubReleaseNote>,
        pub freshness_check_performed: bool,
        pub freshness_check_result: String,
        pub refresh_enqueued: bool,
        pub refresh_job_id: Option<String>,
        pub freshness: Vec<ResponseFreshnessSource>,
        pub confidence: String,
        pub confidence_assessment: ConfidenceAssessment,
        pub suggested_next_tools: Vec<String>,
        pub provenance: String,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateApiDiffChange {
        pub name: String,
        pub kind: String,
        pub change_type: CrateApiDiffChangeType,
        pub from_signature: Option<String>,
        pub to_signature: Option<String>,
        pub from_visibility: Option<String>,
        pub to_visibility: Option<String>,
        pub breaking_change: bool,
    }

    #[derive(Debug, Clone, Copy, Deserialize, Serialize, schemars::JsonSchema, PartialEq, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum CrateApiDiffChangeType {
        Added,
        Removed,
        SignatureChanged,
        VisibilityChanged,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateFeaturesRequest {
        pub crate_name: String,
        pub version: Option<String>,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateFeaturesResponse {
        pub crate_name: String,
        pub selected_version: String,
        pub latest_version: String,
        pub default_features: Vec<String>,
        pub feature_count: usize,
        pub features: Vec<CrateFeatureFlag>,
        pub freshness_check_performed: bool,
        pub freshness_check_result: String,
        pub refresh_enqueued: bool,
        pub refresh_job_id: Option<String>,
        pub freshness: Vec<ResponseFreshnessSource>,
        pub confidence: String,
        pub confidence_assessment: ConfidenceAssessment,
        pub suggested_next_tools: Vec<String>,
        pub provenance: String,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateFeatureFlag {
        pub name: String,
        pub is_default: bool,
        pub enables_features: Vec<String>,
        pub enables_dependencies: Vec<String>,
        pub transitive_enables: Vec<String>,
    }

    #[derive(Debug, Clone, Copy, Deserialize, Serialize, schemars::JsonSchema)]
    #[serde(rename_all = "snake_case")]
    pub enum CrateGraphDirection {
        Dependencies,
        Dependents,
        Both,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateGraphRequest {
        pub crate_name: String,
        pub version: Option<String>,
        pub direction: Option<CrateGraphDirection>,
        pub depth: Option<u32>,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateGraphResponse {
        pub crate_name: String,
        pub selected_version: String,
        pub direction: CrateGraphDirection,
        pub depth: u32,
        pub node_count: usize,
        pub edge_count: usize,
        pub nodes: Vec<CrateGraphNode>,
        pub edges: Vec<CrateGraphEdge>,
        pub cycle_safe_traversal_notes: Vec<String>,
        pub freshness_check_performed: bool,
        pub freshness_check_result: String,
        pub refresh_enqueued: bool,
        pub refresh_job_id: Option<String>,
        pub freshness: Vec<ResponseFreshnessSource>,
        pub confidence: String,
        pub confidence_assessment: ConfidenceAssessment,
        pub suggested_next_tools: Vec<String>,
        pub provenance: String,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateGraphNode {
        pub crate_name: String,
        pub latest_version: Option<String>,
        pub min_distance: u32,
        pub role: String,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateGraphEdge {
        pub from_crate: String,
        pub from_version: Option<String>,
        pub to_crate: String,
        pub to_version: Option<String>,
        pub requirement: String,
        pub dependency_kind: String,
        pub optional: bool,
        pub depth: u32,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateHotspotsRequest {
        pub crate_name: String,
        pub version: Option<String>,
        pub path_glob: Option<String>,
        pub include_unsafe: Option<bool>,
        pub include_concurrency: Option<bool>,
        /// Paging cursor.
        pub cursor: Option<String>,
        /// Page (1-based).
        pub page: Option<u32>,
        pub limit: Option<u32>,
    }

    #[derive(Debug, Clone, Copy, Deserialize, Serialize, schemars::JsonSchema, PartialEq, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum HotspotKind {
        Unsafe,
        Concurrency,
    }

    #[derive(Debug, Clone, Copy, Deserialize, Serialize, schemars::JsonSchema, PartialEq, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum HotspotSeverity {
        Low,
        Medium,
        High,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateHotspotsResponse {
        pub crate_name: String,
        pub selected_version: String,
        pub latest_version: String,
        pub path_glob: Option<String>,
        pub include_unsafe: bool,
        pub include_concurrency: bool,
        /// Current cursor.
        pub cursor: Option<String>,
        /// Next page cursor.
        pub next_cursor: Option<String>,
        /// Page.
        pub page: u32,
        pub limit: u32,
        /// More results available.
        pub has_more: bool,
        /// Truncated by pagination.
        pub truncated: bool,
        pub scanned_files: usize,
        pub count: usize,
        pub hotspots: Vec<CrateHotspotHit>,
        pub freshness_check_performed: bool,
        pub freshness_check_result: String,
        pub refresh_enqueued: bool,
        pub refresh_job_id: Option<String>,
        pub freshness: Vec<ResponseFreshnessSource>,
        pub confidence: String,
        pub confidence_assessment: ConfidenceAssessment,
        pub suggested_next_tools: Vec<String>,
        pub provenance: String,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateHotspotHit {
        pub path: String,
        pub line: u32,
        pub kind: HotspotKind,
        pub pattern: String,
        pub severity: HotspotSeverity,
        pub snippet: String,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateLicenseCheckRequest {
        pub crate_name: String,
        pub version: Option<String>,
        pub allow_licenses: Option<Vec<String>>,
        pub deny_licenses: Option<Vec<String>>,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateLicenseCheckResponse {
        pub crate_name: String,
        pub selected_version: String,
        pub latest_version: String,
        pub license_expression: Option<String>,
        pub matched_licenses: Vec<String>,
        pub allow_licenses: Vec<String>,
        pub deny_licenses: Vec<String>,
        pub policy_result: LicensePolicyResult,
        pub policy_reasons: Vec<String>,
        pub freshness_check_performed: bool,
        pub freshness_check_result: String,
        pub refresh_enqueued: bool,
        pub refresh_job_id: Option<String>,
        pub freshness: Vec<ResponseFreshnessSource>,
        pub confidence: String,
        pub confidence_assessment: ConfidenceAssessment,
        pub suggested_next_tools: Vec<String>,
        pub provenance: String,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateUsagePatternsRequest {
        pub crate_name: String,
        pub symbol_name: String,
        pub version: Option<String>,
        /// Paging cursor.
        pub cursor: Option<String>,
        /// Page (1-based).
        pub page: Option<u32>,
        pub limit: Option<u32>,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateUsagePatternsResponse {
        pub crate_name: String,
        pub selected_version: String,
        pub latest_version: String,
        pub symbol_name: String,
        /// Current cursor.
        pub cursor: Option<String>,
        /// Next page cursor.
        pub next_cursor: Option<String>,
        /// Page.
        pub page: u32,
        pub limit: u32,
        /// More results available.
        pub has_more: bool,
        /// Truncated by pagination.
        pub truncated: bool,
        pub count: usize,
        /// Number of dependent crate versions whose source directories were
        /// scanned. Helps distinguish "no source available" from "symbol not
        /// found in dependents".
        pub scanned_dependents: usize,
        pub patterns: Vec<CrateUsagePattern>,
        pub freshness_check_performed: bool,
        pub freshness_check_result: String,
        pub refresh_enqueued: bool,
        pub refresh_job_id: Option<String>,
        pub freshness: Vec<ResponseFreshnessSource>,
        pub confidence: String,
        pub confidence_assessment: ConfidenceAssessment,
        pub suggested_next_tools: Vec<String>,
        pub provenance: String,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateUsagePattern {
        pub dependent_crate: String,
        pub dependent_version: String,
        pub dependent_downloads: i64,
        pub path: String,
        pub line_start: u32,
        pub line_end: u32,
        pub snippet: String,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateReExportsRequest {
        pub crate_name: String,
        pub version: Option<String>,
        pub path_prefix: Option<String>,
        /// Paging cursor.
        pub cursor: Option<String>,
        /// Page (1-based).
        pub page: Option<u32>,
        pub limit: Option<u32>,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateReExportsResponse {
        pub crate_name: String,
        pub selected_version: String,
        pub latest_version: String,
        pub path_prefix: Option<String>,
        /// Current cursor.
        pub cursor: Option<String>,
        /// Next page cursor.
        pub next_cursor: Option<String>,
        /// Page.
        pub page: u32,
        pub limit: u32,
        /// More results available.
        pub has_more: bool,
        /// Truncated by pagination.
        pub truncated: bool,
        pub count: usize,
        pub re_exports: Vec<CrateReExportEntry>,
        pub freshness_check_performed: bool,
        pub freshness_check_result: String,
        pub refresh_enqueued: bool,
        pub refresh_job_id: Option<String>,
        pub freshness: Vec<ResponseFreshnessSource>,
        pub confidence: String,
        pub confidence_assessment: ConfidenceAssessment,
        pub suggested_next_tools: Vec<String>,
        pub provenance: String,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateReExportEntry {
        pub canonical_path: String,
        pub original_definition_path: String,
        pub kind: String,
        pub visibility: String,
        pub shortest_public_path: bool,
        pub source_path: String,
        pub source_line: u32,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateImportPathRequest {
        pub crate_name: String,
        pub symbol_name: String,
        pub version: Option<String>,
        pub kind: Option<String>,
        /// Paging cursor.
        pub cursor: Option<String>,
        /// Page (1-based).
        pub page: Option<u32>,
        pub limit: Option<u32>,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateImportPathResponse {
        pub crate_name: String,
        pub selected_version: String,
        pub latest_version: String,
        pub symbol_name: String,
        pub kind: Option<String>,
        /// Current cursor.
        pub cursor: Option<String>,
        /// Next page cursor.
        pub next_cursor: Option<String>,
        /// Page.
        pub page: u32,
        pub limit: u32,
        /// More results available.
        pub has_more: bool,
        /// Truncated by pagination.
        pub truncated: bool,
        pub count: usize,
        pub best_import_path: Option<String>,
        pub matches: Vec<CrateImportPathMatch>,
        pub freshness_check_performed: bool,
        pub freshness_check_result: String,
        pub refresh_enqueued: bool,
        pub refresh_job_id: Option<String>,
        pub freshness: Vec<ResponseFreshnessSource>,
        pub confidence: String,
        pub confidence_assessment: ConfidenceAssessment,
        pub suggested_next_tools: Vec<String>,
        pub provenance: String,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateImportPathMatch {
        pub symbol_name: String,
        pub kind: String,
        pub import_path: String,
        pub definition_path: Option<String>,
        pub source_path: String,
        pub start_line: u32,
        pub end_line: u32,
        pub index_source: String,
        pub exact_name_match: bool,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateMigrationPathRequest {
        pub crate_name: String,
        pub from_version: String,
        pub to_version: String,
        pub limit: Option<u32>,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateMigrationPathResponse {
        pub crate_name: String,
        pub from_version: String,
        pub to_version: String,
        pub breaking_changes_detected: bool,
        pub added_count: usize,
        pub removed_count: usize,
        pub changed_count: usize,
        pub migration_actions: Vec<CrateMigrationAction>,
        /// GitHub release notes for versions in the migration range (newest
        /// first).
        #[serde(skip_serializing_if = "Vec::is_empty", default)]
        pub release_notes: Vec<GitHubReleaseNote>,
        pub confidence: String,
        pub confidence_assessment: ConfidenceAssessment,
        pub suggested_next_tools: Vec<String>,
        pub provenance: String,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateMigrationAction {
        pub action: String,
        pub rationale: String,
        pub affected_symbol: String,
        pub kind: String,
        pub from_signature: Option<String>,
        pub to_signature: Option<String>,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateCompareRequest {
        pub left_crate: String,
        pub right_crate: String,
        pub left_version: Option<String>,
        pub right_version: Option<String>,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateCompareResponse {
        pub left: CrateCompareSide,
        pub right: CrateCompareSide,
        pub recommendation: Option<String>,
        pub recommendation_reasons: Vec<String>,
        pub freshness: Vec<ResponseFreshnessSource>,
        pub freshness_check_performed: bool,
        pub freshness_check_result: String,
        pub refresh_enqueued: bool,
        pub refresh_job_id: Option<String>,
        pub confidence: String,
        pub confidence_assessment: ConfidenceAssessment,
        pub suggested_next_tools: Vec<String>,
        pub provenance: String,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateCompareSide {
        pub crate_name: String,
        pub selected_version: String,
        pub latest_version: String,
        pub selected_rust_version: Option<String>,
        pub selected_published_at: Option<String>,
        pub license_expression: Option<String>,
        pub total_downloads: i64,
        pub dependent_crate_count: i64,
        pub dependency_count: i64,
        pub feature_count: i64,
        pub advisory_count: i64,
        pub yanked: bool,
        pub maintenance_score: f64,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateCompatibilityRequest {
        pub left_crate: String,
        pub left_version: Option<String>,
        pub right_crate: String,
        pub right_version: Option<String>,
        pub check_features: Option<bool>,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateCompatibilityMatrixRequest {
        pub left_crate: String,
        pub right_crate: String,
        pub left_versions: Option<Vec<String>>,
        pub right_versions: Option<Vec<String>>,
        pub check_features: Option<bool>,
        pub version_limit: Option<u32>,
        pub max_pairs: Option<u32>,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateCompatibilityResponse {
        pub left_crate: String,
        pub left_version: Option<String>,
        pub right_crate: String,
        pub right_version: Option<String>,
        pub resolvable: bool,
        pub resolved_versions: Vec<DependencyResolveResolvedVersion>,
        pub conflicts: Vec<DependencyResolveConflict>,
        pub check_features: bool,
        pub feature_unification_summary: Option<DependencyResolveFeatureSummary>,
        pub confidence: String,
        pub confidence_assessment: ConfidenceAssessment,
        pub suggested_next_tools: Vec<String>,
        pub provenance: String,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateCompatibilityMatrixResponse {
        pub left_crate: String,
        pub right_crate: String,
        pub check_features: bool,
        pub pairs_tested: usize,
        pub compatible_pairs: Vec<CrateCompatibilityMatrixEntry>,
        pub incompatible_pairs: Vec<CrateCompatibilityMatrixEntry>,
        pub confidence: String,
        pub confidence_assessment: ConfidenceAssessment,
        pub suggested_next_tools: Vec<String>,
        pub provenance: String,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateCompatibilityMatrixEntry {
        pub left_version: String,
        pub right_version: String,
        pub resolvable: bool,
        pub conflict_messages: Vec<String>,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateDeriveMacrosRequest {
        pub crate_name: String,
        pub version: Option<String>,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateDeriveMacrosResponse {
        pub crate_name: String,
        pub selected_version: String,
        pub latest_version: String,
        pub derive_macros: Vec<CrateDeriveMacroEntry>,
        pub attribute_macros: Vec<CrateAttributeMacroEntry>,
        pub function_like_macros: Vec<CrateFunctionLikeMacroEntry>,
        pub freshness_check_performed: bool,
        pub freshness_check_result: String,
        pub refresh_enqueued: bool,
        pub refresh_job_id: Option<String>,
        pub freshness: Vec<ResponseFreshnessSource>,
        pub confidence: String,
        pub confidence_assessment: ConfidenceAssessment,
        pub suggested_next_tools: Vec<String>,
        pub provenance: String,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateDeriveMacroEntry {
        pub name: String,
        pub accepted_attributes: Vec<String>,
        pub usage_pattern: String,
        pub source_path: String,
        pub source_line: i32,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateAttributeMacroEntry {
        pub name: String,
        pub usage_pattern: String,
        pub signature_pattern: Option<String>,
        pub source_path: String,
        pub source_line: i32,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateFunctionLikeMacroEntry {
        pub name: String,
        pub usage_pattern: String,
        pub signature_pattern: Option<String>,
        pub source_path: String,
        pub source_line: i32,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateErrorTypesRequest {
        pub crate_name: String,
        pub version: Option<String>,
        pub type_name: Option<String>,
        /// Paging cursor.
        pub cursor: Option<String>,
        /// Page (1-based).
        pub page: Option<u32>,
        pub limit: Option<u32>,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateErrorTypesResponse {
        pub crate_name: String,
        pub selected_version: String,
        pub latest_version: String,
        pub type_name: Option<String>,
        /// Current cursor.
        pub cursor: Option<String>,
        /// Next page cursor.
        pub next_cursor: Option<String>,
        /// Page.
        pub page: u32,
        pub limit: u32,
        /// More results available.
        pub has_more: bool,
        /// Truncated by pagination.
        pub truncated: bool,
        pub count: usize,
        pub error_types: Vec<CrateErrorTypeEntry>,
        pub freshness_check_performed: bool,
        pub freshness_check_result: String,
        pub refresh_enqueued: bool,
        pub refresh_job_id: Option<String>,
        pub freshness: Vec<ResponseFreshnessSource>,
        pub confidence: String,
        pub confidence_assessment: ConfidenceAssessment,
        pub suggested_next_tools: Vec<String>,
        pub provenance: String,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateErrorTypeEntry {
        pub type_name: String,
        pub kind: String,
        pub variants: Vec<String>,
        pub fields: Vec<String>,
        pub display_patterns: Vec<String>,
        pub from_conversions: Vec<String>,
        pub returned_by: Vec<String>,
        pub source_path: String,
        pub source_line: i32,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateAlternativesRequest {
        pub crate_name: String,
        pub version: Option<String>,
        /// Paging cursor.
        pub cursor: Option<String>,
        /// Page (1-based).
        pub page: Option<u32>,
        pub limit: Option<u32>,
        pub allow_licenses: Option<Vec<String>>,
        pub deny_licenses: Option<Vec<String>>,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateAlternativesResponse {
        pub crate_name: String,
        pub selected_version: String,
        pub latest_version: String,
        /// Current cursor.
        pub cursor: Option<String>,
        /// Next page cursor.
        pub next_cursor: Option<String>,
        /// Page.
        pub page: u32,
        pub limit: u32,
        /// More results available.
        pub has_more: bool,
        /// Truncated by pagination.
        pub truncated: bool,
        pub count: usize,
        pub allow_licenses: Vec<String>,
        pub deny_licenses: Vec<String>,
        pub alternatives: Vec<CrateAlternativeHit>,
        pub freshness_check_performed: bool,
        pub freshness_check_result: String,
        pub refresh_enqueued: bool,
        pub refresh_job_id: Option<String>,
        pub freshness: Vec<ResponseFreshnessSource>,
        pub confidence: String,
        pub confidence_assessment: ConfidenceAssessment,
        pub suggested_next_tools: Vec<String>,
        pub provenance: String,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateAlternativeHit {
        pub crate_name: String,
        pub latest_version: Option<String>,
        pub description: Option<String>,
        pub categories: Vec<String>,
        pub keywords: Vec<String>,
        pub total_downloads: i64,
        pub dependent_crates: i64,
        pub advisory_count: i64,
        pub yanked: bool,
        pub license_expression: Option<String>,
        pub policy_result: LicensePolicyResult,
        pub policy_reasons: Vec<String>,
        pub score: f64,
        pub rank_reasons: Vec<String>,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateTraitImplsRequest {
        pub crate_name: String,
        pub version: Option<String>,
        pub trait_name: Option<String>,
        pub type_name: Option<String>,
        /// Paging cursor.
        pub cursor: Option<String>,
        /// Page (1-based).
        pub page: Option<u32>,
        pub limit: Option<u32>,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateTraitImplsResponse {
        pub crate_name: String,
        pub selected_version: String,
        pub latest_version: String,
        pub trait_name: Option<String>,
        pub type_name: Option<String>,
        /// Current cursor.
        pub cursor: Option<String>,
        /// Next page cursor.
        pub next_cursor: Option<String>,
        /// Page.
        pub page: u32,
        pub limit: u32,
        /// More results available.
        pub has_more: bool,
        /// Truncated by pagination.
        pub truncated: bool,
        pub count: usize,
        pub impls: Vec<CrateTraitImplRelation>,
        pub trait_definitions: Vec<CrateTraitDefinition>,
        pub freshness_check_performed: bool,
        pub freshness_check_result: String,
        pub refresh_enqueued: bool,
        pub refresh_job_id: Option<String>,
        pub freshness: Vec<ResponseFreshnessSource>,
        pub confidence: String,
        pub confidence_assessment: ConfidenceAssessment,
        pub suggested_next_tools: Vec<String>,
        pub provenance: String,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateTraitImplRelation {
        pub type_name: String,
        pub type_name_display: Option<String>,
        pub trait_name: Option<String>,
        pub trait_name_display: Option<String>,
        pub impl_kind: String,
        pub blanket_impl: bool,
        pub synthetic_impl: bool,
        pub negative_impl: bool,
        pub blanket_type: Option<String>,
        pub generic_params: Vec<String>,
        pub where_clauses: Vec<String>,
        pub methods: Vec<CrateImplMethod>,
        pub source_path: String,
        pub start_line: i32,
        pub end_line: i32,
        pub index_source: String,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateTypeInfoRequest {
        pub crate_name: String,
        pub type_name: String,
        pub version: Option<String>,
        pub include_methods: Option<bool>,
        pub include_trait_impls: Option<bool>,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateTypeInfoResponse {
        pub crate_name: String,
        pub selected_version: String,
        pub latest_version: String,
        pub type_name: String,
        pub include_methods: bool,
        pub include_trait_impls: bool,
        pub type_definition: Option<CrateTypeDefinition>,
        pub inherent_methods: Vec<CrateImplMethod>,
        pub trait_impls: Vec<CrateTraitImpl>,
        pub trait_definitions: Vec<CrateTraitDefinition>,
        pub conversions: Vec<CrateTypeConversion>,
        pub freshness_check_performed: bool,
        pub freshness_check_result: String,
        pub refresh_enqueued: bool,
        pub refresh_job_id: Option<String>,
        pub freshness: Vec<ResponseFreshnessSource>,
        pub confidence: String,
        pub confidence_assessment: ConfidenceAssessment,
        pub suggested_next_tools: Vec<String>,
        pub provenance: String,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateTypeDefinition {
        pub type_name: String,
        pub kind: String,
        pub visibility: Option<String>,
        pub canonical_path: Option<String>,
        pub definition_path: Option<String>,
        pub generic_params: Vec<String>,
        pub where_clauses: Vec<String>,
        pub fields: Vec<CrateTypeField>,
        pub variants: Vec<CrateTypeVariant>,
        pub deprecated_since: Option<String>,
        pub deprecated_note: Option<String>,
        pub is_non_exhaustive: bool,
        pub auto_traits: Vec<String>,
        pub source_path: String,
        pub start_line: i32,
        pub end_line: i32,
        pub index_source: String,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateTypeField {
        pub name: Option<String>,
        pub field_type: String,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateTypeVariant {
        pub name: String,
        pub fields: Vec<CrateTypeField>,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateTraitImpl {
        pub trait_name: Option<String>,
        pub trait_name_display: Option<String>,
        pub impl_kind: String,
        pub blanket_impl: bool,
        pub synthetic_impl: bool,
        pub negative_impl: bool,
        pub blanket_type: Option<String>,
        pub generic_params: Vec<String>,
        pub where_clauses: Vec<String>,
        pub methods: Vec<CrateImplMethod>,
        pub source_path: String,
        pub start_line: i32,
        pub end_line: i32,
        pub index_source: String,
    }

    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateTypeConversion {
        pub trait_name: String,
        pub source_type: Option<String>,
        pub target_type: Option<String>,
    }

    /// Request payload for `crate_deprecated`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateDeprecatedRequest {
        /// Crate name.
        pub crate_name: String,
        /// Version (default: latest).
        pub version: Option<String>,
        /// Max results (default: 50).
        pub limit: Option<u32>,
        /// Page (1-based).
        pub page: Option<u32>,
    }

    /// Response payload for `crate_deprecated`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateDeprecatedResponse {
        /// Resolved crate name.
        pub crate_name: String,
        /// Version the deprecation data was read from.
        pub selected_version: String,
        /// Latest indexed version of the crate.
        pub latest_version: String,
        /// Number of deprecated items returned.
        pub count: usize,
        /// Whether additional results are available on the next page.
        pub has_more: bool,
        /// Truncated by pagination.
        pub truncated: bool,
        /// Deprecated items found in this version.
        pub deprecated_items: Vec<DeprecatedItem>,
        /// Freshness check done.
        pub freshness_check_performed: bool,
        /// Freshness result.
        pub freshness_check_result: String,
        /// Refresh enqueued.
        pub refresh_enqueued: bool,
        /// Refresh job ID.
        pub refresh_job_id: Option<String>,
        /// Freshness sources.
        pub freshness: Vec<ResponseFreshnessSource>,
        /// Confidence level.
        pub confidence: String,
        /// Confidence details.
        pub confidence_assessment: ConfidenceAssessment,
        /// Suggested next tools.
        pub suggested_next_tools: Vec<String>,
        /// Data origin.
        pub provenance: String,
    }

    /// One deprecated API item returned by `crate_deprecated`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct DeprecatedItem {
        /// Symbol name.
        pub name: String,
        /// Kind (e.g. `fn`, `struct`, `enum`).
        pub kind: String,
        /// Deprecated since version.
        pub deprecated_since: Option<String>,
        /// Deprecation note / replacement.
        pub deprecated_note: Option<String>,
        /// Canonical import path.
        pub canonical_path: Option<String>,
        /// Index source.
        pub index_source: String,
    }
}
