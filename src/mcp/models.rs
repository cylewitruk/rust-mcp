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
    pub next_best_calls: Vec<String>,
    pub provenance: String,
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
pub struct CrateCompareRequest {
    pub left_crate: String,
    pub right_crate: String,
    pub left_version: Option<String>,
    pub right_version: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DependencyAuditRequest {
    pub cargo_toml_path: String,
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
