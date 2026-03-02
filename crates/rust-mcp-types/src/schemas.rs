use rmcp::schemars::{JsonSchema, Schema, SchemaGenerator};

use crate::types;

/// Schema pair for one MCP tool.
#[derive(Debug, Clone)]
pub struct ToolSchema {
    /// MCP tool name (for example `crate_search`).
    pub tool_name: &'static str,
    /// JSON Schema for the request payload.
    pub request: Schema,
    /// JSON Schema for the response payload.
    pub response: Schema,
}

impl ToolSchema {
    fn new<Request, Response>(tool_name: &'static str) -> Self
    where
        Request: JsonSchema,
        Response: JsonSchema,
    {
        Self {
            tool_name,
            request: schema_for_type::<Request>(),
            response: schema_for_type::<Response>(),
        }
    }
}

fn schema_for_type<T: JsonSchema>() -> Schema {
    SchemaGenerator::default().into_root_schema_for::<T>()
}

/// Schema exports for core tools.
pub mod core {
    use super::{ToolSchema, types};

    /// JSON Schemas for `ping`.
    pub fn ping() -> ToolSchema {
        ToolSchema::new::<types::common::PingRequest, String>("ping")
    }

    /// Returns all core tool schemas.
    pub fn all() -> Vec<ToolSchema> {
        vec![ping()]
    }
}

/// Schema exports for `schema_*` tools.
pub mod schema {
    use super::{ToolSchema, types};

    /// JSON Schemas for `schema_get`.
    pub fn get() -> ToolSchema {
        ToolSchema::new::<types::schema::ToolSchemasRequest, types::schema::ToolSchemasResponse>(
            "schema_get",
        )
    }

    /// Returns all schema tool schemas.
    pub fn all() -> Vec<ToolSchema> {
        vec![get()]
    }
}

/// Schema exports for `index_*` tools.
pub mod index {
    use super::{ToolSchema, types};

    /// JSON Schemas for `index_sync_crates`.
    pub fn sync_crates() -> ToolSchema {
        ToolSchema::new::<types::index::IndexSyncCratesRequest, types::index::IndexSyncCratesResponse>(
            "index_sync_crates",
        )
    }

    /// JSON Schemas for `index_status`.
    pub fn status() -> ToolSchema {
        ToolSchema::new::<types::index::IndexStatusRequest, types::index::IndexStatusResponse>(
            "index_status",
        )
    }

    /// JSON Schemas for `index_refresh`.
    pub fn refresh() -> ToolSchema {
        ToolSchema::new::<types::index::IndexRefreshRequest, types::index::IndexRefreshResponse>(
            "index_refresh",
        )
    }

    /// Returns all index tool schemas.
    pub fn all() -> Vec<ToolSchema> {
        vec![sync_crates(), status(), refresh()]
    }
}

/// Schema exports for `source_*` tools.
pub mod source {
    use super::{ToolSchema, types};

    /// JSON Schemas for `source_search`.
    pub fn search() -> ToolSchema {
        ToolSchema::new::<types::source::SourceSearchRequest, types::source::SourceSearchResponse>(
            "source_search",
        )
    }

    /// JSON Schemas for `source_read`.
    pub fn read() -> ToolSchema {
        ToolSchema::new::<types::source::SourceReadRequest, types::source::SourceReadResponse>(
            "source_read",
        )
    }

    /// JSON Schemas for `source_context`.
    pub fn context() -> ToolSchema {
        ToolSchema::new::<types::source::SourceContextRequest, types::source::SourceContextResponse>(
            "source_context",
        )
    }

    /// Returns all source tool schemas.
    pub fn all() -> Vec<ToolSchema> {
        vec![search(), read(), context()]
    }
}

/// Schema exports for `symbol_*` tools.
pub mod symbol {
    use super::{ToolSchema, types};

    /// JSON Schemas for `symbol_search`.
    pub fn search() -> ToolSchema {
        ToolSchema::new::<types::symbol::SymbolSearchRequest, types::symbol::SymbolSearchResponse>(
            "symbol_search",
        )
    }

    /// Returns all symbol tool schemas.
    pub fn all() -> Vec<ToolSchema> {
        vec![search()]
    }
}

/// Schema exports for `docs_*` tools.
pub mod docs {
    use super::{ToolSchema, types};

