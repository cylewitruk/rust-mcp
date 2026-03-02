use serde::Deserialize;
use serde_json::Value;
use sqlx::FromRow;
use sqlx::types::Json;

/// Row returned by full-text / fuzzy crate search queries.
#[derive(Debug, FromRow)]
pub struct CrateSearchRow {
    /// Primary key of the matched crate.
    pub crate_id: i64,
    /// Canonical crate name.
    pub name: String,
    /// Optional crate description.
    pub description: Option<String>,
    /// Optional repository URL.
    pub repository_url: Option<String>,
    /// Optional documentation URL.
    pub docs_url: Option<String>,
    /// Optional homepage URL.
    pub homepage_url: Option<String>,
    /// Normalized category slugs associated with the crate.
    pub categories: Vec<String>,
    /// Normalized keywords associated with the crate.
    pub keywords: Vec<String>,
    /// Aggregated download count for the crate.
    pub total_downloads: i64,
    /// Latest known version string for the crate.
    pub latest_version: Option<String>,
    /// Publish timestamp for the latest known version.
    pub latest_published_at: Option<String>,
    /// Count of crates depending on this crate.
    pub dependent_count: i64,
    /// Search relevance score computed by the query.
    pub relevance_score: f64,
}

/// Row for reading indexed source content.
#[derive(Debug, FromRow)]
pub struct SourceReadRow {
    /// Canonical crate name owning the file.
    pub crate_name: String,
    /// Version string selected for the read.
    pub version: String,
    /// Relative path of the source file in the crate.
    pub path: String,
    /// Full source file contents (NULL for rustdoc_json entries).
    pub content: Option<String>,
}

/// Row for searching indexed source content.
#[allow(missing_docs)]
#[derive(Debug, Clone, FromRow)]
pub struct SourceSearchRow {
    pub crate_name: String,
    pub version: String,
    pub path: String,
    pub content: Option<String>,
    pub indexed_at: String,
}

/// Row for reading indexed docs pages for `docs.search`.
#[derive(Debug, Clone, FromRow)]
pub struct DocsSearchRow {
    /// Canonical crate name owning the docs page.
    pub crate_name: String,
    /// Semver version selected for the docs page.
    pub version: String,
    /// Relative docs path.
    pub path: String,
    /// Optional HTML title extracted from the docs page.
    pub title: Option<String>,
    /// Optional canonical docs.rs source URL.
    pub source_url: Option<String>,
    /// Docs indexing timestamp as text.
    pub indexed_at: String,
    /// Normalized searchable page content.
    pub content: String,
}

/// Core crate metadata row shared by multiple tools.
#[derive(Debug, FromRow)]
pub struct CrateCoreRow {
    /// Primary key of the crate.
    pub id: i64,
    /// Canonical crate name.
    pub name: String,
    /// Optional crate description.
    pub description: Option<String>,
    /// Optional repository URL.
    pub repository_url: Option<String>,
    /// Optional documentation URL.
    pub docs_url: Option<String>,
    /// Optional homepage URL.
    pub homepage_url: Option<String>,
    /// Normalized category slugs associated with the crate.
    pub categories: Vec<String>,
    /// Normalized keywords associated with the crate.
    pub keywords: Vec<String>,
    /// Last updated timestamp of the crate metadata row.
    pub updated_at: Option<String>,
}

/// Version selection row used when resolving the target version for a crate.
#[derive(Debug, Clone, FromRow)]
pub struct CrateVersionSelectionRow {
    /// Primary key of the crate version row.
    pub id: i64,
    /// Semver version string.
    pub version: String,
    /// Optional declared Rust version (MSRV-like metadata).
    pub rust_version: Option<String>,
    /// Publish timestamp for this crate version.
    pub published_at: Option<String>,
    /// Optional README content stored for the selected version.
    pub readme: Option<String>,
}

/// Version timeline row used by `crate.intel`.
#[derive(Debug, Clone, FromRow)]
pub struct CrateVersionHistoryRow {
    /// Semver version string.
    pub version: String,
    /// Optional declared Rust version.
    pub rust_version: Option<String>,
    /// Publish timestamp for this version.
    pub published_at: Option<String>,
    /// Whether this version is yanked.
    pub yanked: bool,
    /// Download count for this version.
    pub total_downloads: i64,
    /// Whether at least one advisory matches this version.
    pub has_advisory: bool,
}

