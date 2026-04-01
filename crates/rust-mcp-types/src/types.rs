use rmcp::schemars;
use serde::{Deserialize, Serialize};

/// Lenient deserializer that accepts both JSON numbers and strings for
/// `Option<u32>` fields.  Some MCP clients stringify numeric tool arguments;
/// this avoids hard deserialization failures in those cases.
mod lenient_u32 {
    use serde::{Deserialize, Deserializer, de};

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u32>, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Val {
            Num(u32),
            Str(String),
        }

        Ok(match Option::<Val>::deserialize(d)? {
            None => None,
            Some(Val::Num(n)) => Some(n),
            Some(Val::Str(s)) if s.is_empty() => None,
            Some(Val::Str(s)) => Some(
                s.parse::<u32>()
                    .map_err(de::Error::custom)?,
            ),
        })
    }
}

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
        /// Crates to index, e.g. `[{"name": "serde"}, {"name": "tokio",
        /// "version": "1.40.0"}]`.
        pub crates: Vec<IndexCrateEntry>,
        /// Also fetch and index dependency metadata (default: false).
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
        /// Refresh scope: `"crate"`, `"all"`, `"security"`, `"docs"`,
        /// `"local_cache"`, or `"rustdoc_json"` (default: `"all"`).
        pub scope: Option<IndexRefreshScope>,
        /// Filter refresh to a specific crate, e.g. `"serde"`.
        pub crate_name: Option<String>,
        /// Search query to select candidate crates for refresh.
        pub query: Option<String>,
        /// Page number (1-based, default: 1).
        #[serde(default, deserialize_with = "super::lenient_u32::deserialize")]
        pub page: Option<u32>,
        /// Page size for candidate selection (default: 10).
        #[serde(default, deserialize_with = "super::lenient_u32::deserialize")]
        pub per_page: Option<u32>,
        /// Also sync dependency metadata (default: false).
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
        #[serde(default, deserialize_with = "super::lenient_u32::deserialize")]
        pub estimated_seconds: Option<u32>,
        /// Estimated remaining duration in seconds.
        #[serde(default, deserialize_with = "super::lenient_u32::deserialize")]
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
        /// Text or regex pattern to search for, e.g. `"async fn connect"`.
        pub query: String,
        /// Filter by crate name, e.g. `"tokio"`.
        pub crate_name: Option<String>,
        /// Filter by semver version, e.g. `"1.40.0"`.
        pub version: Option<String>,
        /// Path glob filter, e.g. `"src/net/*"`.
        pub path_glob: Option<String>,
        /// Matching mode: `"text"` (substring) or `"regex"` (default:
        /// `"text"`).
        pub mode: Option<SourceSearchMode>,
        /// Paging cursor from a previous response's `next_cursor`.
        pub cursor: Option<String>,
        /// Page number (1-based, default: 1).
        #[serde(default, deserialize_with = "super::lenient_u32::deserialize")]
        pub page: Option<u32>,
        /// Max results per page (default: 20).
        #[serde(default, deserialize_with = "super::lenient_u32::deserialize")]
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
        #[serde(default, deserialize_with = "super::lenient_u32::deserialize")]
        pub match_line: Option<u32>,
        /// Snippet around the match.
        pub snippet: String,
    }

    /// Request payload for `source_read`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct SourceReadRequest {
        /// Crate name, e.g. `"serde"`.
        pub crate_name: String,
        /// Semver version (default: latest indexed), e.g. `"1.0.204"`.
        pub version: Option<String>,
        /// Source file path within the crate, e.g. `"src/de/mod.rs"`.
        pub path: String,
        /// Start line (inclusive, 1-based).
        #[serde(default, deserialize_with = "super::lenient_u32::deserialize")]
        pub start_line: Option<u32>,
        /// End line (inclusive, 1-based).
        #[serde(default, deserialize_with = "super::lenient_u32::deserialize")]
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
        /// Crate name, e.g. `"tokio"`.
        pub crate_name: String,
        /// Semver version (default: latest indexed), e.g. `"1.40.0"`.
        pub version: Option<String>,
        /// Source file path within the crate, e.g. `"src/runtime/mod.rs"`.
        pub path: String,
        /// Anchor line number (1-based) for context extraction.
        #[serde(default, deserialize_with = "super::lenient_u32::deserialize")]
        pub line: Option<u32>,
        /// Symbol name to resolve the anchor line automatically, e.g.
        /// `"Runtime"`.
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
        /// Symbol name or substring to search for, e.g. `"HashMap"`.
        pub query: String,
        /// Filter by crate name, e.g. `"std"`.
        pub crate_name: Option<String>,
        /// Filter by semver version, e.g. `"1.80.0"`.
        pub version: Option<String>,
        /// Filter by symbol kind, e.g. `"struct"`, `"fn"`, `"trait"`, `"enum"`.
        pub kind: Option<String>,
        /// Include results from all indexed versions, not just latest (default:
        /// false).
        pub include_all_versions: Option<bool>,
        /// Collapse entries with duplicate canonical paths (default: true).
        pub collapse_by_canonical: Option<bool>,
        /// Include doc comments (first paragraph summary) in each hit.
        pub include_docs: Option<bool>,
        /// Paging cursor from a previous response's `next_cursor`.
        pub cursor: Option<String>,
        /// Page number (1-based, default: 1).
        #[serde(default, deserialize_with = "super::lenient_u32::deserialize")]
        pub page: Option<u32>,
        /// Max results per page (default: 20).
        #[serde(default, deserialize_with = "super::lenient_u32::deserialize")]
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
        /// Doc comment (first paragraph summary when `include_docs` is true).
        #[serde(skip_serializing_if = "Option::is_none")]
        pub docs: Option<String>,
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
        /// Search query for docs.rs page content, e.g. `"async runtime"`.
        pub query: String,
        /// Filter by crate name, e.g. `"tokio"`.
        pub crate_name: Option<String>,
        /// Filter by semver version, e.g. `"1.40.0"`.
        pub version: Option<String>,
        /// Filter by docs path prefix, e.g. `"tokio/runtime"`.
        pub path_prefix: Option<String>,
        /// Paging cursor from a previous response's `next_cursor`.
        pub cursor: Option<String>,
        /// Page number (1-based, default: 1).
        #[serde(default, deserialize_with = "super::lenient_u32::deserialize")]
        pub page: Option<u32>,
        /// Max results per page (default: 10).
        #[serde(default, deserialize_with = "super::lenient_u32::deserialize")]
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
        /// Full text content of a Cargo.toml manifest file.
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

    /// Request payload for `dependency_resolve`. Provide either `dependencies`
    /// or `cargo_toml` (or both with `additions`).
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct DependencyResolveRequest {
        /// Explicit dependency list to resolve, e.g. `[{"name": "serde",
        /// "version_req": "^1"}]`.
        pub dependencies: Option<Vec<DependencyResolveInputDependency>>,
        /// Full text content of a Cargo.toml manifest to extract dependencies
        /// from.
        #[serde(default)]
        pub cargo_toml: Option<String>,
        /// Additional dependencies to resolve alongside manifest inputs.
        pub additions: Option<Vec<DependencyResolveInputDependency>>,
        /// Also analyze feature unification across resolved dependencies
        /// (default: false).
        pub check_features: Option<bool>,
        /// Max transitive dependency expansion depth (default: 1).
        #[serde(default, deserialize_with = "super::lenient_u32::deserialize")]
        pub limit: Option<u32>,
    }

    /// A single dependency input for `dependency_resolve`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct DependencyResolveInputDependency {
        /// Crate name, e.g. `"serde"`.
        pub name: String,
        /// Semver version requirement, e.g. `"^1.0"` (default: any).
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
        /// Crate name, e.g. `"tokio"`.
        pub crate_name: String,
        /// Semver version (default: latest indexed), e.g. `"1.40.0"`.
        pub version: Option<String>,
        /// Feature flags to evaluate impact for, e.g. `["rt-multi-thread",
        /// "macros"]`.
        pub features: Vec<String>,
        /// Dependency count threshold to classify a feature as "heavy"
        /// (default: 5).
        #[serde(default, deserialize_with = "super::lenient_u32::deserialize")]
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
        /// Free-text search query, e.g. `"json parser"`.
        pub query: Option<String>,
        /// Filter by crates.io category, e.g. `"web-programming"`.
        pub category: Option<String>,
        /// Filter by crates.io keyword, e.g. `"async"`.
        pub keyword: Option<String>,
        /// Sort mode: `"relevance"`, `"downloads"`, or `"recent"` (default:
        /// `"relevance"`).
        pub sort: Option<CrateSearchSort>,
        /// Paging cursor from a previous response's `next_cursor`.
        pub cursor: Option<String>,
        /// Page number (1-based, default: 1).
        #[serde(default, deserialize_with = "super::lenient_u32::deserialize")]
        pub page: Option<u32>,
        /// Max results per page (default: 10).
        #[serde(default, deserialize_with = "super::lenient_u32::deserialize")]
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
        /// Crate name, e.g. `"serde"`.
        pub crate_name: String,
        /// Semver version (default: latest indexed), e.g. `"1.0.204"`.
        pub version: Option<String>,
        /// Max version history entries to return (default: 10).
        #[serde(default, deserialize_with = "super::lenient_u32::deserialize")]
        pub versions_limit: Option<u32>,
        /// Max dependent crate entries to return (default: 10).
        #[serde(default, deserialize_with = "super::lenient_u32::deserialize")]
        pub dependents_limit: Option<u32>,
        /// Max readme content characters to include (default: 2000, 0 to omit).
        #[serde(default, deserialize_with = "super::lenient_u32::deserialize")]
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
        /// Crate name, e.g. `"serde"`.
        pub crate_name: String,
        /// Paging cursor from a previous response's `next_cursor`.
        pub cursor: Option<String>,
        /// Page number (1-based, default: 1).
        #[serde(default, deserialize_with = "super::lenient_u32::deserialize")]
        pub page: Option<u32>,
        /// Max results per page (default: 20).
        #[serde(default, deserialize_with = "super::lenient_u32::deserialize")]
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

    /// A method defined within an impl block or trait definition.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateImplMethod {
        /// Method name, e.g. `"serialize"`.
        pub name: String,
        /// Rendered method signature, e.g. `"fn serialize<S>(&self, serializer:
        /// S) -> Result<S::Ok, S::Error>"`.
        pub signature: Option<String>,
    }

    /// An associated type declared in a trait definition.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateTraitAssociatedType {
        /// Associated type name, e.g. `"Item"`.
        pub name: String,
        /// Trait bounds on the associated type, e.g. `["Display", "Send"]`.
        pub bounds: Vec<String>,
        /// Default type value, if any, e.g. `"()"`.
        pub default: Option<String>,
    }

    /// Full trait definition metadata including methods, associated types, and
    /// supertraits.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateTraitDefinition {
        /// Trait name, e.g. `"Iterator"`.
        pub trait_name: String,
        /// Whether this is an auto trait (e.g. `Send`, `Sync`).
        pub is_auto: bool,
        /// Whether the trait is declared `unsafe`.
        pub is_unsafe: bool,
        /// Whether the trait is dyn-compatible (object-safe).
        pub is_dyn_compatible: bool,
        /// Supertrait bounds, e.g. `["Clone", "Debug"]`.
        pub supertraits: Vec<String>,
        /// Methods that implementors must provide.
        pub required_methods: Vec<CrateImplMethod>,
        /// Methods with default implementations.
        pub provided_methods: Vec<CrateImplMethod>,
        /// Associated types declared in the trait.
        pub associated_types: Vec<CrateTraitAssociatedType>,
        /// Generic type parameters, e.g. `["T: Display"]`.
        pub generic_params: Vec<String>,
        /// Data source label (e.g. `"rustdoc_json"`, `"local_cache"`).
        pub index_source: String,
    }

    /// Request payload for `crate_api`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateApiRequest {
        /// Crate name, e.g. `"tokio"`.
        pub crate_name: String,
        /// Semver version (default: latest indexed), e.g. `"1.40.0"`.
        pub version: Option<String>,
        /// Glob filter for path or symbol name, e.g. `"*connection*"`.
        pub path_glob: Option<String>,
        /// Filter by symbol kinds, e.g. `["fn", "struct", "trait"]`.
        pub kinds: Option<Vec<String>>,
        /// Include doc comments (first paragraph summary) for each symbol.
        pub include_docs: Option<bool>,
        /// Paging cursor from a previous response's `next_cursor`.
        pub cursor: Option<String>,
        /// Page number (1-based, default: 1).
        #[serde(default, deserialize_with = "super::lenient_u32::deserialize")]
        pub page: Option<u32>,
        /// Max results per page (default: 50).
        #[serde(default, deserialize_with = "super::lenient_u32::deserialize")]
        pub limit: Option<u32>,
    }

    /// Response payload for `crate_api`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateApiResponse {
        /// Resolved crate name.
        pub crate_name: String,
        /// Version the API symbols were read from.
        pub selected_version: String,
        /// Latest indexed version of the crate.
        pub latest_version: String,
        /// Effective path glob filter applied.
        pub path_glob: Option<String>,
        /// Effective kind filters applied.
        pub kinds: Vec<String>,
        /// Current cursor.
        pub cursor: Option<String>,
        /// Next page cursor.
        pub next_cursor: Option<String>,
        /// Page.
        pub page: u32,
        /// Effective results-per-page limit.
        pub limit: u32,
        /// More results available.
        pub has_more: bool,
        /// Truncated by pagination.
        pub truncated: bool,
        /// Number of symbols returned in this page.
        pub count: usize,
        /// Public API symbols matching the query.
        pub symbols: Vec<CrateApiSymbol>,
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

    /// One public API symbol returned by `crate_api`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateApiSymbol {
        /// Symbol name, e.g. `"HashMap"`.
        pub name: String,
        /// Symbol kind, e.g. `"struct"`, `"fn"`, `"trait"`.
        pub kind: String,
        /// Rendered signature, e.g. `"pub fn new() -> HashMap<K, V>"`.
        pub signature: Option<String>,
        /// Visibility label, e.g. `"pub"`, `"pub(crate)"`.
        pub visibility: Option<String>,
        /// Doc comment (first paragraph summary when `include_docs` is true).
        #[serde(skip_serializing_if = "Option::is_none")]
        pub docs: Option<String>,
        /// Source file path where the symbol is defined.
        pub source_path: String,
        /// Start line in the source file.
        pub start_line: i32,
        /// End line in the source file.
        pub end_line: i32,
        /// Data source label (e.g. `"rustdoc_json"`, `"local_cache"`).
        pub index_source: String,
    }

    /// Request payload for `crate_api_diff`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateApiDiffRequest {
        /// Crate name, e.g. `"serde"`.
        pub crate_name: String,
        /// Source semver version to diff from, e.g. `"1.0.0"`.
        pub from_version: String,
        /// Target semver version to diff to, e.g. `"1.0.204"`.
        pub to_version: String,
        /// Max diff entries to return (default: 100).
        #[serde(default, deserialize_with = "super::lenient_u32::deserialize")]
        pub limit: Option<u32>,
    }

    /// Response payload for `crate_api_diff`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateApiDiffResponse {
        /// Resolved crate name.
        pub crate_name: String,
        /// Source version compared.
        pub from_version: String,
        /// Target version compared.
        pub to_version: String,
        /// Number of symbols added in the target version.
        pub added_count: usize,
        /// Number of symbols removed in the target version.
        pub removed_count: usize,
        /// Number of symbols with changed signatures or visibility.
        pub changed_count: usize,
        /// Whether any change is classified as breaking.
        pub breaking_changes_detected: bool,
        /// Individual API changes between the two versions.
        pub changes: Vec<CrateApiDiffChange>,
        /// Whether the result was truncated by the limit.
        pub truncated: bool,
        /// GitHub release notes for versions in the diff range (newest first).
        #[serde(skip_serializing_if = "Vec::is_empty", default)]
        pub release_notes: Vec<GitHubReleaseNote>,
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

    /// One API change entry in a `crate_api_diff` result.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateApiDiffChange {
        /// Symbol name that changed, e.g. `"Deserializer"`.
        pub name: String,
        /// Symbol kind, e.g. `"fn"`, `"struct"`, `"trait"`.
        pub kind: String,
        /// Type of change detected.
        pub change_type: CrateApiDiffChangeType,
        /// Signature in the source version (for changed/removed symbols).
        pub from_signature: Option<String>,
        /// Signature in the target version (for changed/added symbols).
        pub to_signature: Option<String>,
        /// Visibility in the source version (for visibility changes).
        pub from_visibility: Option<String>,
        /// Visibility in the target version (for visibility changes).
        pub to_visibility: Option<String>,
        /// Whether this change is classified as a breaking change.
        pub breaking_change: bool,
    }

    /// Type of API change between two crate versions.
    #[derive(Debug, Clone, Copy, Deserialize, Serialize, schemars::JsonSchema, PartialEq, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum CrateApiDiffChangeType {
        /// Symbol was added in the target version.
        Added,
        /// Symbol was removed in the target version.
        Removed,
        /// Symbol signature changed between versions.
        SignatureChanged,
        /// Symbol visibility changed between versions.
        VisibilityChanged,
    }

    /// Request payload for `crate_features`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateFeaturesRequest {
        /// Crate name, e.g. `"serde"`.
        pub crate_name: String,
        /// Semver version (default: latest indexed), e.g. `"1.0.204"`.
        pub version: Option<String>,
    }

    /// Response payload for `crate_features`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateFeaturesResponse {
        /// Resolved crate name.
        pub crate_name: String,
        /// Version the feature data was read from.
        pub selected_version: String,
        /// Latest indexed version of the crate.
        pub latest_version: String,
        /// Features enabled by default.
        pub default_features: Vec<String>,
        /// Total number of feature flags.
        pub feature_count: usize,
        /// Individual feature flag details.
        pub features: Vec<CrateFeatureFlag>,
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

    /// One feature flag entry returned by `crate_features`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateFeatureFlag {
        /// Feature flag name, e.g. `"derive"`.
        pub name: String,
        /// Whether this feature is enabled by default.
        pub is_default: bool,
        /// Other feature flags enabled when this feature is activated.
        pub enables_features: Vec<String>,
        /// Optional dependencies activated when this feature is enabled.
        pub enables_dependencies: Vec<String>,
        /// Full transitive closure of enabled features.
        pub transitive_enables: Vec<String>,
    }

    /// Direction for dependency graph traversal.
    #[derive(Debug, Clone, Copy, Deserialize, Serialize, schemars::JsonSchema)]
    #[serde(rename_all = "snake_case")]
    pub enum CrateGraphDirection {
        /// Traverse downstream dependencies (what this crate depends on).
        Dependencies,
        /// Traverse upstream dependents (what depends on this crate).
        Dependents,
        /// Traverse both directions.
        Both,
    }

    /// Request payload for `crate_graph`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateGraphRequest {
        /// Crate name, e.g. `"tokio"`.
        pub crate_name: String,
        /// Semver version (default: latest indexed), e.g. `"1.40.0"`.
        pub version: Option<String>,
        /// Graph traversal direction (default: `"dependencies"`).
        pub direction: Option<CrateGraphDirection>,
        /// Max traversal depth (default: 2).
        #[serde(default, deserialize_with = "super::lenient_u32::deserialize")]
        pub depth: Option<u32>,
    }

    /// Response payload for `crate_graph`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateGraphResponse {
        /// Resolved crate name.
        pub crate_name: String,
        /// Version the graph was built from.
        pub selected_version: String,
        /// Effective traversal direction.
        pub direction: CrateGraphDirection,
        /// Effective max traversal depth.
        pub depth: u32,
        /// Total nodes in the graph.
        pub node_count: usize,
        /// Total edges in the graph.
        pub edge_count: usize,
        /// Crate nodes in the dependency graph.
        pub nodes: Vec<CrateGraphNode>,
        /// Dependency edges between nodes.
        pub edges: Vec<CrateGraphEdge>,
        /// Notes about cycle-safe traversal decisions.
        pub cycle_safe_traversal_notes: Vec<String>,
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

    /// A crate node in the dependency graph.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateGraphNode {
        /// Crate name.
        pub crate_name: String,
        /// Latest indexed version of this crate.
        pub latest_version: Option<String>,
        /// Minimum graph distance from the root crate.
        pub min_distance: u32,
        /// Role in the graph, e.g. `"root"`, `"dependency"`, `"dependent"`.
        pub role: String,
    }

    /// A dependency edge in the dependency graph.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateGraphEdge {
        /// Source crate name.
        pub from_crate: String,
        /// Source crate version.
        pub from_version: Option<String>,
        /// Target crate name.
        pub to_crate: String,
        /// Target crate version.
        pub to_version: Option<String>,
        /// Semver version requirement, e.g. `"^1.0"`.
        pub requirement: String,
        /// Dependency kind: `"normal"`, `"dev"`, or `"build"`.
        pub dependency_kind: String,
        /// Whether this is an optional dependency.
        pub optional: bool,
        /// Graph depth at which this edge was discovered.
        pub depth: u32,
    }

    /// Request payload for `crate_hotspots`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateHotspotsRequest {
        /// Crate name, e.g. `"ring"`.
        pub crate_name: String,
        /// Semver version (default: latest indexed), e.g. `"0.17.8"`.
        pub version: Option<String>,
        /// Path glob filter for source files, e.g. `"src/crypto/*"`.
        pub path_glob: Option<String>,
        /// Include `unsafe` block hotspots (default: true).
        pub include_unsafe: Option<bool>,
        /// Include concurrency hotspots (default: true).
        pub include_concurrency: Option<bool>,
        /// Paging cursor from a previous response's `next_cursor`.
        pub cursor: Option<String>,
        /// Page number (1-based, default: 1).
        #[serde(default, deserialize_with = "super::lenient_u32::deserialize")]
        pub page: Option<u32>,
        /// Max results per page (default: 50).
        #[serde(default, deserialize_with = "super::lenient_u32::deserialize")]
        pub limit: Option<u32>,
    }

    /// Category of a detected hotspot.
    #[derive(Debug, Clone, Copy, Deserialize, Serialize, schemars::JsonSchema, PartialEq, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum HotspotKind {
        /// Unsafe code block or function.
        Unsafe,
        /// Concurrency primitive usage (mutex, atomic, channel, etc.).
        Concurrency,
    }

    /// Severity level of a detected hotspot.
    #[derive(Debug, Clone, Copy, Deserialize, Serialize, schemars::JsonSchema, PartialEq, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum HotspotSeverity {
        /// Low-risk pattern.
        Low,
        /// Medium-risk pattern.
        Medium,
        /// High-risk pattern.
        High,
    }

    /// Response payload for `crate_hotspots`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateHotspotsResponse {
        /// Resolved crate name.
        pub crate_name: String,
        /// Version the hotspots were detected in.
        pub selected_version: String,
        /// Latest indexed version of the crate.
        pub latest_version: String,
        /// Effective path glob filter applied.
        pub path_glob: Option<String>,
        /// Whether unsafe hotspots were included.
        pub include_unsafe: bool,
        /// Whether concurrency hotspots were included.
        pub include_concurrency: bool,
        /// Current cursor.
        pub cursor: Option<String>,
        /// Next page cursor.
        pub next_cursor: Option<String>,
        /// Page.
        pub page: u32,
        /// Effective results-per-page limit.
        pub limit: u32,
        /// More results available.
        pub has_more: bool,
        /// Truncated by pagination.
        pub truncated: bool,
        /// Number of source files scanned.
        pub scanned_files: usize,
        /// Number of hotspots returned in this page.
        pub count: usize,
        /// Detected hotspot hits.
        pub hotspots: Vec<CrateHotspotHit>,
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

    /// One hotspot hit in the `crate_hotspots` response.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateHotspotHit {
        /// Source file path containing the hotspot.
        pub path: String,
        /// Line number of the hotspot.
        pub line: u32,
        /// Hotspot category (unsafe or concurrency).
        pub kind: HotspotKind,
        /// Detected pattern, e.g. `"unsafe block"`, `"Mutex::lock"`.
        pub pattern: String,
        /// Risk severity level.
        pub severity: HotspotSeverity,
        /// Source code snippet around the hotspot.
        pub snippet: String,
    }

    /// Request payload for `crate_license_check`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateLicenseCheckRequest {
        /// Crate name, e.g. `"serde"`.
        pub crate_name: String,
        /// Semver version (default: latest indexed), e.g. `"1.0.204"`.
        pub version: Option<String>,
        /// SPDX license identifiers to allow, e.g. `["MIT", "Apache-2.0"]`.
        pub allow_licenses: Option<Vec<String>>,
        /// SPDX license identifiers to deny, e.g. `["GPL-3.0"]`.
        pub deny_licenses: Option<Vec<String>>,
    }

    /// Response payload for `crate_license_check`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateLicenseCheckResponse {
        /// Resolved crate name.
        pub crate_name: String,
        /// Version the license was read from.
        pub selected_version: String,
        /// Latest indexed version of the crate.
        pub latest_version: String,
        /// SPDX license expression from the crate manifest, e.g. `"MIT OR
        /// Apache-2.0"`.
        pub license_expression: Option<String>,
        /// Individual SPDX license identifiers parsed from the expression.
        pub matched_licenses: Vec<String>,
        /// Effective allow-list used for evaluation.
        pub allow_licenses: Vec<String>,
        /// Effective deny-list used for evaluation.
        pub deny_licenses: Vec<String>,
        /// Policy evaluation result: `allowed`, `denied`, or `unknown`.
        pub policy_result: LicensePolicyResult,
        /// Human-readable reasons for the policy result.
        pub policy_reasons: Vec<String>,
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

    /// Request payload for `crate_usage_patterns`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateUsagePatternsRequest {
        /// Crate name whose symbol to search for in dependents, e.g. `"serde"`.
        pub crate_name: String,
        /// Symbol name to find usage examples of, e.g. `"Serialize"`.
        pub symbol_name: String,
        /// Semver version (default: latest indexed), e.g. `"1.0.204"`.
        pub version: Option<String>,
        /// Paging cursor from a previous response's `next_cursor`.
        pub cursor: Option<String>,
        /// Page number (1-based, default: 1).
        #[serde(default, deserialize_with = "super::lenient_u32::deserialize")]
        pub page: Option<u32>,
        /// Max results per page (default: 20).
        #[serde(default, deserialize_with = "super::lenient_u32::deserialize")]
        pub limit: Option<u32>,
    }

    /// Response payload for `crate_usage_patterns`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateUsagePatternsResponse {
        /// Resolved crate name.
        pub crate_name: String,
        /// Version the usage search was scoped to.
        pub selected_version: String,
        /// Latest indexed version of the crate.
        pub latest_version: String,
        /// Symbol name searched for.
        pub symbol_name: String,
        /// Current cursor.
        pub cursor: Option<String>,
        /// Next page cursor.
        pub next_cursor: Option<String>,
        /// Page.
        pub page: u32,
        /// Effective results-per-page limit.
        pub limit: u32,
        /// More results available.
        pub has_more: bool,
        /// Truncated by pagination.
        pub truncated: bool,
        /// Number of usage patterns returned in this page.
        pub count: usize,
        /// Number of dependent crate versions whose source directories were
        /// scanned. Helps distinguish "no source available" from "symbol not
        /// found in dependents".
        pub scanned_dependents: usize,
        /// Source snippets showing how the symbol is used in dependent crates.
        pub patterns: Vec<CrateUsagePattern>,
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

    /// One usage pattern found in a dependent crate's source code.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateUsagePattern {
        /// Dependent crate name that uses the symbol.
        pub dependent_crate: String,
        /// Version of the dependent crate.
        pub dependent_version: String,
        /// Aggregated download count of the dependent crate.
        pub dependent_downloads: i64,
        /// Source file path in the dependent crate.
        pub path: String,
        /// Start line of the usage snippet.
        pub line_start: u32,
        /// End line of the usage snippet.
        pub line_end: u32,
        /// Source code snippet showing the usage.
        pub snippet: String,
    }

    /// Request payload for `crate_re_exports`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateReExportsRequest {
        /// Crate name, e.g. `"tokio"`.
        pub crate_name: String,
        /// Semver version (default: latest indexed), e.g. `"1.40.0"`.
        pub version: Option<String>,
        /// Filter re-exports by module path prefix, e.g. `"tokio::io"`.
        pub path_prefix: Option<String>,
        /// Paging cursor from a previous response's `next_cursor`.
        pub cursor: Option<String>,
        /// Page number (1-based, default: 1).
        #[serde(default, deserialize_with = "super::lenient_u32::deserialize")]
        pub page: Option<u32>,
        /// Max results per page (default: 50).
        #[serde(default, deserialize_with = "super::lenient_u32::deserialize")]
        pub limit: Option<u32>,
    }

    /// Response payload for `crate_re_exports`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateReExportsResponse {
        /// Resolved crate name.
        pub crate_name: String,
        /// Version the re-export data was read from.
        pub selected_version: String,
        /// Latest indexed version of the crate.
        pub latest_version: String,
        /// Effective path prefix filter applied.
        pub path_prefix: Option<String>,
        /// Current cursor.
        pub cursor: Option<String>,
        /// Next page cursor.
        pub next_cursor: Option<String>,
        /// Page.
        pub page: u32,
        /// Effective results-per-page limit.
        pub limit: u32,
        /// More results available.
        pub has_more: bool,
        /// Truncated by pagination.
        pub truncated: bool,
        /// Number of re-export entries returned.
        pub count: usize,
        /// Re-export mapping entries.
        pub re_exports: Vec<CrateReExportEntry>,
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

    /// One re-export mapping entry.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateReExportEntry {
        /// Canonical public path for the re-exported item, e.g.
        /// `"tokio::io::AsyncRead"`.
        pub canonical_path: String,
        /// Original internal definition path, e.g.
        /// `"tokio::io::async_read::AsyncRead"`.
        pub original_definition_path: String,
        /// Symbol kind, e.g. `"trait"`, `"struct"`.
        pub kind: String,
        /// Visibility at the re-export site, e.g. `"pub"`.
        pub visibility: String,
        /// Whether this is the shortest public import path.
        pub shortest_public_path: bool,
        /// Source file where the re-export is declared.
        pub source_path: String,
        /// Line number of the re-export declaration.
        pub source_line: u32,
    }

    /// Request payload for `crate_import_path`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateImportPathRequest {
        /// Crate name, e.g. `"serde"`.
        pub crate_name: String,
        /// Symbol name to resolve the import path for, e.g. `"Deserialize"`.
        pub symbol_name: String,
        /// Semver version (default: latest indexed), e.g. `"1.0.204"`.
        pub version: Option<String>,
        /// Filter by symbol kind, e.g. `"trait"`, `"struct"`, `"fn"`.
        pub kind: Option<String>,
        /// Paging cursor from a previous response's `next_cursor`.
        pub cursor: Option<String>,
        /// Page number (1-based, default: 1).
        #[serde(default, deserialize_with = "super::lenient_u32::deserialize")]
        pub page: Option<u32>,
        /// Max results per page (default: 20).
        #[serde(default, deserialize_with = "super::lenient_u32::deserialize")]
        pub limit: Option<u32>,
    }

    /// Response payload for `crate_import_path`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateImportPathResponse {
        /// Resolved crate name.
        pub crate_name: String,
        /// Version the import paths were resolved from.
        pub selected_version: String,
        /// Latest indexed version of the crate.
        pub latest_version: String,
        /// Symbol name searched for.
        pub symbol_name: String,
        /// Effective kind filter applied.
        pub kind: Option<String>,
        /// Current cursor.
        pub cursor: Option<String>,
        /// Next page cursor.
        pub next_cursor: Option<String>,
        /// Page.
        pub page: u32,
        /// Effective results-per-page limit.
        pub limit: u32,
        /// More results available.
        pub has_more: bool,
        /// Truncated by pagination.
        pub truncated: bool,
        /// Number of matches returned.
        pub count: usize,
        /// Recommended shortest public import path, e.g.
        /// `"serde::Deserialize"`.
        pub best_import_path: Option<String>,
        /// All matching import paths for the symbol.
        pub matches: Vec<CrateImportPathMatch>,
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

    /// One import path match for a symbol.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateImportPathMatch {
        /// Matched symbol name.
        pub symbol_name: String,
        /// Symbol kind, e.g. `"trait"`, `"struct"`.
        pub kind: String,
        /// Public import path, e.g. `"serde::Deserialize"`.
        pub import_path: String,
        /// Internal definition path (may differ from import path due to
        /// re-exports).
        pub definition_path: Option<String>,
        /// Source file path where the symbol is defined.
        pub source_path: String,
        /// Start line of the symbol definition.
        pub start_line: u32,
        /// End line of the symbol definition.
        pub end_line: u32,
        /// Data source label (e.g. `"rustdoc_json"`, `"local_cache"`).
        pub index_source: String,
        /// Whether the symbol name matched exactly (vs. substring).
        pub exact_name_match: bool,
    }

    /// Request payload for `crate_migration_path`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateMigrationPathRequest {
        /// Crate name, e.g. `"axum"`.
        pub crate_name: String,
        /// Source semver version to migrate from, e.g. `"0.6.0"`.
        pub from_version: String,
        /// Target semver version to migrate to, e.g. `"0.7.0"`.
        pub to_version: String,
        /// Max migration action entries to return (default: 100).
        #[serde(default, deserialize_with = "super::lenient_u32::deserialize")]
        pub limit: Option<u32>,
    }

    /// Response payload for `crate_migration_path`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateMigrationPathResponse {
        /// Resolved crate name.
        pub crate_name: String,
        /// Source version migrating from.
        pub from_version: String,
        /// Target version migrating to.
        pub to_version: String,
        /// Whether any breaking changes were detected.
        pub breaking_changes_detected: bool,
        /// Number of symbols added in the target version.
        pub added_count: usize,
        /// Number of symbols removed in the target version.
        pub removed_count: usize,
        /// Number of symbols with changed signatures.
        pub changed_count: usize,
        /// Actionable migration steps for the upgrade.
        pub migration_actions: Vec<CrateMigrationAction>,
        /// GitHub release notes for versions in the migration range (newest
        /// first).
        #[serde(skip_serializing_if = "Vec::is_empty", default)]
        pub release_notes: Vec<GitHubReleaseNote>,
        /// Confidence level.
        pub confidence: String,
        /// Confidence details.
        pub confidence_assessment: ConfidenceAssessment,
        /// Suggested next tools.
        pub suggested_next_tools: Vec<String>,
        /// Data origin.
        pub provenance: String,
    }

    /// One migration action step in a `crate_migration_path` response.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateMigrationAction {
        /// Action verb, e.g. `"replace"`, `"remove"`, `"update"`.
        pub action: String,
        /// Human-readable rationale for the migration action.
        pub rationale: String,
        /// Symbol name affected by this action.
        pub affected_symbol: String,
        /// Symbol kind, e.g. `"fn"`, `"struct"`, `"trait"`.
        pub kind: String,
        /// Signature in the source version (for changed/removed symbols).
        pub from_signature: Option<String>,
        /// Signature in the target version (for changed/added symbols).
        pub to_signature: Option<String>,
    }

    /// Request payload for `crate_compare`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateCompareRequest {
        /// First crate name to compare, e.g. `"reqwest"`.
        pub left_crate: String,
        /// Second crate name to compare, e.g. `"ureq"`.
        pub right_crate: String,
        /// Semver version for the first crate (default: latest indexed).
        pub left_version: Option<String>,
        /// Semver version for the second crate (default: latest indexed).
        pub right_version: Option<String>,
    }

    /// Response payload for `crate_compare`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateCompareResponse {
        /// Metrics and metadata for the first crate.
        pub left: CrateCompareSide,
        /// Metrics and metadata for the second crate.
        pub right: CrateCompareSide,
        /// Recommended crate choice (crate name or `null` if no clear winner).
        pub recommendation: Option<String>,
        /// Reasons supporting the recommendation.
        pub recommendation_reasons: Vec<String>,
        /// Freshness sources.
        pub freshness: Vec<ResponseFreshnessSource>,
        /// Freshness check done.
        pub freshness_check_performed: bool,
        /// Freshness result.
        pub freshness_check_result: String,
        /// Refresh enqueued.
        pub refresh_enqueued: bool,
        /// Refresh job ID.
        pub refresh_job_id: Option<String>,
        /// Confidence level.
        pub confidence: String,
        /// Confidence details.
        pub confidence_assessment: ConfidenceAssessment,
        /// Suggested next tools.
        pub suggested_next_tools: Vec<String>,
        /// Data origin.
        pub provenance: String,
    }

    /// Metrics for one side of a `crate_compare` result.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateCompareSide {
        /// Crate name.
        pub crate_name: String,
        /// Version used for comparison.
        pub selected_version: String,
        /// Latest indexed version.
        pub latest_version: String,
        /// Minimum Rust version for the selected version.
        pub selected_rust_version: Option<String>,
        /// Publication timestamp for the selected version.
        pub selected_published_at: Option<String>,
        /// SPDX license expression.
        pub license_expression: Option<String>,
        /// Aggregated download count.
        pub total_downloads: i64,
        /// Number of crates that depend on this crate.
        pub dependent_crate_count: i64,
        /// Number of dependencies.
        pub dependency_count: i64,
        /// Number of feature flags.
        pub feature_count: i64,
        /// Number of security advisories.
        pub advisory_count: i64,
        /// Whether the selected version is yanked.
        pub yanked: bool,
        /// Computed maintenance health score (0.0 - 1.0).
        pub maintenance_score: f64,
    }

    /// Request payload for `crate_compatibility`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateCompatibilityRequest {
        /// First crate name, e.g. `"tokio"`.
        pub left_crate: String,
        /// Semver version for the first crate (default: latest indexed).
        pub left_version: Option<String>,
        /// Second crate name, e.g. `"async-std"`.
        pub right_crate: String,
        /// Semver version for the second crate (default: latest indexed).
        pub right_version: Option<String>,
        /// Also analyze feature unification between the two crates (default:
        /// false).
        pub check_features: Option<bool>,
    }

    /// Request payload for `crate_compatibility_matrix`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateCompatibilityMatrixRequest {
        /// First crate name, e.g. `"tokio"`.
        pub left_crate: String,
        /// Second crate name, e.g. `"hyper"`.
        pub right_crate: String,
        /// Specific versions of the first crate to test, e.g. `["1.38.0",
        /// "1.40.0"]`.
        pub left_versions: Option<Vec<String>>,
        /// Specific versions of the second crate to test, e.g. `["1.4.0",
        /// "1.5.0"]`.
        pub right_versions: Option<Vec<String>>,
        /// Also analyze feature unification for each pair (default: false).
        pub check_features: Option<bool>,
        /// Max versions to test per crate when no explicit list given (default:
        /// 5).
        #[serde(default, deserialize_with = "super::lenient_u32::deserialize")]
        pub version_limit: Option<u32>,
        /// Max total version pairs to test (default: 25).
        #[serde(default, deserialize_with = "super::lenient_u32::deserialize")]
        pub max_pairs: Option<u32>,
    }

    /// Response payload for `crate_compatibility`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateCompatibilityResponse {
        /// First crate name.
        pub left_crate: String,
        /// Version tested for the first crate.
        pub left_version: Option<String>,
        /// Second crate name.
        pub right_crate: String,
        /// Version tested for the second crate.
        pub right_version: Option<String>,
        /// Whether the two crates can coexist without dependency conflicts.
        pub resolvable: bool,
        /// Resolved dependency versions when compatible.
        pub resolved_versions: Vec<DependencyResolveResolvedVersion>,
        /// Conflicts preventing compatibility.
        pub conflicts: Vec<DependencyResolveConflict>,
        /// Whether feature unification was analyzed.
        pub check_features: bool,
        /// Feature unification summary (when `check_features` was true).
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

    /// Response payload for `crate_compatibility_matrix`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateCompatibilityMatrixResponse {
        /// First crate name.
        pub left_crate: String,
        /// Second crate name.
        pub right_crate: String,
        /// Whether feature unification was analyzed.
        pub check_features: bool,
        /// Total number of version pairs tested.
        pub pairs_tested: usize,
        /// Version pairs that resolved without conflicts.
        pub compatible_pairs: Vec<CrateCompatibilityMatrixEntry>,
        /// Version pairs that had dependency conflicts.
        pub incompatible_pairs: Vec<CrateCompatibilityMatrixEntry>,
        /// Confidence level.
        pub confidence: String,
        /// Confidence details.
        pub confidence_assessment: ConfidenceAssessment,
        /// Suggested next tools.
        pub suggested_next_tools: Vec<String>,
        /// Data origin.
        pub provenance: String,
    }

    /// One version-pair entry in the compatibility matrix.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateCompatibilityMatrixEntry {
        /// Version of the first crate tested.
        pub left_version: String,
        /// Version of the second crate tested.
        pub right_version: String,
        /// Whether this pair resolved without conflicts.
        pub resolvable: bool,
        /// Conflict messages (empty when compatible).
        pub conflict_messages: Vec<String>,
    }

    /// Request payload for `crate_derive_macros`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateDeriveMacrosRequest {
        /// Crate name, e.g. `"serde_derive"`.
        pub crate_name: String,
        /// Semver version (default: latest indexed), e.g. `"1.0.204"`.
        pub version: Option<String>,
    }

    /// Response payload for `crate_derive_macros`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateDeriveMacrosResponse {
        /// Resolved crate name.
        pub crate_name: String,
        /// Version the macro data was read from.
        pub selected_version: String,
        /// Latest indexed version of the crate.
        pub latest_version: String,
        /// Derive macros exported by the crate.
        pub derive_macros: Vec<CrateDeriveMacroEntry>,
        /// Attribute macros exported by the crate.
        pub attribute_macros: Vec<CrateAttributeMacroEntry>,
        /// Function-like procedural macros exported by the crate.
        pub function_like_macros: Vec<CrateFunctionLikeMacroEntry>,
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

    /// One derive macro exported by a crate.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateDeriveMacroEntry {
        /// Derive macro name, e.g. `"Serialize"`.
        pub name: String,
        /// Helper attributes accepted by this derive, e.g. `["serde"]`.
        pub accepted_attributes: Vec<String>,
        /// Example usage pattern, e.g. `"#[derive(Serialize)]"`.
        pub usage_pattern: String,
        /// Source file path where the macro is defined.
        pub source_path: String,
        /// Source line where the macro is defined.
        pub source_line: i32,
    }

    /// One attribute macro exported by a crate.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateAttributeMacroEntry {
        /// Attribute macro name, e.g. `"tokio::main"`.
        pub name: String,
        /// Example usage pattern, e.g. `"#[tokio::main]"`.
        pub usage_pattern: String,
        /// Rendered macro signature, if available.
        pub signature_pattern: Option<String>,
        /// Source file path where the macro is defined.
        pub source_path: String,
        /// Source line where the macro is defined.
        pub source_line: i32,
    }

    /// One function-like procedural macro exported by a crate.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateFunctionLikeMacroEntry {
        /// Macro name, e.g. `"html!"`.
        pub name: String,
        /// Example usage pattern, e.g. `"html! { <div>...</div> }"`.
        pub usage_pattern: String,
        /// Rendered macro signature, if available.
        pub signature_pattern: Option<String>,
        /// Source file path where the macro is defined.
        pub source_path: String,
        /// Source line where the macro is defined.
        pub source_line: i32,
    }

    /// Request payload for `crate_error_types`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateErrorTypesRequest {
        /// Crate name, e.g. `"anyhow"`.
        pub crate_name: String,
        /// Semver version (default: latest indexed), e.g. `"1.0.86"`.
        pub version: Option<String>,
        /// Filter by error type name, e.g. `"Error"`.
        pub type_name: Option<String>,
        /// Paging cursor from a previous response's `next_cursor`.
        pub cursor: Option<String>,
        /// Page number (1-based, default: 1).
        #[serde(default, deserialize_with = "super::lenient_u32::deserialize")]
        pub page: Option<u32>,
        /// Max results per page (default: 50).
        #[serde(default, deserialize_with = "super::lenient_u32::deserialize")]
        pub limit: Option<u32>,
    }

    /// Response payload for `crate_error_types`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateErrorTypesResponse {
        /// Resolved crate name.
        pub crate_name: String,
        /// Version the error types were read from.
        pub selected_version: String,
        /// Latest indexed version of the crate.
        pub latest_version: String,
        /// Effective type name filter applied.
        pub type_name: Option<String>,
        /// Current cursor.
        pub cursor: Option<String>,
        /// Next page cursor.
        pub next_cursor: Option<String>,
        /// Page.
        pub page: u32,
        /// Effective results-per-page limit.
        pub limit: u32,
        /// More results available.
        pub has_more: bool,
        /// Truncated by pagination.
        pub truncated: bool,
        /// Number of error types returned.
        pub count: usize,
        /// Error type entries found in the crate.
        pub error_types: Vec<CrateErrorTypeEntry>,
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

    /// One error type entry with conversion and usage metadata.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateErrorTypeEntry {
        /// Error type name, e.g. `"Error"`.
        pub type_name: String,
        /// Type kind, e.g. `"enum"`, `"struct"`.
        pub kind: String,
        /// Enum variant names (for enum error types).
        pub variants: Vec<String>,
        /// Struct field names (for struct error types).
        pub fields: Vec<String>,
        /// `Display` format patterns from the impl.
        pub display_patterns: Vec<String>,
        /// Types this error converts from via `From` impls, e.g.
        /// `["std::io::Error"]`.
        pub from_conversions: Vec<String>,
        /// Functions that return this error type in their signature.
        pub returned_by: Vec<String>,
        /// Source file path where the error type is defined.
        pub source_path: String,
        /// Source line where the error type is defined.
        pub source_line: i32,
    }

    /// Request payload for `crate_alternatives`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateAlternativesRequest {
        /// Crate name to find alternatives for, e.g. `"reqwest"`.
        pub crate_name: String,
        /// Semver version (default: latest indexed), e.g. `"0.12.5"`.
        pub version: Option<String>,
        /// Paging cursor from a previous response's `next_cursor`.
        pub cursor: Option<String>,
        /// Page number (1-based, default: 1).
        #[serde(default, deserialize_with = "super::lenient_u32::deserialize")]
        pub page: Option<u32>,
        /// Max results per page (default: 10).
        #[serde(default, deserialize_with = "super::lenient_u32::deserialize")]
        pub limit: Option<u32>,
        /// SPDX license identifiers to allow, e.g. `["MIT", "Apache-2.0"]`.
        pub allow_licenses: Option<Vec<String>>,
        /// SPDX license identifiers to deny, e.g. `["GPL-3.0"]`.
        pub deny_licenses: Option<Vec<String>>,
    }

    /// Response payload for `crate_alternatives`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateAlternativesResponse {
        /// Resolved crate name.
        pub crate_name: String,
        /// Version used for category/keyword matching.
        pub selected_version: String,
        /// Latest indexed version of the crate.
        pub latest_version: String,
        /// Current cursor.
        pub cursor: Option<String>,
        /// Next page cursor.
        pub next_cursor: Option<String>,
        /// Page.
        pub page: u32,
        /// Effective results-per-page limit.
        pub limit: u32,
        /// More results available.
        pub has_more: bool,
        /// Truncated by pagination.
        pub truncated: bool,
        /// Number of alternatives returned.
        pub count: usize,
        /// Effective allow-list used for license evaluation.
        pub allow_licenses: Vec<String>,
        /// Effective deny-list used for license evaluation.
        pub deny_licenses: Vec<String>,
        /// Ranked alternative crate suggestions.
        pub alternatives: Vec<CrateAlternativeHit>,
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

    /// One alternative crate suggestion.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateAlternativeHit {
        /// Alternative crate name.
        pub crate_name: String,
        /// Latest indexed version.
        pub latest_version: Option<String>,
        /// Crate description.
        pub description: Option<String>,
        /// Category labels.
        pub categories: Vec<String>,
        /// Keyword labels.
        pub keywords: Vec<String>,
        /// Aggregated download count.
        pub total_downloads: i64,
        /// Number of dependent crates.
        pub dependent_crates: i64,
        /// Number of security advisories.
        pub advisory_count: i64,
        /// Whether the latest version is yanked.
        pub yanked: bool,
        /// SPDX license expression.
        pub license_expression: Option<String>,
        /// License policy evaluation result.
        pub policy_result: LicensePolicyResult,
        /// Human-readable reasons for the policy result.
        pub policy_reasons: Vec<String>,
        /// Computed relevance score (higher is better).
        pub score: f64,
        /// Factors contributing to the ranking.
        pub rank_reasons: Vec<String>,
    }

    /// Request payload for `crate_trait_impls`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateTraitImplsRequest {
        /// Crate name, e.g. `"serde"`.
        pub crate_name: String,
        /// Semver version (default: latest indexed), e.g. `"1.0.204"`.
        pub version: Option<String>,
        /// Filter by trait name, e.g. `"Serialize"`.
        pub trait_name: Option<String>,
        /// Filter by implementing type name, e.g. `"HashMap"`.
        pub type_name: Option<String>,
        /// Include doc comments (first paragraph summary) for each impl.
        pub include_docs: Option<bool>,
        /// Paging cursor from a previous response's `next_cursor`.
        pub cursor: Option<String>,
        /// Page number (1-based, default: 1).
        #[serde(default, deserialize_with = "super::lenient_u32::deserialize")]
        pub page: Option<u32>,
        /// Max results per page (default: 50).
        #[serde(default, deserialize_with = "super::lenient_u32::deserialize")]
        pub limit: Option<u32>,
    }

    /// Response payload for `crate_trait_impls`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateTraitImplsResponse {
        /// Resolved crate name.
        pub crate_name: String,
        /// Version the impl data was read from.
        pub selected_version: String,
        /// Latest indexed version of the crate.
        pub latest_version: String,
        /// Effective trait name filter applied.
        pub trait_name: Option<String>,
        /// Effective type name filter applied.
        pub type_name: Option<String>,
        /// Current cursor.
        pub cursor: Option<String>,
        /// Next page cursor.
        pub next_cursor: Option<String>,
        /// Page.
        pub page: u32,
        /// Effective results-per-page limit.
        pub limit: u32,
        /// More results available.
        pub has_more: bool,
        /// Truncated by pagination.
        pub truncated: bool,
        /// Number of impl relations returned.
        pub count: usize,
        /// Trait implementation relationships found.
        pub impls: Vec<CrateTraitImplRelation>,
        /// Full trait definitions for traits referenced in `impls`.
        pub trait_definitions: Vec<CrateTraitDefinition>,
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

    /// One trait implementation relationship.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateTraitImplRelation {
        /// Implementing type name, e.g. `"Vec"`.
        pub type_name: String,
        /// Rendered type name with generics, e.g. `"Vec<T>"`.
        pub type_name_display: Option<String>,
        /// Trait being implemented (null for inherent impls).
        pub trait_name: Option<String>,
        /// Rendered trait name with generics.
        pub trait_name_display: Option<String>,
        /// Impl kind label, e.g. `"direct"`, `"blanket"`, `"auto"`.
        pub impl_kind: String,
        /// Whether this is a blanket impl (e.g. `impl<T: Display> ToString for
        /// T`).
        pub blanket_impl: bool,
        /// Whether this is a compiler-synthesized impl.
        pub synthetic_impl: bool,
        /// Whether this is a negative impl (e.g. `impl !Send for Type`).
        pub negative_impl: bool,
        /// Blanket type parameter when `blanket_impl` is true, e.g. `"T"`.
        pub blanket_type: Option<String>,
        /// Generic type parameters on the impl block.
        pub generic_params: Vec<String>,
        /// Where clauses on the impl block.
        pub where_clauses: Vec<String>,
        /// Methods provided by this impl.
        pub methods: Vec<CrateImplMethod>,
        /// Doc comment (first paragraph summary when `include_docs` is true).
        #[serde(skip_serializing_if = "Option::is_none")]
        pub docs: Option<String>,
        /// Source file path where the impl is defined.
        pub source_path: String,
        /// Start line of the impl block.
        pub start_line: i32,
        /// End line of the impl block.
        pub end_line: i32,
        /// Data source label (e.g. `"rustdoc_json"`, `"local_cache"`).
        pub index_source: String,
    }

    /// Request payload for `crate_type_info`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateTypeInfoRequest {
        /// Crate name, e.g. `"tokio"`.
        pub crate_name: String,
        /// Type name to look up, e.g. `"Runtime"`.
        pub type_name: String,
        /// Semver version (default: latest indexed), e.g. `"1.40.0"`.
        pub version: Option<String>,
        /// Include inherent methods in the response (default: true).
        pub include_methods: Option<bool>,
        /// Include trait implementations in the response (default: true).
        pub include_trait_impls: Option<bool>,
        /// Include full doc comments for the type definition.
        pub include_docs: Option<bool>,
    }

    /// Response payload for `crate_type_info`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateTypeInfoResponse {
        /// Resolved crate name.
        pub crate_name: String,
        /// Version the type info was read from.
        pub selected_version: String,
        /// Latest indexed version of the crate.
        pub latest_version: String,
        /// Type name queried.
        pub type_name: String,
        /// Whether inherent methods were included.
        pub include_methods: bool,
        /// Whether trait impls were included.
        pub include_trait_impls: bool,
        /// Type definition metadata (null if type not found).
        pub type_definition: Option<CrateTypeDefinition>,
        /// Inherent methods (non-trait `impl` methods).
        pub inherent_methods: Vec<CrateImplMethod>,
        /// Trait implementations for this type.
        pub trait_impls: Vec<CrateTraitImpl>,
        /// Full trait definitions for traits referenced in `trait_impls`.
        pub trait_definitions: Vec<CrateTraitDefinition>,
        /// Type conversions (`From`/`Into`/`TryFrom`/`TryInto` impls).
        pub conversions: Vec<CrateTypeConversion>,
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

    /// Full type definition metadata returned by `crate_type_info`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateTypeDefinition {
        /// Type name, e.g. `"Runtime"`.
        pub type_name: String,
        /// Type kind, e.g. `"struct"`, `"enum"`, `"union"`, `"type_alias"`.
        pub kind: String,
        /// Visibility label, e.g. `"pub"`.
        pub visibility: Option<String>,
        /// Canonical public path, e.g. `"tokio::runtime::Runtime"`.
        pub canonical_path: Option<String>,
        /// Internal definition path (may differ from canonical due to
        /// re-exports).
        pub definition_path: Option<String>,
        /// Generic type parameters, e.g. `["T: Send"]`.
        pub generic_params: Vec<String>,
        /// Where clause constraints.
        pub where_clauses: Vec<String>,
        /// Struct fields (for structs).
        pub fields: Vec<CrateTypeField>,
        /// Enum variants (for enums).
        pub variants: Vec<CrateTypeVariant>,
        /// Version since which the type is deprecated.
        pub deprecated_since: Option<String>,
        /// Deprecation note or replacement suggestion.
        pub deprecated_note: Option<String>,
        /// Whether the type is marked `#[non_exhaustive]`.
        pub is_non_exhaustive: bool,
        /// Auto traits implemented (e.g. `["Send", "Sync"]`).
        pub auto_traits: Vec<String>,
        /// Full doc comment (when `include_docs` is true).
        #[serde(skip_serializing_if = "Option::is_none")]
        pub docs: Option<String>,
        /// Source file path where the type is defined.
        pub source_path: String,
        /// Start line of the type definition.
        pub start_line: i32,
        /// End line of the type definition.
        pub end_line: i32,
        /// Data source label (e.g. `"rustdoc_json"`, `"local_cache"`).
        pub index_source: String,
    }

    /// A field in a struct or enum variant.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateTypeField {
        /// Field name (null for tuple struct fields).
        pub name: Option<String>,
        /// Field type expression, e.g. `"String"`, `"Vec<u8>"`.
        pub field_type: String,
    }

    /// An enum variant with its fields.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateTypeVariant {
        /// Variant name, e.g. `"Ok"`, `"Err"`.
        pub name: String,
        /// Fields within this variant.
        pub fields: Vec<CrateTypeField>,
    }

    /// A trait implementation for a specific type (used in `crate_type_info`).
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateTraitImpl {
        /// Trait being implemented (null for inherent impls).
        pub trait_name: Option<String>,
        /// Rendered trait name with generics.
        pub trait_name_display: Option<String>,
        /// Impl kind label, e.g. `"direct"`, `"blanket"`, `"auto"`.
        pub impl_kind: String,
        /// Whether this is a blanket impl.
        pub blanket_impl: bool,
        /// Whether this is a compiler-synthesized impl.
        pub synthetic_impl: bool,
        /// Whether this is a negative impl.
        pub negative_impl: bool,
        /// Blanket type parameter when `blanket_impl` is true.
        pub blanket_type: Option<String>,
        /// Generic type parameters on the impl block.
        pub generic_params: Vec<String>,
        /// Where clauses on the impl block.
        pub where_clauses: Vec<String>,
        /// Methods provided by this impl.
        pub methods: Vec<CrateImplMethod>,
        /// Source file path where the impl is defined.
        pub source_path: String,
        /// Start line of the impl block.
        pub start_line: i32,
        /// End line of the impl block.
        pub end_line: i32,
        /// Data source label (e.g. `"rustdoc_json"`, `"local_cache"`).
        pub index_source: String,
    }

    /// A type conversion relationship (From/Into/TryFrom/TryInto).
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateTypeConversion {
        /// Conversion trait name, e.g. `"From"`, `"TryInto"`.
        pub trait_name: String,
        /// Source type in the conversion.
        pub source_type: Option<String>,
        /// Target type in the conversion.
        pub target_type: Option<String>,
    }

    /// Request payload for `crate_deprecated`.
    #[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
    pub struct CrateDeprecatedRequest {
        /// Crate name.
        pub crate_name: String,
        /// Version (default: latest).
        pub version: Option<String>,
        /// Include doc comments (first paragraph summary) for each item.
        pub include_docs: Option<bool>,
        /// Max results (default: 50).
        #[serde(default, deserialize_with = "super::lenient_u32::deserialize")]
        pub limit: Option<u32>,
        /// Page (1-based).
        #[serde(default, deserialize_with = "super::lenient_u32::deserialize")]
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
        /// Doc comment (first paragraph summary when `include_docs` is true).
        #[serde(skip_serializing_if = "Option::is_none")]
        pub docs: Option<String>,
        /// Index source.
        pub index_source: String,
    }
}