    /// JSON Schemas for `docs_search`.
    pub fn search() -> ToolSchema {
        ToolSchema::new::<types::docs::DocsSearchRequest, types::docs::DocsSearchResponse>(
            "docs_search",
        )
    }

    /// Returns all docs tool schemas.
    pub fn all() -> Vec<ToolSchema> {
        vec![search()]
    }
}

/// Schema exports for `dependency_*` tools.
pub mod dependency {
    use super::{ToolSchema, types};

    /// JSON Schemas for `dependency_audit`.
    pub fn audit() -> ToolSchema {
        ToolSchema::new::<
            types::dependency::DependencyAuditRequest,
            types::dependency::DependencyAuditResponse,
        >("dependency_audit")
    }

    /// JSON Schemas for `dependency_resolve`.
    pub fn resolve() -> ToolSchema {
        ToolSchema::new::<
            types::dependency::DependencyResolveRequest,
            types::dependency::DependencyResolveResponse,
        >("dependency_resolve")
    }

    /// JSON Schemas for `dependency_feature_impact`.
    pub fn feature_impact() -> ToolSchema {
        ToolSchema::new::<
            types::dependency::DependencyFeatureImpactRequest,
            types::dependency::DependencyFeatureImpactResponse,
        >("dependency_feature_impact")
    }

    /// Returns all dependency tool schemas.
    pub fn all() -> Vec<ToolSchema> {
        vec![audit(), resolve(), feature_impact()]
    }
}

/// Schema exports for `crate_*` tools.
pub mod krate {
    use super::{ToolSchema, types};

    /// JSON Schemas for `crate_search`.
    pub fn search() -> ToolSchema {
        ToolSchema::new::<types::krate::CrateSearchRequest, types::krate::CrateSearchResponse>(
            "crate_search",
        )
    }

    /// JSON Schemas for `crate_intel`.
    pub fn intel() -> ToolSchema {
        ToolSchema::new::<types::krate::CrateIntelRequest, types::krate::CrateIntelResponse>(
            "crate_intel",
        )
    }

    /// JSON Schemas for `crate_features`.
    pub fn features() -> ToolSchema {
        ToolSchema::new::<types::krate::CrateFeaturesRequest, types::krate::CrateFeaturesResponse>(
            "crate_features",
        )
    }

    /// JSON Schemas for `crate_api_diff`.
    pub fn api_diff() -> ToolSchema {
        ToolSchema::new::<types::krate::CrateApiDiffRequest, types::krate::CrateApiDiffResponse>(
            "crate_api_diff",
        )
    }

    /// JSON Schemas for `crate_api`.
    pub fn api() -> ToolSchema {
        ToolSchema::new::<types::krate::CrateApiRequest, types::krate::CrateApiResponse>(
            "crate_api",
        )
    }

    /// JSON Schemas for `crate_type_info`.
    pub fn type_info() -> ToolSchema {
        ToolSchema::new::<types::krate::CrateTypeInfoRequest, types::krate::CrateTypeInfoResponse>(
            "crate_type_info",
        )
    }

    /// JSON Schemas for `crate_trait_impls`.
    pub fn trait_impls() -> ToolSchema {
        ToolSchema::new::<types::krate::CrateTraitImplsRequest, types::krate::CrateTraitImplsResponse>(
            "crate_trait_impls",
        )
    }

    /// JSON Schemas for `crate_re_exports`.
    pub fn re_exports() -> ToolSchema {
        ToolSchema::new::<types::krate::CrateReExportsRequest, types::krate::CrateReExportsResponse>(
            "crate_re_exports",
        )
    }

    /// JSON Schemas for `crate_import_path`.
    pub fn import_path() -> ToolSchema {
        ToolSchema::new::<types::krate::CrateImportPathRequest, types::krate::CrateImportPathResponse>(
            "crate_import_path",
        )
    }

    /// JSON Schemas for `crate_error_types`.
    pub fn error_types() -> ToolSchema {
        ToolSchema::new::<types::krate::CrateErrorTypesRequest, types::krate::CrateErrorTypesResponse>(
            "crate_error_types",
        )
    }

    /// JSON Schemas for `crate_derive_macros`.
    pub fn derive_macros() -> ToolSchema {
        ToolSchema::new::<
            types::krate::CrateDeriveMacrosRequest,
            types::krate::CrateDeriveMacrosResponse,
        >("crate_derive_macros")
    }

