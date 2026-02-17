use rmcp::schemars::{JsonSchema, Schema, SchemaGenerator};

use crate::types;

/// Schema pair for one MCP tool.
#[derive(Debug, Clone)]
pub struct ToolSchema {
    /// MCP tool name (for example `crate.search`).
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

/// Schema exports for `schema.*` tools.
pub mod schema {
    use super::{ToolSchema, types};

    /// JSON Schemas for `schema.get`.
    pub fn get() -> ToolSchema {
        ToolSchema::new::<types::schema::ToolSchemasRequest, types::schema::ToolSchemasResponse>(
            "schema.get",
        )
    }

    /// Returns all schema tool schemas.
    pub fn all() -> Vec<ToolSchema> {
        vec![get()]
    }
}

/// Schema exports for `index.*` tools.
pub mod index {
    use super::{ToolSchema, types};

    /// JSON Schemas for `index.sync_crates`.
    pub fn sync_crates() -> ToolSchema {
        ToolSchema::new::<types::index::IndexSyncCratesRequest, types::index::IndexSyncCratesResponse>(
            "index.sync_crates",
        )
    }

    /// JSON Schemas for `index.status`.
    pub fn status() -> ToolSchema {
        ToolSchema::new::<types::index::IndexStatusRequest, types::index::IndexStatusResponse>(
            "index.status",
        )
    }

    /// JSON Schemas for `index.refresh`.
    pub fn refresh() -> ToolSchema {
        ToolSchema::new::<types::index::IndexRefreshRequest, types::index::IndexRefreshResponse>(
            "index.refresh",
        )
    }

    /// Returns all index tool schemas.
    pub fn all() -> Vec<ToolSchema> {
        vec![sync_crates(), status(), refresh()]
    }
}

/// Schema exports for `source.*` tools.
pub mod source {
    use super::{ToolSchema, types};

    /// JSON Schemas for `source.search`.
    pub fn search() -> ToolSchema {
        ToolSchema::new::<types::source::SourceSearchRequest, types::source::SourceSearchResponse>(
            "source.search",
        )
    }

    /// JSON Schemas for `source.read`.
    pub fn read() -> ToolSchema {
        ToolSchema::new::<types::source::SourceReadRequest, types::source::SourceReadResponse>(
            "source.read",
        )
    }

    /// JSON Schemas for `source.context`.
    pub fn context() -> ToolSchema {
        ToolSchema::new::<types::source::SourceContextRequest, types::source::SourceContextResponse>(
            "source.context",
        )
    }

    /// Returns all source tool schemas.
    pub fn all() -> Vec<ToolSchema> {
        vec![search(), read(), context()]
    }
}

/// Schema exports for `symbol.*` tools.
pub mod symbol {
    use super::{ToolSchema, types};

    /// JSON Schemas for `symbol.search`.
    pub fn search() -> ToolSchema {
        ToolSchema::new::<types::symbol::SymbolSearchRequest, types::symbol::SymbolSearchResponse>(
            "symbol.search",
        )
    }

    /// Returns all symbol tool schemas.
    pub fn all() -> Vec<ToolSchema> {
        vec![search()]
    }
}

/// Schema exports for `docs.*` tools.
pub mod docs {
    use super::{ToolSchema, types};

    /// JSON Schemas for `docs.search`.
    pub fn search() -> ToolSchema {
        ToolSchema::new::<types::docs::DocsSearchRequest, types::docs::DocsSearchResponse>(
            "docs.search",
        )
    }

    /// Returns all docs tool schemas.
    pub fn all() -> Vec<ToolSchema> {
        vec![search()]
    }
}

/// Schema exports for `dependency.*` tools.
pub mod dependency {
    use super::{ToolSchema, types};

    /// JSON Schemas for `dependency.audit`.
    pub fn audit() -> ToolSchema {
        ToolSchema::new::<
            types::dependency::DependencyAuditRequest,
            types::dependency::DependencyAuditResponse,
        >("dependency.audit")
    }

    /// JSON Schemas for `dependency.resolve`.
    pub fn resolve() -> ToolSchema {
        ToolSchema::new::<
            types::dependency::DependencyResolveRequest,
            types::dependency::DependencyResolveResponse,
        >("dependency.resolve")
    }

    /// JSON Schemas for `dependency.feature_impact`.
    pub fn feature_impact() -> ToolSchema {
        ToolSchema::new::<
            types::dependency::DependencyFeatureImpactRequest,
            types::dependency::DependencyFeatureImpactResponse,
        >("dependency.feature_impact")
    }

    /// Returns all dependency tool schemas.
    pub fn all() -> Vec<ToolSchema> {
        vec![audit(), resolve(), feature_impact()]
    }
}

/// Schema exports for `crate.*` tools.
pub mod krate {
    use super::{ToolSchema, types};

    /// JSON Schemas for `crate.search`.
    pub fn search() -> ToolSchema {
        ToolSchema::new::<types::krate::CrateSearchRequest, types::krate::CrateSearchResponse>(
            "crate.search",
        )
    }

    /// JSON Schemas for `crate.intel`.
    pub fn intel() -> ToolSchema {
        ToolSchema::new::<types::krate::CrateIntelRequest, types::krate::CrateIntelResponse>(
            "crate.intel",
        )
    }