/// Dependency edge row used by `crate.intel`.
#[derive(Debug, Clone, FromRow)]
pub struct CrateDependencyRow {
    /// Canonical dependency crate name.
    pub dependency_name: String,
    /// Semver requirement string.
    pub requirement: String,
    /// Dependency kind (`normal`, `dev`, `build`, etc.).
    pub dependency_kind: String,
    /// Whether the dependency is optional.
    pub optional: bool,
    /// JSON array of enabled dependency features.
    pub features: Value,
}

/// Dependent crate row used by `crate.intel`.
#[derive(Debug, Clone, FromRow)]
pub struct CrateDependentRow {
    /// Canonical dependent crate name.
    pub crate_name: String,
    /// Latest known version of the dependent crate.
    pub latest_version: Option<String>,
    /// Download count of the latest dependent version.
    pub total_downloads: i64,
}

/// Advisory row used by `crate.intel`.
#[derive(Debug, Clone, FromRow)]
pub struct CrateAdvisoryRow {
    /// Advisory identifier.
    pub advisory_id: String,
    /// Advisory title.
    pub title: String,
    /// Optional normalized severity label.
    pub severity: Option<String>,
    /// Optional canonical advisory URL.
    pub url: Option<String>,
    /// Rendered affected range summary.
    pub affected_range: String,
    /// JSON array of fixed/patched versions.
    pub fixed_versions: Value,
    /// Advisory source key.
    pub source: String,
}

/// Symbol line lookup row used by `source.context`.
#[derive(Debug, Clone, FromRow)]
pub struct SourceContextLineLookupRow {
    /// 1-based start line of the matched symbol.
    pub start_line: i32,
}

/// Impl context lookup row used by `source.context`.
#[derive(Debug, Clone, FromRow)]
pub struct SourceContextImplLookupRow {
    /// Implemented type name.
    pub type_name: String,
    /// Optional rendered implemented type path.
    pub type_name_display: Option<String>,
    /// Optional trait name for trait impls.
    pub trait_name: Option<String>,
    /// Optional rendered trait display path/signature.
    pub trait_name_display: Option<String>,
    /// Impl kind (`inherent`, `trait`, `derive`, etc.).
    pub impl_kind: String,
    /// 1-based start line for the impl block.
    pub start_line: i32,
}

/// Type context lookup row used by `source.context`.
#[derive(Debug, Clone, FromRow)]
pub struct SourceContextTypeLookupRow {
    /// Type name.
    pub type_name: String,
    /// Type kind (`struct`, `enum`, `trait`, etc.).
    pub kind: String,
    /// 1-based start line for the type definition.
    pub start_line: i32,
}

/// Symbol search row used by `symbol.search`.
#[derive(Debug, Clone, FromRow)]
pub struct SymbolSearchRow {
    /// Symbol row primary key.
    pub _symbol_id: i64,
    /// Canonical crate name owning the symbol.
    pub crate_name: String,
    /// Semver version selected for the symbol row.
    pub version: String,
    /// Relative source path for the symbol.
    pub source_path: String,
    /// Symbol name.
    pub name: String,
    /// Symbol kind (`function`, `struct`, `trait`, etc.).
    pub kind: String,
    /// Optional rendered declaration/signature.
    pub signature: Option<String>,
    /// Optional rendered visibility marker.
    pub visibility: Option<String>,
    /// 1-based source start line.
    pub start_line: i32,
    /// 1-based source end line.
    pub end_line: i32,
    /// Index provenance (`rustdoc_json`, `local_cache`, etc.).
    pub index_source: String,
    /// Symbol indexing timestamp as text.
    pub indexed_at: String,
}

/// JSON entry for generic parameter metadata.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum GenericParamEntry {
    /// Structured representation containing a rendered generic parameter.
    Rendered {
        /// Display-ready generic parameter string.
        rendered: String,
    },
    /// Legacy plain generic parameter string.
    Plain(
        /// Display-ready generic parameter string.
        String,
    ),
}

impl GenericParamEntry {
    /// Returns a display-ready rendering for the generic parameter entry.
    pub fn rendered(&self) -> &str {
        match self {
            Self::Rendered { rendered } => rendered,
            Self::Plain(value) => value,
        }
    }
}

/// JSON entry for impl/trait methods.
#[derive(Debug, Clone, Deserialize)]
pub struct ImplMethodEntry {
    /// Method name.
    pub name: String,
    /// Optional rendered signature string.
    #[serde(default)]
    pub signature: Option<String>,
}

