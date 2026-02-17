use rmcp::schemars;
use serde::{Deserialize, Serialize};

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
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct CrateImplMethod {
    pub name: String,
    pub signature: Option<String>,
}

/// Associated type information declared by a trait definition.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct CrateTraitAssociatedType {
    pub name: String,
    pub bounds: Vec<String>,
    pub default: Option<String>,
}

/// Trait definition metadata extracted from indexed sources.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
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

// ---------------------------------------------------------------------------
// Shared DB row types (re-exported from crate::db::models)
// ---------------------------------------------------------------------------

pub(crate) use crate::db::models::{
    CrateCoreRow, CrateSearchRow, CrateVersionSelectionRow, SourceReadRow,
};