    /// JSON Schemas for `crate.features`.
    pub fn features() -> ToolSchema {
        ToolSchema::new::<types::krate::CrateFeaturesRequest, types::krate::CrateFeaturesResponse>(
            "crate.features",
        )
    }

    /// JSON Schemas for `crate.api_diff`.
    pub fn api_diff() -> ToolSchema {
        ToolSchema::new::<types::krate::CrateApiDiffRequest, types::krate::CrateApiDiffResponse>(
            "crate.api_diff",
        )
    }

    /// JSON Schemas for `crate.api`.
    pub fn api() -> ToolSchema {
        ToolSchema::new::<types::krate::CrateApiRequest, types::krate::CrateApiResponse>(
            "crate.api",
        )
    }

    /// JSON Schemas for `crate.type_info`.
    pub fn type_info() -> ToolSchema {
        ToolSchema::new::<types::krate::CrateTypeInfoRequest, types::krate::CrateTypeInfoResponse>(
            "crate.type_info",
        )
    }

    /// JSON Schemas for `crate.trait_impls`.
    pub fn trait_impls() -> ToolSchema {
        ToolSchema::new::<types::krate::CrateTraitImplsRequest, types::krate::CrateTraitImplsResponse>(
            "crate.trait_impls",
        )
    }

    /// JSON Schemas for `crate.re_exports`.
    pub fn re_exports() -> ToolSchema {
        ToolSchema::new::<types::krate::CrateReExportsRequest, types::krate::CrateReExportsResponse>(
            "crate.re_exports",
        )
    }

    /// JSON Schemas for `crate.error_types`.
    pub fn error_types() -> ToolSchema {
        ToolSchema::new::<types::krate::CrateErrorTypesRequest, types::krate::CrateErrorTypesResponse>(
            "crate.error_types",
        )
    }

    /// JSON Schemas for `crate.derive_macros`.
    pub fn derive_macros() -> ToolSchema {
        ToolSchema::new::<
            types::krate::CrateDeriveMacrosRequest,
            types::krate::CrateDeriveMacrosResponse,
        >("crate.derive_macros")
    }

    /// JSON Schemas for `crate.compare`.
    pub fn compare() -> ToolSchema {
        ToolSchema::new::<types::krate::CrateCompareRequest, types::krate::CrateCompareResponse>(
            "crate.compare",
        )
    }

    /// JSON Schemas for `crate.compatibility`.
    pub fn compatibility() -> ToolSchema {
        ToolSchema::new::<
            types::krate::CrateCompatibilityRequest,
            types::krate::CrateCompatibilityResponse,
        >("crate.compatibility")
    }

    /// JSON Schemas for `crate.compatibility_matrix`.
    pub fn compatibility_matrix() -> ToolSchema {
        ToolSchema::new::<
            types::krate::CrateCompatibilityMatrixRequest,
            types::krate::CrateCompatibilityMatrixResponse,
        >("crate.compatibility_matrix")
    }

    /// JSON Schemas for `crate.migration_path`.
    pub fn migration_path() -> ToolSchema {
        ToolSchema::new::<
            types::krate::CrateMigrationPathRequest,
            types::krate::CrateMigrationPathResponse,
        >("crate.migration_path")
    }

    /// JSON Schemas for `crate.license_check`.
    pub fn license_check() -> ToolSchema {
        ToolSchema::new::<
            types::krate::CrateLicenseCheckRequest,
            types::krate::CrateLicenseCheckResponse,
        >("crate.license_check")
    }

    /// JSON Schemas for `crate.alternatives`.
    pub fn alternatives() -> ToolSchema {
        ToolSchema::new::<
            types::krate::CrateAlternativesRequest,
            types::krate::CrateAlternativesResponse,
        >("crate.alternatives")
    }

    /// JSON Schemas for `crate.versions`.
    pub fn versions() -> ToolSchema {
        ToolSchema::new::<types::krate::CrateVersionsRequest, types::krate::CrateVersionsResponse>(
            "crate.versions",
        )
    }

    /// JSON Schemas for `crate.graph`.
    pub fn graph() -> ToolSchema {
        ToolSchema::new::<types::krate::CrateGraphRequest, types::krate::CrateGraphResponse>(
            "crate.graph",
        )
    }

    /// JSON Schemas for `crate.hotspots`.
    pub fn hotspots() -> ToolSchema {
        ToolSchema::new::<types::krate::CrateHotspotsRequest, types::krate::CrateHotspotsResponse>(
            "crate.hotspots",
        )
    }

    /// JSON Schemas for `crate.usage_patterns`.
    pub fn usage_patterns() -> ToolSchema {
        ToolSchema::new::<
            types::krate::CrateUsagePatternsRequest,
            types::krate::CrateUsagePatternsResponse,
        >("crate.usage_patterns")
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
        assert_eq!(schemas.len(), 33, "unexpected tool schema count");
    }

    #[test]
    fn tool_schema_lookup_finds_known_tool() {
        let schema = tool_schema("schema.get");
        assert!(schema.is_some(), "expected schema.get schema to exist");
    }
}