/// JSON entry for trait associated types.
#[derive(Debug, Clone, Deserialize)]
pub struct TraitAssociatedTypeEntry {
    /// Associated type name.
    pub name: String,
    /// Rendered bounds for the associated type.
    #[serde(default)]
    pub bounds: Vec<String>,
    /// Optional default type expression.
    #[serde(default)]
    pub default: Option<String>,
}

/// JSON entry for type fields.
#[derive(Debug, Clone, Deserialize)]
pub struct TypeFieldEntry {
    /// Optional field name (None for tuple fields).
    #[serde(default)]
    pub name: Option<String>,
    /// Rendered field type.
    #[serde(rename = "type")]
    pub field_type: String,
}

/// JSON entry for type variants.
#[derive(Debug, Clone, Deserialize)]
pub struct TypeVariantEntry {
    /// Enum variant name.
    pub name: String,
    /// Variant fields.
    #[serde(default)]
    pub fields: Vec<TypeFieldEntry>,
}

/// Type-definition lookup row used by `crate.type_info`.
#[derive(Debug, Clone, FromRow)]
pub struct CrateTypeInfoRow {
    /// Type name as indexed.
    pub type_name: String,
    /// Type kind (`struct`, `enum`, `union`, `trait`, `type_alias`, etc.).
    pub kind: String,
    /// Optional visibility marker (`public`/`private`).
    pub visibility: Option<String>,
    /// Optional canonical import path.
    pub canonical_path: Option<String>,
    /// Optional original definition path.
    pub definition_path: Option<String>,
    /// Generic parameter metadata JSON payload.
    pub generic_params: Json<Vec<GenericParamEntry>>,
    /// Rendered where-clause predicates.
    pub where_clauses: Json<Vec<String>>,
    /// Type fields metadata.
    pub fields: Json<Vec<TypeFieldEntry>>,
    /// Enum variant metadata.
    pub variants: Json<Vec<TypeVariantEntry>>,
    /// Optional deprecation version marker.
    pub deprecated_since: Option<String>,
    /// Optional deprecation note text.
    pub deprecated_note: Option<String>,
    /// Whether the type is marked `#[non_exhaustive]`.
    pub is_non_exhaustive: bool,
    /// Auto traits implemented by the type.
    pub auto_traits: Json<Vec<String>>,
    /// Source path where the type was indexed from.
    pub source_path: String,
    /// 1-based start line in the source path.
    pub start_line: i32,
    /// 1-based end line in the source path.
    pub end_line: i32,
    /// Index provenance (`rustdoc_json`, `local_cache`, etc.).
    pub index_source: String,
}

/// Impl-block lookup row used by `crate.type_info` and `crate.trait_impls`.
#[derive(Debug, Clone, FromRow)]
pub struct CrateImplLookupRow {
    /// Implemented type name.
    pub type_name: String,
    /// Optional rendered implemented type path.
    pub type_name_display: Option<String>,
    /// Optional trait name for trait impls.
    pub trait_name: Option<String>,
    /// Optional rendered trait display path/signature.
    pub trait_name_display: Option<String>,
    /// Impl kind (`inherent`, `trait`, `derive`, etc.).
    pub impl_kind: String,
    /// Methods captured for the impl block.
    pub methods: Json<Vec<ImplMethodEntry>>,
    /// Whether this is a blanket impl.
    pub is_blanket: bool,
    /// Whether this impl is synthetic (compiler-generated).
    pub is_synthetic: bool,
    /// Whether this is a negative impl.
    pub is_negative: bool,
    /// Optional blanket target type expression.
    pub blanket_type: Option<String>,
    /// Generic parameter metadata for the impl.
    pub generics: Json<Vec<GenericParamEntry>>,
    /// Rendered where-clause predicates for the impl.
    pub where_clauses: Json<Vec<String>>,
    /// Source path where the impl was indexed from.
    pub source_path: String,
    /// 1-based start line in the source path.
    pub start_line: i32,
    /// 1-based end line in the source path.
    pub end_line: i32,
    /// Index provenance (`rustdoc_json`, `local_cache`, etc.).
    pub index_source: String,
}