    /// JSON Schemas for `crate_compare`.
    pub fn compare() -> ToolSchema {
        ToolSchema::new::<types::krate::CrateCompareRequest, types::krate::CrateCompareResponse>(
            "crate_compare",
        )
    }

    /// JSON Schemas for `crate_compatibility`.
    pub fn compatibility() -> ToolSchema {
        ToolSchema::new::<
            types::krate::CrateCompatibilityRequest,
            types::krate::CrateCompatibilityResponse,
        >("crate_compatibility")
    }

    /// JSON Schemas for `crate_compatibility_matrix`.
    pub fn compatibility_matrix() -> ToolSchema {
        ToolSchema::new::<
            types::krate::CrateCompatibilityMatrixRequest,
            types::krate::CrateCompatibilityMatrixResponse,
        >("crate_compatibility_matrix")
    }

    /// JSON Schemas for `crate_migration_path`.
    pub fn migration_path() -> ToolSchema {
        ToolSchema::new::<
            types::krate::CrateMigrationPathRequest,
            types::krate::CrateMigrationPathResponse,
        >("crate_migration_path")
    }

    /// JSON Schemas for `crate_license_check`.
    pub fn license_check() -> ToolSchema {
        ToolSchema::new::<
            types::krate::CrateLicenseCheckRequest,
            types::krate::CrateLicenseCheckResponse,
        >("crate_license_check")
    }

    /// JSON Schemas for `crate_alternatives`.
    pub fn alternatives() -> ToolSchema {
        ToolSchema::new::<
            types::krate::CrateAlternativesRequest,
            types::krate::CrateAlternativesResponse,
        >("crate_alternatives")
    }

    /// JSON Schemas for `crate_versions`.
    pub fn versions() -> ToolSchema {
        ToolSchema::new::<types::krate::CrateVersionsRequest, types::krate::CrateVersionsResponse>(
            "crate_versions",
        )
    }

    /// JSON Schemas for `crate_graph`.
    pub fn graph() -> ToolSchema {
        ToolSchema::new::<types::krate::CrateGraphRequest, types::krate::CrateGraphResponse>(
            "crate_graph",
        )
    }

    /// JSON Schemas for `crate_hotspots`.
    pub fn hotspots() -> ToolSchema {
        ToolSchema::new::<types::krate::CrateHotspotsRequest, types::krate::CrateHotspotsResponse>(
            "crate_hotspots",
        )
    }

    /// JSON Schemas for `crate_usage_patterns`.
    pub fn usage_patterns() -> ToolSchema {
        ToolSchema::new::<
            types::krate::CrateUsagePatternsRequest,
            types::krate::CrateUsagePatternsResponse,
        >("crate_usage_patterns")
    }

    /// Returns all crate tool schemas.
    pub fn all() -> Vec<ToolSchema> {
        vec![
            search(),
            intel(),
            features(),
            api_diff(),
            api(),
            type_info(),
            trait_impls(),
            re_exports(),
            import_path(),
            error_types(),
            derive_macros(),
            compare(),
            compatibility(),
            compatibility_matrix(),
            migration_path(),
            license_check(),
            alternatives(),
            versions(),
            graph(),
            hotspots(),
            usage_patterns(),
        ]
    }
}

/// Returns all MCP tool schemas grouped in a single list.
pub fn all_tool_schemas() -> Vec<ToolSchema> {
    let mut out = Vec::new();
    out.extend(core::all());
    out.extend(schema::all());
    out.extend(index::all());
    out.extend(krate::all());
    out.extend(dependency::all());
    out.extend(source::all());
    out.extend(symbol::all());
    out.extend(docs::all());
    out
}

/// Returns the schema pair for one tool name.
pub fn tool_schema(tool_name: &str) -> Option<ToolSchema> {
    all_tool_schemas()
        .into_iter()
        .find(|schema| schema.tool_name == tool_name)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{all_tool_schemas, tool_schema};

    #[test]
    fn all_tool_schemas_are_unique_and_complete() {
        let schemas = all_tool_schemas();
        let names = schemas
            .iter()
            .map(|schema| schema.tool_name)
            .collect::<BTreeSet<_>>();

        assert_eq!(schemas.len(), names.len(), "duplicate tool names found");
        assert_eq!(schemas.len(), 34, "unexpected tool schema count");
    }

    #[test]
    fn tool_schema_lookup_finds_known_tool() {
        let schema = tool_schema("schema_get");
        assert!(schema.is_some(), "expected schema.get schema to exist");
    }
}
