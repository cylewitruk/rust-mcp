use rmcp::schemars;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::FromRow;

// ---------------------------------------------------------------------------
// Shared response envelope types
// ---------------------------------------------------------------------------

/// Freshness provenance attached to tool responses that perform freshness
/// checks against the index.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct ResponseFreshnessSource {
    pub source: String,
    pub status: String,
    pub checked_at: Option<String>,
}

/// Coarse confidence level used across tool responses.
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

/// Structured confidence assessment included in tool responses.
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
pub struct ConfidenceAssessment {
    pub level: ConfidenceLevel,
    pub reason: String,
}

/// License policy evaluation result.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LicensePolicyResult {
    Allowed,
    Denied,
    Unknown,
}

// ---------------------------------------------------------------------------
// Shared request / response types
// ---------------------------------------------------------------------------

/// Request payload for `source.read`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SourceReadRequest {
    pub crate_name: String,
    pub version: Option<String>,
    pub path: String,
    pub start_line: Option<u32>,
    pub end_line: Option<u32>,
}

/// Response payload for `source.read`.
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

/// A method entry inside an impl block (shared by type_info / trait_impls).
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateImplMethod {
    pub name: String,
    pub signature: Option<String>,
}

// ---------------------------------------------------------------------------
// Shared DB row types
// ---------------------------------------------------------------------------

/// Row returned by full-text / fuzzy crate search queries.
#[derive(Debug, FromRow)]
pub(crate) struct CrateSearchRow {
    pub(crate) crate_id: i64,
    pub(crate) name: String,
    pub(crate) description: Option<String>,
    pub(crate) repository_url: Option<String>,
    pub(crate) docs_url: Option<String>,
    pub(crate) homepage_url: Option<String>,
    pub(crate) categories: Vec<String>,
    pub(crate) keywords: Vec<String>,
    pub(crate) total_downloads: i64,
    pub(crate) latest_version: Option<String>,
    pub(crate) latest_published_at: Option<String>,
    pub(crate) dependent_count: i64,
    pub(crate) relevance_score: f64,
}

/// Row for reading indexed source content.
#[derive(Debug, FromRow)]
pub(crate) struct SourceReadRow {
    pub(crate) crate_name: String,
    pub(crate) version: String,
    pub(crate) path: String,
    pub(crate) content: String,
}

/// Core crate metadata row shared by multiple tools.
#[derive(Debug, FromRow)]
pub(crate) struct CrateCoreRow {
    pub(crate) id: i64,
    pub(crate) name: String,
    pub(crate) description: Option<String>,
    pub(crate) repository_url: Option<String>,
    pub(crate) docs_url: Option<String>,
    pub(crate) homepage_url: Option<String>,
    pub(crate) categories: Vec<String>,
    pub(crate) keywords: Vec<String>,
    pub(crate) updated_at: Option<String>,
}

/// Version selection row used when resolving the target version for a crate.
#[derive(Debug, Clone, FromRow)]
pub(crate) struct CrateVersionSelectionRow {
    pub(crate) id: i64,
    pub(crate) version: String,
    pub(crate) rust_version: Option<String>,
    pub(crate) published_at: Option<String>,
    pub(crate) readme: Option<String>,
}

/// Impl-block lookup row used by type_info and trait_impls tools.
#[derive(Debug, Clone, FromRow)]
pub(crate) struct CrateImplLookupRow {
    pub(crate) type_name: String,
    pub(crate) type_name_display: Option<String>,
    pub(crate) trait_name: Option<String>,
    pub(crate) trait_name_display: Option<String>,
    pub(crate) impl_kind: String,
    pub(crate) methods: Value,
    pub(crate) source_path: String,
    pub(crate) start_line: i32,
    pub(crate) end_line: i32,
    pub(crate) index_source: String,
}