/// Trait-definition lookup row used by `crate.type_info` and
/// `crate.trait_impls`.
#[derive(Debug, Clone, FromRow)]
pub struct CrateTraitLookupRow {
    /// Trait name.
    pub trait_name: String,
    /// Whether the trait is an auto trait.
    pub is_auto: bool,
    /// Whether the trait is unsafe.
    pub is_unsafe: bool,
    /// Whether the trait is dyn compatible.
    pub is_dyn_compatible: bool,
    /// Rendered supertrait bounds.
    pub supertraits: Json<Vec<String>>,
    /// Required trait methods.
    pub required_methods: Json<Vec<ImplMethodEntry>>,
    /// Provided trait methods.
    pub provided_methods: Json<Vec<ImplMethodEntry>>,
    /// Associated type metadata.
    pub associated_types: Json<Vec<TraitAssociatedTypeEntry>>,
    /// Generic parameter metadata for the trait.
    pub generics: Json<Vec<GenericParamEntry>>,
    /// Index provenance (`rustdoc_json`, `local_cache`, etc.).
    pub index_source: String,
}

/// Crate/version key row used to map cargo registry cache directories.
#[derive(Debug, Clone, FromRow)]
pub struct LocalCacheVersionKeyRow {
    /// `<crate>-<version>` directory name in the cargo registry cache.
    pub cache_dir_name: String,
    /// Canonical crate name.
    pub crate_name: String,
    /// Semver version string.
    pub version: String,
    /// Primary key of the crate version row.
    pub crate_version_id: i64,
}

/// Crate/version row used when selecting rustdoc JSON sync candidates.
#[derive(Debug, Clone, FromRow)]
pub struct RustdocSyncCandidateRow {
    /// Canonical crate name.
    pub crate_name: String,
    /// Semver version string.
    pub version: String,
    /// Primary key of the crate version row.
    pub crate_version_id: i64,
}

/// Aggregated coverage counters for `index.status`.
#[derive(Debug, Clone, FromRow)]
pub struct IndexCoverageCountsRow {
    /// Total crates stored in the index.
    pub crates: i64,
    /// Total crate version rows stored in the index.
    pub crate_versions: i64,
    /// Total dependency edges stored in the index.
    pub dependency_edges: i64,
    /// Total advisory matches stored in the index.
    pub advisory_matches: i64,
    /// Total source files stored in the index.
    pub source_files: i64,
    /// Total symbols stored in the index.
    pub symbols: i64,
    /// Total docs pages stored in the index.
    pub docs_pages: i64,
}

/// Aggregated refresh queue counters for `index.status`.
#[derive(Debug, Clone, FromRow)]
pub struct IndexQueueCountsRow {
    /// Pending jobs ready to run now.
    pub pending_jobs: i64,
    /// Pending jobs scheduled for the future.
    pub delayed_jobs: i64,
    /// Pending jobs that are retries.
    pub retrying_jobs: i64,
    /// Jobs currently marked as running.
    pub running_jobs: i64,
    /// Jobs currently marked as failed.
    pub failed_jobs: i64,
}

/// Retry-attempt distribution for refresh jobs.
#[derive(Debug, Clone, FromRow)]
pub struct RefreshJobRetryDistributionRow {
    /// In-flight jobs with attempt count 1.
    pub inflight_attempt_1: i64,
    /// In-flight jobs with attempt count 2.
    pub inflight_attempt_2: i64,
    /// In-flight jobs with attempt count >= 3.
    pub inflight_attempt_3_plus: i64,
    /// Failed jobs with attempt count 1.
    pub failed_attempt_1: i64,
    /// Failed jobs with attempt count 2.
    pub failed_attempt_2: i64,
    /// Failed jobs with attempt count >= 3.
    pub failed_attempt_3_plus: i64,
}

/// Failure counts grouped by refresh scope.
#[derive(Debug, Clone, FromRow)]
pub struct IndexFailureByScopeRow {
    /// Refresh scope key (`crate`, `all`, `security`, etc.).
    pub scope: String,
    /// Number of failed jobs for this scope.
    pub failed_jobs: i64,
}

/// Freshness timestamps used by `index.status`.
#[derive(Debug, Clone, FromRow)]
pub struct IndexFreshnessRow {
    /// Last crate metadata update time.
    pub crates_updated_at: Option<String>,
    /// Last source indexing time.
    pub source_indexed_at: Option<String>,
    /// Last symbol indexing time.
    pub symbols_indexed_at: Option<String>,
    /// Last docs page indexing time.
    pub docs_indexed_at: Option<String>,
    /// Last advisory match write time.
    pub advisories_updated_at: Option<String>,
}

