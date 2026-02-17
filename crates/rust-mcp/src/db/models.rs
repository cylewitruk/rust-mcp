use serde::Deserialize;
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
    /// Full source file contents.
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
