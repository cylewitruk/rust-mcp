use std::collections::BTreeMap;

use rmcp::schemars;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::FromRow;

#[derive(Debug, Deserialize)]
pub(super) struct CratesIoSearchResponse {
    pub(super) crates: Vec<CratesIoSearchCrate>,
    pub(super) meta: CratesIoSearchMeta,
}

#[derive(Debug, Deserialize)]
pub(super) struct CratesIoSearchMeta {
    pub(super) total: u64,
}

#[derive(Debug, Deserialize)]
pub(super) struct CratesIoSearchCrate {
    pub(super) id: String,
    #[allow(dead_code)]
    pub(super) name: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct CratesIoCrateDetailResponse {
    #[serde(rename = "crate")]
    pub(super) krate: CratesIoCrateRecord,
    pub(super) versions: Vec<CratesIoVersionRecord>,
    #[serde(default)]
    pub(super) keywords: Vec<CratesIoKeyword>,
    #[serde(default)]
    pub(super) categories: Vec<CratesIoCategory>,
}

#[derive(Debug, Deserialize)]
pub(super) struct CratesIoCrateRecord {
    pub(super) name: String,
    pub(super) description: Option<String>,
    pub(super) repository: Option<String>,
    pub(super) documentation: Option<String>,
    pub(super) homepage: Option<String>,
    pub(super) max_version: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct CratesIoVersionRecord {
    pub(super) num: String,
    pub(super) created_at: Option<String>,
    pub(super) updated_at: Option<String>,
    #[serde(default)]
    pub(super) yanked: bool,
    pub(super) downloads: Option<i64>,
    pub(super) checksum: Option<String>,
    pub(super) rust_version: Option<String>,
    pub(super) license: Option<String>,
    #[serde(default)]
    pub(super) features: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub(super) struct CratesIoKeyword {
    pub(super) id: String,
    pub(super) keyword: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct CratesIoCategory {
    pub(super) id: String,
    pub(super) slug: Option<String>,
    pub(super) category: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct CratesIoDependenciesResponse {
    #[serde(default)]
    pub(super) dependencies: Vec<CratesIoDependency>,
}

#[derive(Debug, Deserialize)]
pub(super) struct CratesIoDependency {
    pub(super) crate_id: String,
    pub(super) req: String,
    pub(super) kind: Option<String>,
    #[serde(default)]
    pub(super) optional: bool,
    #[serde(default)]
    pub(super) features: Vec<String>,
}

#[derive(Debug)]
pub(super) struct SyncCrateOutcome {
    pub(super) versions_synced: usize,
    pub(super) dependencies_synced: usize,
    pub(super) selected_version: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PingRequest {
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum CrateSearchSort {
    Relevance,
    Downloads,
    Recent,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CrateSearchRequest {
    pub query: Option<String>,
    pub category: Option<String>,
    pub keyword: Option<String>,
    pub sort: Option<CrateSearchSort>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SourceSearchMode {
    Text,
    Regex,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SourceSearchRequest {
    pub query: String,
    pub crate_name: Option<String>,
    pub version: Option<String>,
    pub path_glob: Option<String>,
    pub mode: Option<SourceSearchMode>,
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct SourceSearchResponse {
    pub query: String,
    pub crate_name: Option<String>,
    pub version: Option<String>,
    pub path_glob: Option<String>,
    pub mode: SourceSearchMode,
    pub limit: u32,
    pub count: usize,
    pub confidence: String,
    pub confidence_assessment: ConfidenceAssessment,
    pub next_best_calls: Vec<String>,
    pub provenance: String,
    pub hits: Vec<SourceSearchHit>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct SourceSearchHit {
    pub crate_name: String,
    pub version: String,
    pub path: String,
    pub indexed_at: String,
    pub match_line: Option<u32>,
    pub snippet: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SourceReadRequest {
    pub crate_name: String,
    pub version: Option<String>,
    pub path: String,
    pub start_line: Option<u32>,
    pub end_line: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SourceContextRequest {
    pub crate_name: String,
    pub version: Option<String>,
    pub path: String,
    pub line: Option<u32>,
    pub symbol_name: Option<String>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct SourceReadResponse {
    pub crate_name: String,
    pub version: String,
    pub path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub total_lines: u32,
    pub content: String,
    pub confidence: String,
    pub confidence_assessment: ConfidenceAssessment,
    pub next_best_calls: Vec<String>,
    pub provenance: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct SourceContextResponse {
    pub crate_name: String,
    pub selected_version: String,
    pub latest_version: String,
    pub path: String,
    pub line: u32,
    pub symbol_name: Option<String>,
    pub module_path: String,
    pub imports_in_scope: Vec<String>,
    pub containing_impl: Option<SourceContextImplBlock>,
    pub surrounding_types: Vec<SourceContextTypeContext>,
    pub freshness_check_performed: bool,
    pub freshness_check_result: String,
    pub refresh_enqueued: bool,
    pub refresh_job_id: Option<String>,
    pub freshness: Vec<ResponseFreshnessSource>,
    pub confidence: String,
    pub confidence_assessment: ConfidenceAssessment,
    pub next_best_calls: Vec<String>,
    pub provenance: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct SourceContextImplBlock {
    pub type_name: String,
    pub type_name_display: Option<String>,
    pub trait_name: Option<String>,
    pub trait_name_display: Option<String>,
    pub impl_kind: String,
    pub source_line: i32,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct SourceContextTypeContext {
    pub type_name: String,
    pub kind: String,
    pub source_line: i32,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SymbolSearchRequest {
    pub query: String,
    pub crate_name: Option<String>,
    pub version: Option<String>,
    pub kind: Option<String>,
    pub include_all_versions: Option<bool>,
    pub collapse_by_canonical: Option<bool>,
    pub cursor: Option<String>,
    pub page: Option<u32>,
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct SymbolSearchResponse {
    pub query: String,
    pub crate_name: Option<String>,
    pub version: Option<String>,
    pub kind: Option<String>,
    pub include_all_versions: bool,
    pub collapse_by_canonical: bool,
    pub cursor: Option<String>,
    pub next_cursor: Option<String>,
    pub page: u32,
    pub limit: u32,
    pub total_count: usize,
    pub has_more: bool,
    pub count: usize,
    pub confidence: String,
    #[serde(default = "default_confidence_assessment")]
    pub confidence_assessment: ConfidenceAssessment,
    pub next_best_calls: Vec<String>,
    pub provenance: String,
    pub hits: Vec<SymbolSearchHit>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct SymbolSearchHit {
    pub crate_name: String,
    pub version: String,
    pub source_path: String,
    pub name: String,
    pub kind: String,
    pub signature: Option<String>,
    pub visibility: Option<String>,
    pub start_line: i32,
    pub end_line: i32,
    pub index_source: String,
    pub indexed_at: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DocsSearchRequest {
    pub query: String,
    pub crate_name: Option<String>,
    pub version: Option<String>,
    pub path_prefix: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct DocsSearchResponse {
    pub query: String,
    pub crate_name: Option<String>,
    pub version: Option<String>,
    pub path_prefix: Option<String>,
    pub limit: u32,
    pub count: usize,
    pub confidence: String,
    #[serde(default = "default_confidence_assessment")]
    pub confidence_assessment: ConfidenceAssessment,
    pub next_best_calls: Vec<String>,
    pub provenance: String,
    pub hits: Vec<DocsSearchHit>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct DocsSearchHit {
    pub crate_name: String,
    pub version: String,
    pub path: String,
    pub title: Option<String>,
    pub source_url: Option<String>,
    pub indexed_at: String,
    pub snippet: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct ResponseFreshnessSource {
    pub source: String,
    pub status: String,
    pub checked_at: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ConfidenceLevel {
    High,
    Medium,
    Low,
}

impl ConfidenceLevel {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
pub struct ConfidenceAssessment {
    pub level: ConfidenceLevel,
    pub reason: String,
}

fn default_confidence_assessment() -> ConfidenceAssessment {
    ConfidenceAssessment {
        level: ConfidenceLevel::Low,
        reason: "confidence assessment unavailable in cached legacy response".to_string(),
    }
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateSearchResponse {
    pub query: Option<String>,
    pub category: Option<String>,
    pub keyword: Option<String>,
    pub sort: CrateSearchSort,
    pub limit: u32,
    pub count: usize,
    pub freshness_checks_performed: usize,
    pub refresh_jobs_enqueued: usize,
    pub freshness: Vec<ResponseFreshnessSource>,
    pub confidence: String,
    pub confidence_assessment: ConfidenceAssessment,
    pub next_best_calls: Vec<String>,
    pub provenance: String,
    pub hits: Vec<CrateSearchHit>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateSearchHit {
    pub name: String,
    pub description: Option<String>,
    pub repository_url: Option<String>,
    pub docs_url: Option<String>,
    pub homepage_url: Option<String>,
    pub categories: Vec<String>,
    pub keywords: Vec<String>,
    pub total_downloads: i64,
    pub latest_published_at: Option<String>,
    pub dependent_crates: i64,
    pub rank_score: f64,
    pub match_reasons: Vec<String>,
}

#[derive(Debug, FromRow)]
pub(super) struct CrateSearchRow {
    pub(super) crate_id: i64,
    pub(super) name: String,
    pub(super) description: Option<String>,
    pub(super) repository_url: Option<String>,
    pub(super) docs_url: Option<String>,
    pub(super) homepage_url: Option<String>,
    pub(super) categories: Vec<String>,
    pub(super) keywords: Vec<String>,
    pub(super) total_downloads: i64,
    pub(super) latest_version: Option<String>,
    pub(super) latest_published_at: Option<String>,
    pub(super) dependent_count: i64,
    pub(super) relevance_score: f64,
}

#[derive(Debug, FromRow)]
pub(super) struct SourceSearchRow {
    pub(super) crate_name: String,
    pub(super) version: String,
    pub(super) path: String,
    pub(super) content: String,
    pub(super) indexed_at: String,
}

#[derive(Debug, FromRow)]
pub(super) struct SourceReadRow {
    pub(super) crate_name: String,
    pub(super) version: String,
    pub(super) path: String,
    pub(super) content: String,
}

#[derive(Debug, Clone, FromRow)]
pub(super) struct SourceContextLineLookupRow {
    pub(super) start_line: i32,
}

#[derive(Debug, Clone, FromRow)]
pub(super) struct SourceContextImplLookupRow {
    pub(super) type_name: String,
    pub(super) type_name_display: Option<String>,
    pub(super) trait_name: Option<String>,
    pub(super) trait_name_display: Option<String>,
    pub(super) impl_kind: String,
    pub(super) start_line: i32,
}

#[derive(Debug, Clone, FromRow)]
pub(super) struct SourceContextTypeLookupRow {
    pub(super) type_name: String,
    pub(super) kind: String,
    pub(super) start_line: i32,
}

#[derive(Debug, FromRow)]
pub(super) struct SymbolSearchRow {
    pub(super) _symbol_id: i64,
    pub(super) crate_name: String,
    pub(super) version: String,
    pub(super) source_path: String,
    pub(super) name: String,
    pub(super) kind: String,
    pub(super) signature: Option<String>,
    pub(super) visibility: Option<String>,
    pub(super) start_line: i32,
    pub(super) end_line: i32,
    pub(super) index_source: String,
    pub(super) indexed_at: String,
}

#[derive(Debug, FromRow)]
pub(super) struct DocsSearchRow {
    pub(super) crate_name: String,
    pub(super) version: String,
    pub(super) path: String,
    pub(super) title: Option<String>,
    pub(super) source_url: Option<String>,
    pub(super) indexed_at: String,
    pub(super) content: String,
}

#[derive(Debug, FromRow)]
pub(super) struct DocsSyncCandidateRow {
    pub(super) crate_name: String,
    pub(super) version: String,
    pub(super) crate_version_id: i64,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct IndexSyncCratesRequest {
    pub query: Option<String>,
    pub page: Option<u32>,
    pub per_page: Option<u32>,
    pub include_dependencies: Option<bool>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct IndexSyncCratesResponse {
    pub query: String,
    pub page: u32,
    pub per_page: u32,
    pub total_candidates: u64,
    pub synced_crates: usize,
    pub synced_versions: usize,
    pub synced_dependencies: usize,
    pub selected_versions: Vec<String>,
    pub errors: Vec<String>,
    pub freshness: Vec<ResponseFreshnessSource>,
    pub provenance: String,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum IndexRefreshScope {
    Crate,
    All,
    Security,
    Docs,
    LocalCache,
    RustdocJson,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct IndexRefreshRequest {
    pub scope: Option<IndexRefreshScope>,
    pub crate_name: Option<String>,
    pub query: Option<String>,
    pub page: Option<u32>,
    pub per_page: Option<u32>,
    pub include_dependencies: Option<bool>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct IndexRefreshResponse {
    pub job_id: String,
    pub scope: IndexRefreshScope,
    pub accepted: bool,
    pub status: String,
    pub message: String,
    pub estimated_seconds: Option<u32>,
    pub estimated_seconds_remaining: Option<u32>,
    pub started_at_epoch_ms: u128,
    pub finished_at_epoch_ms: Option<u128>,
    pub result: Option<IndexRefreshResult>,
    pub freshness: Vec<ResponseFreshnessSource>,
    pub provenance: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct IndexRefreshResult {
    pub synced_crates: usize,
    pub synced_versions: usize,
    pub synced_dependencies: usize,
    pub selected_versions: Vec<String>,
    pub errors: Vec<String>,
    /// Rustdoc-specific: number of type rows written (omitted for non-rustdoc
    /// scopes).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub synced_types: Option<usize>,
    /// Rustdoc-specific: number of impl rows written (omitted for non-rustdoc
    /// scopes).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub synced_impls: Option<usize>,
    /// Rustdoc-specific: number of trait rows written (omitted for non-rustdoc
    /// scopes).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub synced_traits: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct IndexStatusRequest {}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct IndexStatusResponse {
    pub freshness: IndexFreshness,
    pub coverage: IndexCoverage,
    pub operational_metrics: IndexOperationalMetrics,
    pub queue: IndexQueue,
    pub retry_distribution: IndexRetryDistribution,
    pub failures_by_scope: Vec<IndexFailureByScope>,
    pub last_errors: Vec<String>,
    pub provenance: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct IndexOperationalMetrics {
    pub window: String,
    pub query_count: i64,
    pub average_latency_ms: Option<f64>,
    pub error_rate: Option<f64>,
    pub cache_hit_rate: Option<f64>,
    pub index_lag_seconds: Option<i64>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct IndexFreshness {
    pub crates_updated_at: Option<String>,
    pub source_indexed_at: Option<String>,
    pub symbols_indexed_at: Option<String>,
    pub docs_indexed_at: Option<String>,
    pub advisories_updated_at: Option<String>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct IndexCoverage {
    pub crates: i64,
    pub crate_versions: i64,
    pub dependency_edges: i64,
    pub advisory_matches: i64,
    pub source_files: i64,
    pub symbols: i64,
    pub docs_pages: i64,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct IndexQueue {
    pub pending_jobs: i64,
    pub delayed_jobs: i64,
    pub retrying_jobs: i64,
    pub running_jobs: i64,
    pub failed_jobs: i64,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct IndexRetryDistribution {
    pub inflight_attempt_1: i64,
    pub inflight_attempt_2: i64,
    pub inflight_attempt_3_plus: i64,
    pub failed_attempt_1: i64,
    pub failed_attempt_2: i64,
    pub failed_attempt_3_plus: i64,
}

#[derive(Debug, Serialize, schemars::JsonSchema, FromRow)]
pub struct IndexFailureByScope {
    pub scope: String,
    pub failed_jobs: i64,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CrateIntelRequest {
    pub crate_name: String,
    pub version: Option<String>,
    pub versions_limit: Option<u32>,
    pub dependents_limit: Option<u32>,
    pub readme_max_chars: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CrateVersionsRequest {
    pub crate_name: String,
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CrateFeaturesRequest {
    pub crate_name: String,
    pub version: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CrateApiDiffRequest {
    pub crate_name: String,
    pub from_version: String,
    pub to_version: String,
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CrateLicenseCheckRequest {
    pub crate_name: String,
    pub version: Option<String>,
    pub allow_licenses: Option<Vec<String>>,
    pub deny_licenses: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CrateAlternativesRequest {
    pub crate_name: String,
    pub version: Option<String>,
    pub limit: Option<u32>,
    pub allow_licenses: Option<Vec<String>>,
    pub deny_licenses: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CrateHotspotsRequest {
    pub crate_name: String,
    pub version: Option<String>,
    pub path_glob: Option<String>,
    pub include_unsafe: Option<bool>,
    pub include_concurrency: Option<bool>,
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CrateApiRequest {
    pub crate_name: String,
    pub version: Option<String>,
    pub path_glob: Option<String>,
    pub kinds: Option<Vec<String>>,
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CrateTypeInfoRequest {
    pub crate_name: String,
    pub type_name: String,
    pub version: Option<String>,
    pub include_methods: Option<bool>,
    pub include_trait_impls: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CrateTraitImplsRequest {
    pub crate_name: String,
    pub version: Option<String>,
    pub trait_name: Option<String>,
    pub type_name: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CrateCompareRequest {
    pub left_crate: String,
    pub right_crate: String,
    pub left_version: Option<String>,
    pub right_version: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CrateCompatibilityRequest {
    pub left_crate: String,
    pub left_version: Option<String>,
    pub right_crate: String,
    pub right_version: Option<String>,
    pub check_features: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CrateCompatibilityMatrixRequest {
    pub left_crate: String,
    pub right_crate: String,
    pub left_versions: Option<Vec<String>>,
    pub right_versions: Option<Vec<String>>,
    pub check_features: Option<bool>,
    pub version_limit: Option<u32>,
    pub max_pairs: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CrateMigrationPathRequest {
    pub crate_name: String,
    pub from_version: String,
    pub to_version: String,
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DependencyAuditRequest {
    pub cargo_toml_path: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DependencyResolveRequest {
    pub dependencies: Option<Vec<DependencyResolveInputDependency>>,
    pub cargo_toml_path: Option<String>,
    pub additions: Option<Vec<DependencyResolveInputDependency>>,
    pub check_features: Option<bool>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct DependencyResolveInputDependency {
    pub name: String,
    pub version_req: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CrateUsagePatternsRequest {
    pub crate_name: String,
    pub symbol_name: String,
    pub version: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CrateReExportsRequest {
    pub crate_name: String,
    pub version: Option<String>,
    pub path_prefix: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CrateErrorTypesRequest {
    pub crate_name: String,
    pub version: Option<String>,
    pub type_name: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DependencyFeatureImpactRequest {
    pub crate_name: String,
    pub version: Option<String>,
    pub features: Vec<String>,
    pub heavy_threshold: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CrateDeriveMacrosRequest {
    pub crate_name: String,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LicensePolicyResult {
    Allowed,
    Denied,
    Unknown,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
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
    pub next_best_calls: Vec<String>,
    pub provenance: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateAlternativesResponse {
    pub crate_name: String,
    pub selected_version: String,
    pub latest_version: String,
    pub limit: u32,
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
    pub next_best_calls: Vec<String>,
    pub provenance: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
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

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateHotspotsResponse {
    pub crate_name: String,
    pub selected_version: String,
    pub latest_version: String,
    pub path_glob: Option<String>,
    pub include_unsafe: bool,
    pub include_concurrency: bool,
    pub limit: u32,
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
    pub next_best_calls: Vec<String>,
    pub provenance: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateHotspotHit {
    pub path: String,
    pub line: u32,
    pub kind: HotspotKind,
    pub pattern: String,
    pub severity: HotspotSeverity,
    pub snippet: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateApiResponse {
    pub crate_name: String,
    pub selected_version: String,
    pub latest_version: String,
    pub path_glob: Option<String>,
    pub kinds: Vec<String>,
    pub limit: u32,
    pub count: usize,
    pub symbols: Vec<CrateApiSymbol>,
    pub freshness_check_performed: bool,
    pub freshness_check_result: String,
    pub refresh_enqueued: bool,
    pub refresh_job_id: Option<String>,
    pub freshness: Vec<ResponseFreshnessSource>,
    pub confidence: String,
    pub confidence_assessment: ConfidenceAssessment,
    pub next_best_calls: Vec<String>,
    pub provenance: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
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

#[derive(Debug, Serialize, schemars::JsonSchema)]
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
    pub conversions: Vec<CrateTypeConversion>,
    pub freshness_check_performed: bool,
    pub freshness_check_result: String,
    pub refresh_enqueued: bool,
    pub refresh_job_id: Option<String>,
    pub freshness: Vec<ResponseFreshnessSource>,
    pub confidence: String,
    pub confidence_assessment: ConfidenceAssessment,
    pub next_best_calls: Vec<String>,
    pub provenance: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateTypeDefinition {
    pub type_name: String,
    pub kind: String,
    pub visibility: Option<String>,
    pub generic_params: Vec<String>,
    pub fields: Vec<CrateTypeField>,
    pub variants: Vec<CrateTypeVariant>,
    pub source_path: String,
    pub start_line: i32,
    pub end_line: i32,
    pub index_source: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateTypeField {
    pub name: Option<String>,
    pub field_type: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateTypeVariant {
    pub name: String,
    pub fields: Vec<CrateTypeField>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateImplMethod {
    pub name: String,
    pub signature: Option<String>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateTraitImpl {
    pub trait_name: Option<String>,
    pub trait_name_display: Option<String>,
    pub impl_kind: String,
    pub methods: Vec<CrateImplMethod>,
    pub source_path: String,
    pub start_line: i32,
    pub end_line: i32,
    pub index_source: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateTypeConversion {
    pub trait_name: String,
    pub source_type: Option<String>,
    pub target_type: Option<String>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateTraitImplsResponse {
    pub crate_name: String,
    pub selected_version: String,
    pub latest_version: String,
    pub trait_name: Option<String>,
    pub type_name: Option<String>,
    pub limit: u32,
    pub count: usize,
    pub impls: Vec<CrateTraitImplRelation>,
    pub freshness_check_performed: bool,
    pub freshness_check_result: String,
    pub refresh_enqueued: bool,
    pub refresh_job_id: Option<String>,
    pub freshness: Vec<ResponseFreshnessSource>,
    pub confidence: String,
    pub confidence_assessment: ConfidenceAssessment,
    pub next_best_calls: Vec<String>,
    pub provenance: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateTraitImplRelation {
    pub type_name: String,
    pub type_name_display: Option<String>,
    pub trait_name: Option<String>,
    pub trait_name_display: Option<String>,
    pub impl_kind: String,
    pub methods: Vec<CrateImplMethod>,
    pub source_path: String,
    pub start_line: i32,
    pub end_line: i32,
    pub index_source: String,
    pub blanket_impl: bool,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
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
    pub next_best_calls: Vec<String>,
    pub provenance: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
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
    pub next_best_calls: Vec<String>,
    pub provenance: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateCompatibilityMatrixResponse {
    pub left_crate: String,
    pub right_crate: String,
    pub check_features: bool,
    pub pairs_tested: usize,
    pub compatible_pairs: Vec<CrateCompatibilityMatrixEntry>,
    pub incompatible_pairs: Vec<CrateCompatibilityMatrixEntry>,
    pub confidence: String,
    pub confidence_assessment: ConfidenceAssessment,
    pub next_best_calls: Vec<String>,
    pub provenance: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateCompatibilityMatrixEntry {
    pub left_version: String,
    pub right_version: String,
    pub resolvable: bool,
    pub conflict_messages: Vec<String>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateMigrationPathResponse {
    pub crate_name: String,
    pub from_version: String,
    pub to_version: String,
    pub breaking_changes_detected: bool,
    pub added_count: usize,
    pub removed_count: usize,
    pub changed_count: usize,
    pub migration_actions: Vec<CrateMigrationAction>,
    pub confidence: String,
    pub confidence_assessment: ConfidenceAssessment,
    pub next_best_calls: Vec<String>,
    pub provenance: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateMigrationAction {
    pub action: String,
    pub rationale: String,
    pub affected_symbol: String,
    pub kind: String,
    pub from_signature: Option<String>,
    pub to_signature: Option<String>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
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

#[derive(
    Debug, Clone, Copy, Deserialize, Serialize, schemars::JsonSchema, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(rename_all = "snake_case")]
pub enum DependencyAuditIssueCategory {
    Yanked,
    Advisory,
    Outdated,
    MsrvConflict,
    Unresolved,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DependencyAuditSeverity {
    Low,
    Medium,
    High,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct DependencyAuditResponse {
    pub cargo_toml_path: String,
    pub package_name: Option<String>,
    pub package_rust_version: Option<String>,
    pub dependency_count: usize,
    pub issue_count: usize,
    pub dependencies: Vec<DependencyAuditDependency>,
    pub issues: Vec<DependencyAuditIssue>,
    pub freshness: Vec<ResponseFreshnessSource>,
    pub confidence: String,
    pub confidence_assessment: ConfidenceAssessment,
    pub next_best_calls: Vec<String>,
    pub provenance: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct DependencyAuditDependency {
    pub dependency_name: String,
    pub requirement: Option<String>,
    pub selected_version: Option<String>,
    pub latest_version: Option<String>,
    pub selected_rust_version: Option<String>,
    pub yanked: bool,
    pub advisory_count: i64,
    pub status_markers: Vec<String>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct DependencyAuditIssue {
    pub dependency_name: String,
    pub category: DependencyAuditIssueCategory,
    pub severity: DependencyAuditSeverity,
    pub message: String,
    pub selected_version: Option<String>,
    pub latest_version: Option<String>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct DependencyResolveResponse {
    pub input_dependencies: Vec<DependencyResolveResolvedDependency>,
    pub resolvable: bool,
    pub resolved_versions: Vec<DependencyResolveResolvedVersion>,
    pub conflicts: Vec<DependencyResolveConflict>,
    pub check_features: bool,
    pub feature_unification_summary: Option<DependencyResolveFeatureSummary>,
    pub confidence: String,
    pub confidence_assessment: ConfidenceAssessment,
    pub next_best_calls: Vec<String>,
    pub provenance: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct DependencyResolveResolvedDependency {
    pub name: String,
    pub version_req: Option<String>,
    pub source: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct DependencyResolveResolvedVersion {
    pub name: String,
    pub version: String,
    pub yanked: bool,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct DependencyResolveConflict {
    pub dependency_name: String,
    pub requirement: Option<String>,
    pub message: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct DependencyResolveFeatureSummary {
    pub dependency_edges_evaluated: usize,
    pub unique_optional_dependencies: usize,
    pub unique_feature_flags_referenced: usize,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateUsagePatternsResponse {
    pub crate_name: String,
    pub selected_version: String,
    pub latest_version: String,
    pub symbol_name: String,
    pub limit: u32,
    pub count: usize,
    pub patterns: Vec<CrateUsagePattern>,
    pub freshness_check_performed: bool,
    pub freshness_check_result: String,
    pub refresh_enqueued: bool,
    pub refresh_job_id: Option<String>,
    pub freshness: Vec<ResponseFreshnessSource>,
    pub confidence: String,
    pub confidence_assessment: ConfidenceAssessment,
    pub next_best_calls: Vec<String>,
    pub provenance: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateUsagePattern {
    pub dependent_crate: String,
    pub dependent_version: String,
    pub dependent_downloads: i64,
    pub path: String,
    pub line_start: u32,
    pub line_end: u32,
    pub snippet: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateReExportsResponse {
    pub crate_name: String,
    pub selected_version: String,
    pub latest_version: String,
    pub path_prefix: Option<String>,
    pub limit: u32,
    pub count: usize,
    pub re_exports: Vec<CrateReExportEntry>,
    pub freshness_check_performed: bool,
    pub freshness_check_result: String,
    pub refresh_enqueued: bool,
    pub refresh_job_id: Option<String>,
    pub freshness: Vec<ResponseFreshnessSource>,
    pub confidence: String,
    pub confidence_assessment: ConfidenceAssessment,
    pub next_best_calls: Vec<String>,
    pub provenance: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateReExportEntry {
    pub canonical_path: String,
    pub original_definition_path: String,
    pub kind: String,
    pub visibility: String,
    pub shortest_public_path: bool,
    pub source_path: String,
    pub source_line: u32,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateErrorTypesResponse {
    pub crate_name: String,
    pub selected_version: String,
    pub latest_version: String,
    pub type_name: Option<String>,
    pub limit: u32,
    pub count: usize,
    pub error_types: Vec<CrateErrorTypeEntry>,
    pub freshness_check_performed: bool,
    pub freshness_check_result: String,
    pub refresh_enqueued: bool,
    pub refresh_job_id: Option<String>,
    pub freshness: Vec<ResponseFreshnessSource>,
    pub confidence: String,
    pub confidence_assessment: ConfidenceAssessment,
    pub next_best_calls: Vec<String>,
    pub provenance: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
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

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct DependencyFeatureImpactResponse {
    pub crate_name: String,
    pub selected_version: String,
    pub latest_version: String,
    pub features: Vec<String>,
    pub heavy_threshold: u32,
    pub baseline_dependency_count: usize,
    pub combined_dependency_count: usize,
    pub per_feature: Vec<DependencyFeatureImpactEntry>,
    pub heavy_features: Vec<String>,
    pub freshness_check_performed: bool,
    pub freshness_check_result: String,
    pub refresh_enqueued: bool,
    pub refresh_job_id: Option<String>,
    pub freshness: Vec<ResponseFreshnessSource>,
    pub confidence: String,
    pub confidence_assessment: ConfidenceAssessment,
    pub next_best_calls: Vec<String>,
    pub provenance: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct DependencyFeatureImpactEntry {
    pub feature: String,
    pub additional_dependency_count: usize,
    pub additional_dependencies: Vec<String>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
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
    pub next_best_calls: Vec<String>,
    pub provenance: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateDeriveMacroEntry {
    pub name: String,
    pub accepted_attributes: Vec<String>,
    pub usage_pattern: String,
    pub source_path: String,
    pub source_line: i32,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateAttributeMacroEntry {
    pub name: String,
    pub usage_pattern: String,
    pub signature_pattern: Option<String>,
    pub source_path: String,
    pub source_line: i32,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateFunctionLikeMacroEntry {
    pub name: String,
    pub usage_pattern: String,
    pub signature_pattern: Option<String>,
    pub source_path: String,
    pub source_line: i32,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
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
    pub freshness_check_performed: bool,
    pub freshness_check_result: String,
    pub refresh_enqueued: bool,
    pub refresh_job_id: Option<String>,
    pub freshness: Vec<ResponseFreshnessSource>,
    pub confidence: String,
    pub confidence_assessment: ConfidenceAssessment,
    pub next_best_calls: Vec<String>,
    pub provenance: String,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
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

#[derive(Debug, Serialize, schemars::JsonSchema)]
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
    pub next_best_calls: Vec<String>,
    pub provenance: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateFeatureFlag {
    pub name: String,
    pub is_default: bool,
    pub enables_features: Vec<String>,
    pub enables_dependencies: Vec<String>,
    pub transitive_enables: Vec<String>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateVersionsResponse {
    pub crate_name: String,
    pub latest_rust_version: Option<String>,
    pub count: usize,
    pub versions: Vec<CrateVersionTimelineItem>,
    pub freshness_check_performed: bool,
    pub freshness_check_result: String,
    pub refresh_enqueued: bool,
    pub refresh_job_id: Option<String>,
    pub freshness: Vec<ResponseFreshnessSource>,
    pub confidence: String,
    pub confidence_assessment: ConfidenceAssessment,
    pub next_best_calls: Vec<String>,
    pub provenance: String,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CrateGraphDirection {
    Dependencies,
    Dependents,
    Both,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CrateGraphRequest {
    pub crate_name: String,
    pub version: Option<String>,
    pub direction: Option<CrateGraphDirection>,
    pub depth: Option<u32>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
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
    pub next_best_calls: Vec<String>,
    pub provenance: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateGraphNode {
    pub crate_name: String,
    pub latest_version: Option<String>,
    pub min_distance: u32,
    pub role: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
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

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateVersionTimelineItem {
    pub version: String,
    pub rust_version: Option<String>,
    pub published_at: Option<String>,
    pub yanked: bool,
    pub downloads: i64,
    pub advisory_count: i64,
    pub release_age_days: Option<i64>,
    pub is_latest: bool,
    pub adoption_signal: String,
    pub markers: Vec<String>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateIntelResponse {
    pub crate_name: String,
    pub selected_version: String,
    pub selected_rust_version: Option<String>,
    pub selected_version_published_at: Option<String>,
    pub latest_version: String,
    pub latest_rust_version: Option<String>,
    pub total_downloads: i64,
    pub last_updated_at: Option<String>,
    pub description: Option<String>,
    pub repository_url: Option<String>,
    pub docs_url: Option<String>,
    pub homepage_url: Option<String>,
    pub categories: Vec<String>,
    pub keywords: Vec<String>,
    pub readme: Option<String>,
    pub readme_truncated: bool,
    pub version_history: Vec<CrateIntelVersion>,
    pub dependencies: Vec<CrateIntelDependency>,
    pub dependents: Vec<CrateIntelDependent>,
    pub dependent_crate_count: i64,
    pub advisories: Vec<CrateIntelAdvisory>,
    pub freshness_check_performed: bool,
    pub freshness_check_result: String,
    pub refresh_enqueued: bool,
    pub refresh_job_id: Option<String>,
    pub freshness: Vec<ResponseFreshnessSource>,
    pub confidence: String,
    pub confidence_assessment: ConfidenceAssessment,
    pub next_best_calls: Vec<String>,
    pub provenance: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateIntelVersion {
    pub version: String,
    pub rust_version: Option<String>,
    pub published_at: Option<String>,
    pub yanked: bool,
    pub downloads: i64,
    pub has_advisory: bool,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateIntelDependency {
    pub crate_name: String,
    pub requirement: String,
    pub dependency_kind: String,
    pub optional: bool,
    pub features: Vec<String>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateIntelDependent {
    pub crate_name: String,
    pub latest_version: Option<String>,
    pub total_downloads: i64,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateIntelAdvisory {
    pub advisory_id: String,
    pub title: String,
    pub severity: Option<String>,
    pub url: Option<String>,
    pub affected_range: String,
    pub fixed_versions: Vec<String>,
    pub source: String,
}

#[derive(Debug, FromRow)]
pub(super) struct CrateCoreRow {
    pub(super) id: i64,
    pub(super) name: String,
    pub(super) description: Option<String>,
    pub(super) repository_url: Option<String>,
    pub(super) docs_url: Option<String>,
    pub(super) homepage_url: Option<String>,
    pub(super) categories: Vec<String>,
    pub(super) keywords: Vec<String>,
    pub(super) updated_at: Option<String>,
}

#[derive(Debug, Clone, FromRow)]
pub(super) struct CrateVersionSelectionRow {
    pub(super) id: i64,
    pub(super) version: String,
    pub(super) rust_version: Option<String>,
    pub(super) published_at: Option<String>,
    pub(super) readme: Option<String>,
}

#[derive(Debug, FromRow)]
pub(super) struct CrateVersionHistoryRow {
    pub(super) version: String,
    pub(super) rust_version: Option<String>,
    pub(super) published_at: Option<String>,
    pub(super) yanked: bool,
    pub(super) total_downloads: i64,
    pub(super) has_advisory: bool,
}

#[derive(Debug, FromRow)]
pub(super) struct CrateDependencyRow {
    pub(super) dependency_name: String,
    pub(super) requirement: String,
    pub(super) dependency_kind: String,
    pub(super) optional: bool,
    pub(super) features: Value,
}

#[derive(Debug, FromRow)]
pub(super) struct CrateDependentRow {
    pub(super) crate_name: String,
    pub(super) latest_version: Option<String>,
    pub(super) total_downloads: i64,
}

#[derive(Debug, FromRow)]
pub(super) struct CrateAdvisoryRow {
    pub(super) advisory_id: String,
    pub(super) title: String,
    pub(super) severity: Option<String>,
    pub(super) url: Option<String>,
    pub(super) affected_range: String,
    pub(super) fixed_versions: Value,
    pub(super) source: String,
}

#[derive(Debug, FromRow)]
pub(super) struct CrateVersionTimelineRow {
    pub(super) version: String,
    pub(super) rust_version: Option<String>,
    pub(super) published_at: Option<String>,
    pub(super) yanked: bool,
    pub(super) total_downloads: i64,
    pub(super) advisory_count: i64,
    pub(super) release_age_days: Option<i64>,
}

#[derive(Debug, FromRow)]
pub(super) struct AlternativesCandidateRow {
    pub(super) crate_name: String,
    pub(super) description: Option<String>,
    pub(super) categories: Vec<String>,
    pub(super) keywords: Vec<String>,
    pub(super) latest_version: Option<String>,
    pub(super) total_downloads: i64,
    pub(super) yanked: bool,
    pub(super) advisory_count: i64,
    pub(super) license_expression: Option<String>,
    pub(super) dependent_count: i64,
    pub(super) name_similarity: f64,
}

#[derive(Debug, FromRow)]
pub(super) struct HotspotSourceFileRow {
    pub(super) path: String,
    pub(super) content: String,
}

#[derive(Debug, FromRow)]
pub(super) struct ApiSurfaceRow {
    pub(super) name: String,
    pub(super) kind: String,
    pub(super) signature: Option<String>,
    pub(super) visibility: Option<String>,
    pub(super) source_path: String,
    pub(super) start_line: i32,
    pub(super) end_line: i32,
    pub(super) index_source: String,
}

#[derive(Debug, Clone, FromRow)]
pub(super) struct CrateTypeInfoRow {
    pub(super) type_name: String,
    pub(super) kind: String,
    pub(super) visibility: Option<String>,
    pub(super) generic_params: Value,
    pub(super) fields: Value,
    pub(super) variants: Value,
    pub(super) source_path: String,
    pub(super) start_line: i32,
    pub(super) end_line: i32,
    pub(super) index_source: String,
}

#[derive(Debug, Clone, FromRow)]
pub(super) struct CrateImplLookupRow {
    pub(super) type_name: String,
    pub(super) type_name_display: Option<String>,
    pub(super) trait_name: Option<String>,
    pub(super) trait_name_display: Option<String>,
    pub(super) impl_kind: String,
    pub(super) methods: Value,
    pub(super) source_path: String,
    pub(super) start_line: i32,
    pub(super) end_line: i32,
    pub(super) index_source: String,
}

#[derive(Debug, Clone, FromRow)]
pub(super) struct CrateVersionLicenseRow {
    pub(super) version: String,
    pub(super) license_expression: Option<String>,
}

#[derive(Debug, Clone, FromRow)]
pub(super) struct CrateCompareVersionRow {
    pub(super) id: i64,
    pub(super) version: String,
    pub(super) rust_version: Option<String>,
    pub(super) published_at: Option<String>,
    pub(super) yanked: bool,
    pub(super) total_downloads: i64,
    pub(super) license_expression: Option<String>,
}

#[derive(Debug, Clone, FromRow)]
pub(super) struct CrateCompareCountsRow {
    pub(super) dependency_count: i64,
    pub(super) feature_count: i64,
    pub(super) advisory_count: i64,
    pub(super) dependent_count: i64,
}

#[derive(Debug, Clone, FromRow)]
pub(super) struct DependencyAuditCrateRow {
    pub(super) id: i64,
    pub(super) name: String,
}

#[derive(Debug, Clone, FromRow)]
pub(super) struct DependencyAuditVersionRow {
    pub(super) id: i64,
    pub(super) version: String,
    pub(super) rust_version: Option<String>,
    pub(super) yanked: bool,
}

#[derive(Debug, Clone, FromRow)]
pub(super) struct DependencyResolveCrateRow {
    pub(super) id: i64,
    pub(super) name: String,
}

#[derive(Debug, Clone, FromRow)]
pub(super) struct DependencyResolveVersionRow {
    pub(super) id: i64,
    pub(super) version: String,
    pub(super) yanked: bool,
}

#[derive(Debug, Clone, FromRow)]
pub(super) struct DependencyResolveEdgeRow {
    pub(super) to_crate_name: String,
    pub(super) requirement: String,
    pub(super) optional: bool,
    pub(super) features: Value,
}

#[derive(Debug, Clone, FromRow)]
pub(super) struct CrateUsageSourceRow {
    pub(super) dependent_crate: String,
    pub(super) dependent_version: String,
    pub(super) dependent_downloads: i64,
    pub(super) path: String,
    pub(super) content: String,
}

#[derive(Debug, Clone, FromRow)]
pub(super) struct ReExportSourceRow {
    pub(super) path: String,
    pub(super) content: String,
}

#[derive(Debug, Clone, FromRow)]
pub(super) struct ReExportSymbolKindRow {
    pub(super) kind: String,
    pub(super) visibility: Option<String>,
}

#[derive(Debug, Clone, FromRow)]
pub(super) struct ErrorTypeTypeRow {
    pub(super) type_name: String,
    pub(super) kind: String,
    pub(super) fields: Value,
    pub(super) variants: Value,
    pub(super) source_path: String,
    pub(super) start_line: i32,
}

#[derive(Debug, Clone, FromRow)]
pub(super) struct ErrorTypeImplRow {
    pub(super) type_name: String,
    pub(super) trait_name: Option<String>,
    pub(super) trait_name_display: Option<String>,
    pub(super) source_path: String,
}

#[derive(Debug, Clone, FromRow)]
pub(super) struct ErrorTypeReturnRow {
    pub(super) name: String,
    pub(super) signature: Option<String>,
}

#[derive(Debug, Clone, FromRow)]
pub(super) struct FeatureImpactFeatureRow {
    pub(super) feature_name: String,
    pub(super) enables: Value,
}

#[derive(Debug, Clone, FromRow)]
pub(super) struct FeatureImpactDependencyRow {
    pub(super) dependency_name: String,
    pub(super) optional: bool,
}

#[derive(Debug, Clone, FromRow)]
pub(super) struct DeriveMacroSourceRow {
    pub(super) path: String,
    pub(super) content: String,
}

#[derive(Debug, FromRow)]
pub(super) struct CrateFeatureRow {
    pub(super) feature_name: String,
    pub(super) enables: Value,
}

#[derive(Debug, Clone, FromRow)]
pub(super) struct ApiDiffSymbolRow {
    pub(super) name: String,
    pub(super) kind: String,
    pub(super) signature: Option<String>,
    pub(super) visibility: Option<String>,
}

#[derive(Debug, FromRow)]
pub(super) struct GraphLatestVersionRow {
    pub(super) crate_id: i64,
    pub(super) id: i64,
    pub(super) version: String,
}

#[derive(Debug, FromRow)]
pub(super) struct GraphDependencyTraversalRow {
    pub(super) from_crate_name: String,
    pub(super) from_version: String,
    pub(super) to_crate_id: i64,
    pub(super) to_crate_name: String,
    pub(super) requirement: String,
    pub(super) dependency_kind: String,
    pub(super) optional: bool,
}

#[derive(Debug, FromRow)]
pub(super) struct GraphDependentTraversalRow {
    pub(super) from_crate_id: i64,
    pub(super) from_crate_name: String,
    pub(super) from_version: String,
    pub(super) to_crate_name: String,
    pub(super) requirement: String,
    pub(super) dependency_kind: String,
    pub(super) optional: bool,
}

#[cfg(test)]
mod tests {
    use super::{ConfidenceAssessment, ConfidenceLevel};

    #[test]
    fn confidence_level_serializes_lowercase() {
        let value = serde_json::to_string(&ConfidenceLevel::High).expect("serialize confidence");
        assert_eq!(value, "\"high\"");
    }

    #[test]
    fn confidence_assessment_round_trips_reason() {
        let original = ConfidenceAssessment {
            level: ConfidenceLevel::Medium,
            reason: "symbols missing signatures for 2 entries".to_string(),
        };

        let encoded = serde_json::to_string(&original).expect("serialize confidence assessment");
        let decoded: ConfidenceAssessment =
            serde_json::from_str(&encoded).expect("deserialize confidence assessment");

        assert_eq!(decoded.level.as_str(), "medium");
        assert_eq!(decoded.reason, "symbols missing signatures for 2 entries");
    }
}