/// Operational telemetry window aggregates for `index.status`.
#[derive(Debug, Clone, FromRow)]
pub struct IndexOperationalMetricsRow {
    /// Number of tool invocations in the metrics window.
    pub query_count: i64,
    /// Mean invocation latency in milliseconds.
    pub average_latency_ms: Option<f64>,
    /// Fraction of failed invocations in the metrics window.
    pub error_rate: Option<f64>,
    /// Fraction of cache-hit query-cache lookups.
    pub cache_hit_rate: Option<f64>,
    /// Estimated staleness in seconds across indexed datasets.
    pub index_lag_seconds: Option<i64>,
}

/// Refresh-job queue gauge counters.
#[derive(Debug, Clone, FromRow)]
pub struct RefreshJobGaugeCountsRow {
    /// Pending refresh jobs.
    pub pending_jobs: i64,
    /// Running refresh jobs.
    pub running_jobs: i64,
    /// Failed refresh jobs.
    pub failed_jobs: i64,
    /// Pending jobs that are background-priority (discovery / proactive
    /// enrichment; priority ≥ 50). Used to compute the background ratio gauge.
    pub background_pending_jobs: i64,
}

/// Row returned when claiming the next refresh job.
#[derive(Debug, Clone, FromRow)]
pub struct RefreshJobQueueRow {
    /// Refresh job primary key.
    pub id: i64,
    /// Crate name associated with the job.
    pub crate_name: String,
    /// Scope key for the refresh operation.
    pub scope: String,
    /// Whether dependency sync should be included.
    pub include_dependencies: bool,
    /// Serialized payload attached to the job.
    pub payload: Value,
    /// Attempt number after claim increment.
    pub attempts: i32,
}

/// Crate row used by security indexing sweeps.
#[derive(Debug, Clone, FromRow)]
pub struct SecurityCrateRow {
    /// Crate primary key.
    pub id: i64,
    /// Canonical crate name.
    pub name: String,
}

/// Crate version row used by security indexing sweeps.
#[derive(Debug, Clone, FromRow)]
pub struct SecurityVersionRow {
    /// Crate version primary key.
    pub id: i64,
    /// Semver version string.
    pub version: String,
}

/// Insert DTO for upserting a version-specific advisory match.
#[derive(Debug, Clone, Copy)]
pub struct VersionAdvisoryMatchInsert<'a> {
    /// Crate primary key.
    pub crate_id: i64,
    /// Crate version primary key.
    pub version_id: i64,
    /// Advisory identifier.
    pub advisory_id: &'a str,
    /// Optional normalized severity label.
    pub severity: Option<&'a str>,
    /// Advisory title/summary text.
    pub title: &'a str,
    /// Optional canonical advisory URL.
    pub url: Option<&'a str>,
    /// Rendered affected version-range summary.
    pub affected_range: &'a str,
    /// JSON array of fixed/patched versions.
    pub fixed_versions: &'a Value,
    /// Advisory source label.
    pub source: &'a str,
}

/// Insert DTO for writing a crate-level advisory match.
#[derive(Debug, Clone, Copy)]
pub struct CrateLevelAdvisoryMatchInsert<'a> {
    /// Crate primary key.
    pub crate_id: i64,
    /// Advisory identifier.
    pub advisory_id: &'a str,
    /// Optional normalized severity label.
    pub severity: Option<&'a str>,
    /// Advisory title/summary text.
    pub title: &'a str,
    /// Optional canonical advisory URL.
    pub url: Option<&'a str>,
    /// Rendered affected version-range summary.
    pub affected_range: &'a str,
    /// JSON array of fixed/patched versions.
    pub fixed_versions: &'a Value,
    /// Advisory source label.
    pub source: &'a str,
}

/// Insert DTO for a symbol extracted from source or rustdoc metadata.
#[derive(Debug, Clone, Default)]
pub struct IndexedSymbolInsert {
    /// Symbol identifier as shown to users.
    pub name: String,
    /// Symbol kind (`function`, `struct`, `trait`, etc.).
    pub kind: String,
    /// Optional rendered visibility marker.
    pub visibility: Option<String>,
    /// Optional rendered declaration/signature.
    pub signature: Option<String>,
    /// 1-based source start line.
    pub start_line: i32,
    /// 1-based source end line.
    pub end_line: i32,
    /// Optional rustdoc item id when source is rustdoc JSON.
    pub rustdoc_item_id: Option<i32>,
    /// Optional canonical import path.
    pub canonical_path: Option<String>,
    /// Optional original definition path.
    pub definition_path: Option<String>,
    /// Optional deprecation version marker.
    pub deprecated_since: Option<String>,
    /// Optional deprecation note text.
    pub deprecated_note: Option<String>,
    /// Raw markdown documentation from rustdoc `item.docs`.
    pub docs: Option<String>,
    /// Structured attributes from rustdoc `item.attrs`.
    pub attrs: Option<Value>,
}

/// Insert DTO for a type definition extracted from source or rustdoc metadata.
#[derive(Debug, Clone, Default)]
pub struct IndexedTypeInsert {
    /// Type name as indexed.
    pub type_name: String,
    /// Type kind (`struct`, `enum`, `union`, `type_alias`, etc.).
    pub kind: String,
    /// Optional rendered visibility marker.
    pub visibility: Option<String>,
    /// Generic parameter payload.
    pub generic_params: Value,
    /// Field payload.
    pub fields: Value,
    /// Variant payload.
    pub variants: Value,
    /// 1-based source start line.
    pub start_line: i32,
    /// 1-based source end line.
    pub end_line: i32,
    /// Optional rustdoc item id when source is rustdoc JSON.
    pub rustdoc_item_id: Option<i32>,
    /// Optional canonical import path.
    pub canonical_path: Option<String>,
    /// Optional original definition path.
    pub definition_path: Option<String>,
    /// Optional deprecation version marker.
    pub deprecated_since: Option<String>,
    /// Optional deprecation note text.
    pub deprecated_note: Option<String>,
    /// Whether the type is marked non-exhaustive.
    pub is_non_exhaustive: bool,
    /// Auto-trait payload.
    pub auto_traits: Value,
    /// Where-clause payload.
    pub where_clauses: Value,
    /// Raw markdown documentation from rustdoc `item.docs`.
    pub docs: Option<String>,
    /// Structured attributes from rustdoc `item.attrs`.
    pub attrs: Option<Value>,
}

/// Insert DTO for an impl block extracted from source or rustdoc metadata.
#[derive(Debug, Clone, Default)]
pub struct IndexedImplInsert {
    /// Terminal type name the impl applies to.
    pub type_name: String,
    /// Optional rendered display for the implemented type.
    pub type_name_display: Option<String>,
    /// Optional trait name for trait impls.
    pub trait_name: Option<String>,
    /// Optional rendered trait display.
    pub trait_name_display: Option<String>,
    /// Impl kind (`inherent`, `trait`, `derive`, etc.).
    pub impl_kind: String,
    /// Method payload for the impl.
    pub methods: Value,
    /// 1-based source start line.
    pub start_line: i32,
    /// 1-based source end line.
    pub end_line: i32,
    /// Optional rustdoc item id when source is rustdoc JSON.
    pub rustdoc_item_id: Option<i32>,
    /// Whether this impl is blanket.
    pub is_blanket: bool,
    /// Whether this impl is synthetic.
    pub is_synthetic: bool,
    /// Whether this impl is negative.
    pub is_negative: bool,
    /// Optional blanket target type rendering.
    pub blanket_type: Option<String>,
    /// Generic parameter payload for the impl.
    pub generics: Value,
    /// Where-clause payload for the impl.
    pub where_clauses: Value,
    /// Raw markdown documentation from rustdoc `item.docs`.
    pub docs: Option<String>,
}

/// Insert DTO for a trait definition extracted from source or rustdoc metadata.
#[derive(Debug, Clone, Default)]
pub struct IndexedTraitInsert {
    /// Trait name.
    pub trait_name: String,
    /// Whether the trait is auto.
    pub is_auto: bool,
    /// Whether the trait is unsafe.
    pub is_unsafe: bool,
    /// Whether the trait is dyn-compatible.
    pub is_dyn_compatible: bool,
    /// Supertrait payload.
    pub supertraits: Value,
    /// Required method payload.
    pub required_methods: Value,
    /// Provided method payload.
    pub provided_methods: Value,
    /// Associated type payload.
    pub associated_types: Value,
    /// Generic parameter payload.
    pub generics: Value,
    /// Optional rustdoc item id when source is rustdoc JSON.
    pub rustdoc_item_id: Option<i32>,
    /// Raw markdown documentation from rustdoc `item.docs`.
    pub docs: Option<String>,
}

/// Unified extraction batch DTO used by indexers before DB inserts.
#[derive(Debug, Clone, Default)]
pub struct IndexedExtractionBatch {
    /// Extracted symbol inserts.
    pub symbols: Vec<IndexedSymbolInsert>,
    /// Extracted type inserts.
    pub types: Vec<IndexedTypeInsert>,
    /// Extracted impl inserts.
    pub impls: Vec<IndexedImplInsert>,
    /// Extracted trait inserts.
    pub traits: Vec<IndexedTraitInsert>,
}

/// Public symbol row used by `crate.api`.
#[allow(missing_docs)]
#[derive(Debug, Clone, FromRow)]
pub struct ApiSurfaceRow {
    pub name: String,
    pub kind: String,
    pub signature: Option<String>,
    pub visibility: Option<String>,
    pub source_path: String,
    pub start_line: i32,
    pub end_line: i32,
    pub index_source: String,
}

/// Import path lookup row used by `crate.import_path`.
#[allow(missing_docs)]
#[derive(Debug, Clone, FromRow)]
pub struct ImportPathRow {
    pub symbol_name: String,
    pub kind: String,
    pub canonical_path: Option<String>,
    pub definition_path: Option<String>,
    pub source_path: String,
    pub start_line: i32,
    pub end_line: i32,
    pub index_source: String,
}

/// Error-type candidate row used by `crate.error_types`.
#[allow(missing_docs)]
#[derive(Debug, Clone, FromRow)]
pub struct ErrorTypeTypeRow {
    pub type_name: String,
    pub kind: String,
    pub fields: Value,
    pub variants: Value,
    pub source_path: String,
    pub start_line: i32,
}

/// Error impl row used by `crate.error_types`.
#[allow(missing_docs)]
#[derive(Debug, Clone, FromRow)]
pub struct ErrorTypeImplRow {
    pub type_name: String,
    pub trait_name: Option<String>,
    pub trait_name_display: Option<String>,
    pub source_path: String,
}

/// Function signature row used by `crate.error_types`.
#[allow(missing_docs)]
#[derive(Debug, Clone, FromRow)]
pub struct ErrorTypeReturnRow {
    pub name: String,
    pub signature: Option<String>,
}

/// Rustdoc re-export row used by `crate.re_exports`.
#[allow(missing_docs)]
#[derive(Debug, Clone, FromRow)]
pub struct RustdocReExportRow {
    pub canonical_path: String,
    pub original_definition_path: String,
    pub kind: String,
    pub visibility: Option<String>,
    pub source_path: String,
    pub source_line: i32,
}

/// Source content row used by `crate.re_exports` fallback parser.
#[allow(missing_docs)]
#[derive(Debug, Clone, FromRow)]
pub struct ReExportSourceRow {
    pub path: String,
    pub content: Option<String>,
}

/// Symbol metadata row used by `crate.re_exports` fallback parser.
#[allow(missing_docs)]
#[derive(Debug, Clone, FromRow)]
pub struct ReExportSymbolKindRow {
    pub kind: String,
    pub visibility: Option<String>,
}

/// Dependent source row used by `crate.usage_patterns`.
#[allow(missing_docs)]
#[derive(Debug, Clone, FromRow)]
pub struct CrateUsageSourceRow {
    pub dependent_crate: String,
    pub dependent_version: String,
    pub dependent_downloads: i64,
    pub path: String,
    pub content: Option<String>,
}

/// License metadata row used by `crate.license_check`.
#[allow(missing_docs)]
#[derive(Debug, Clone, FromRow)]
pub struct CrateVersionLicenseRow {
    pub version: String,
    pub license_expression: Option<String>,
}

/// Source file row used by `crate.hotspots` and `crate.derive_macros`.
#[allow(missing_docs)]
#[derive(Debug, Clone, FromRow)]
pub struct HotspotSourceFileRow {
    pub path: String,
    pub content: Option<String>,
}

/// Feature row used by `dependency.feature_impact` and `crate.features`.
#[allow(missing_docs)]
#[derive(Debug, Clone, FromRow)]
pub struct FeatureImpactFeatureRow {
    pub feature_name: String,
    pub enables: Value,
}

/// Dependency row used by `dependency.feature_impact`.
#[allow(missing_docs)]
#[derive(Debug, Clone, FromRow)]
pub struct FeatureImpactDependencyRow {
    pub dependency_name: String,
    pub optional: bool,
}

/// Version timeline row used by `crate.versions`.
#[allow(missing_docs)]
#[derive(Debug, Clone, FromRow)]
pub struct CrateVersionTimelineRow {
    pub version: String,
    pub rust_version: Option<String>,
    pub published_at: Option<String>,
    pub yanked: bool,
    pub total_downloads: i64,
    pub advisory_count: i64,
    pub release_age_days: Option<i64>,
}

/// Candidate alternative row used by `crate.alternatives`.
#[allow(missing_docs)]
#[derive(Debug, Clone, FromRow)]
pub struct AlternativesCandidateRow {
    pub crate_name: String,
    pub description: Option<String>,
    pub categories: Vec<String>,
    pub keywords: Vec<String>,
    pub latest_version: Option<String>,
    pub total_downloads: i64,
    pub yanked: bool,
    pub advisory_count: i64,
    pub license_expression: Option<String>,
    pub dependent_count: i64,
    pub name_similarity: f64,
}

/// API diff symbol row used by `crate.api_diff`.
#[allow(missing_docs)]
#[derive(Debug, Clone, FromRow)]
pub struct ApiDiffSymbolRow {
    pub name: String,
    pub kind: String,
    pub signature: Option<String>,
    pub visibility: Option<String>,
    pub index_source: String,
}

/// Compare snapshot version row used by `crate.compare`.
#[allow(missing_docs)]
#[derive(Debug, Clone, FromRow)]
pub struct CrateCompareVersionRow {
    pub id: i64,
    pub version: String,
    pub rust_version: Option<String>,
    pub published_at: Option<String>,
    pub yanked: bool,
    pub total_downloads: i64,
    pub license_expression: Option<String>,
}

/// Compare aggregate counts row used by `crate.compare`.
#[allow(missing_docs)]
#[derive(Debug, Clone, FromRow)]
pub struct CrateCompareCountsRow {
    pub dependency_count: i64,
    pub feature_count: i64,
    pub advisory_count: i64,
    pub dependent_count: i64,
}

/// Latest-version row used by `crate.graph` traversal.
#[allow(missing_docs)]
#[derive(Debug, Clone, FromRow)]
pub struct GraphLatestVersionRow {
    pub crate_id: i64,
    pub id: i64,
    pub version: String,
}

/// Dependency traversal row used by `crate.graph`.
#[allow(missing_docs)]
#[derive(Debug, Clone, FromRow)]
pub struct GraphDependencyTraversalRow {
    pub from_crate_name: String,
    pub from_version: String,
    pub to_crate_id: i64,
    pub to_crate_name: String,
    pub requirement: String,
    pub dependency_kind: String,
    pub optional: bool,
}

/// Dependent traversal row used by `crate.graph`.
#[allow(missing_docs)]
#[derive(Debug, Clone, FromRow)]
pub struct GraphDependentTraversalRow {
    pub from_crate_id: i64,
    pub from_crate_name: String,
    pub from_version: String,
    pub to_crate_name: String,
    pub requirement: String,
    pub dependency_kind: String,
    pub optional: bool,
}

/// Crate/id lookup row used by dependency tools.
#[allow(missing_docs)]
#[derive(Debug, Clone, FromRow)]
pub struct DependencyCrateRow {
    pub id: i64,
    pub name: String,
}

/// Dependency-version row used by dependency tools.
#[allow(missing_docs)]
#[derive(Debug, Clone, FromRow)]
pub struct DependencyVersionRow {
    pub id: i64,
    pub version: String,
    pub rust_version: Option<String>,
    pub yanked: bool,
}

/// Dependency edge row used by `dependency.resolve`.
#[allow(missing_docs)]
#[derive(Debug, Clone, FromRow)]
pub struct DependencyResolveEdgeRow {
    pub to_crate_name: String,
    pub requirement: String,
    pub optional: bool,
    pub features: Value,
}

/// One deprecated item row used by `crate.deprecated`.
#[allow(missing_docs)]
#[derive(Debug, Clone, FromRow)]
pub struct DeprecatedItemRow {
    pub name: String,
    pub kind: String,
    pub deprecated_since: Option<String>,
    pub deprecated_note: Option<String>,
    pub canonical_path: Option<String>,
    pub index_source: String,
}
