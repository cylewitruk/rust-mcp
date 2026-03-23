//! Canonical tool descriptors for all MCP tools.
//!
//! Each tool is described once here with its name and description.  These
//! constants are the single source of truth used by the server card endpoint,
//! the schema catalog, and the MCP server instructions.
//!
//! The `#[tool(...)]` proc-macro attributes in `server.rs` must use the same
//! string literals — a unit test verifies they stay in sync.

use serde::Serialize;

/// Lightweight tool descriptor for discovery and schema endpoints.
#[derive(Debug, Clone, Serialize)]
pub struct ToolDescriptor {
    pub name: &'static str,
    pub description: &'static str,
}

// ---------------------------------------------------------------------------
// Core
// ---------------------------------------------------------------------------

pub const PING: ToolDescriptor = ToolDescriptor {
    name: "ping",
    description: "Check MCP connectivity and DB readiness.",
};

pub const SCHEMA_GET: ToolDescriptor = ToolDescriptor {
    name: "schema_get",
    description: "Return request/response JSON Schemas for one or all MCP tools.",
};

// ---------------------------------------------------------------------------
// Index
// ---------------------------------------------------------------------------

pub const INDEX_CRATES: ToolDescriptor = ToolDescriptor {
    name: "index_crates",
    description: "Fetch and index crates from crates.io. Call when a crate is not yet indexed \
                  (low confidence / empty results).",
};

pub const INDEX_STATUS: ToolDescriptor = ToolDescriptor {
    name: "index_status",
    description: "Return index freshness, coverage, and queue state.",
};

pub const INDEX_REFRESH: ToolDescriptor = ToolDescriptor {
    name: "index_refresh",
    description: "Trigger index refresh for a scope and return job status.",
};

// ---------------------------------------------------------------------------
// Crate
// ---------------------------------------------------------------------------

pub const CRATE_SEARCH: ToolDescriptor = ToolDescriptor {
    name: "crate_search",
    description: "Search indexed crates by name, category, keyword, or description.",
};

pub const CRATE_INTEL: ToolDescriptor = ToolDescriptor {
    name: "crate_intel",
    description: "Start here for any crate. Dense intelligence: versions, deps, dependents, \
                  advisories.",
};

pub const CRATE_FEATURES: ToolDescriptor = ToolDescriptor {
    name: "crate_features",
    description: "Return feature flags, defaults, and transitive enables for a crate version.",
};

pub const CRATE_API_DIFF: ToolDescriptor = ToolDescriptor {
    name: "crate_api_diff",
    description: "Compare public API symbols between two crate versions: added, removed, changed.",
};

pub const CRATE_API: ToolDescriptor = ToolDescriptor {
    name: "crate_api",
    description: "Return public API symbols for a crate version with optional kind/path filters.",
};

pub const CRATE_TYPE_INFO: ToolDescriptor = ToolDescriptor {
    name: "crate_type_info",
    description: "Return type definition metadata and impl details for a crate type.",
};

pub const CRATE_TRAIT_IMPLS: ToolDescriptor = ToolDescriptor {
    name: "crate_trait_impls",
    description: "Return trait/type implementation relationships with optional filters.",
};

pub const CRATE_RE_EXPORTS: ToolDescriptor = ToolDescriptor {
    name: "crate_re_exports",
    description: "Return public re-export mappings to canonical import paths.",
};

pub const CRATE_IMPORT_PATH: ToolDescriptor = ToolDescriptor {
    name: "crate_import_path",
    description: "Resolve public import paths for a crate symbol.",
};

pub const CRATE_ERROR_TYPES: ToolDescriptor = ToolDescriptor {
    name: "crate_error_types",
    description: "Return error-type metadata, conversion impls, and functions returning each \
                  error.",
};

pub const CRATE_DEPRECATED: ToolDescriptor = ToolDescriptor {
    name: "crate_deprecated",
    description: "Return deprecated symbols with notes and suggested replacements.",
};

pub const CRATE_DERIVE_MACROS: ToolDescriptor = ToolDescriptor {
    name: "crate_derive_macros",
    description: "Return proc-macro exports (derive, attribute, function-like) for a crate.",
};

pub const CRATE_COMPARE: ToolDescriptor = ToolDescriptor {
    name: "crate_compare",
    description: "Compare two crates on adoption, risk, and maintenance signals.",
};

pub const CRATE_COMPATIBILITY: ToolDescriptor = ToolDescriptor {
    name: "crate_compatibility",
    description: "Check pairwise dependency compatibility between two crates.",
};

pub const CRATE_COMPATIBILITY_MATRIX: ToolDescriptor = ToolDescriptor {
    name: "crate_compatibility_matrix",
    description: "Evaluate compatibility across multiple version pairs between two crates.",
};

pub const CRATE_MIGRATION_PATH: ToolDescriptor = ToolDescriptor {
    name: "crate_migration_path",
    description: "Summarize migration actions for a crate upgrade from API diff breaking changes.",
};

pub const CRATE_LICENSE_CHECK: ToolDescriptor = ToolDescriptor {
    name: "crate_license_check",
    description: "Return license metadata and evaluate optional allow/deny policy lists.",
};

pub const CRATE_ALTERNATIVES: ToolDescriptor = ToolDescriptor {
    name: "crate_alternatives",
    description: "Suggest ranked alternative crates by taxonomy overlap and adoption signals.",
};

pub const CRATE_VERSIONS: ToolDescriptor = ToolDescriptor {
    name: "crate_versions",
    description: "Return crate version timeline with yanked/security/adoption markers.",
};

pub const CRATE_GRAPH: ToolDescriptor = ToolDescriptor {
    name: "crate_graph",
    description: "Return depth-bounded dependency/dependent graph for a crate.",
};

pub const CRATE_HOTSPOTS: ToolDescriptor = ToolDescriptor {
    name: "crate_hotspots",
    description: "Detect unsafe and concurrency hotspots in crate source.",
};

pub const CRATE_USAGE_PATTERNS: ToolDescriptor = ToolDescriptor {
    name: "crate_usage_patterns",
    description: "Return source snippets from dependent crates that use a target symbol.",
};

// ---------------------------------------------------------------------------
// Dependency
// ---------------------------------------------------------------------------

pub const DEPENDENCY_AUDIT: ToolDescriptor = ToolDescriptor {
    name: "dependency_audit",
    description: "Audit a Cargo.toml for yanked versions, advisories, outdated deps, and MSRV \
                  conflicts.",
};

pub const DEPENDENCY_RESOLVE: ToolDescriptor = ToolDescriptor {
    name: "dependency_resolve",
    description: "Simulate dependency resolution and report resolvable versions or conflicts.",
};

pub const DEPENDENCY_FEATURE_IMPACT: ToolDescriptor = ToolDescriptor {
    name: "dependency_feature_impact",
    description: "Estimate additional dependency surface from selected feature flags.",
};

// ---------------------------------------------------------------------------
// Source
// ---------------------------------------------------------------------------

pub const SOURCE_SEARCH: ToolDescriptor = ToolDescriptor {
    name: "source_search",
    description: "Search indexed source files by text/regex with optional crate/version/path \
                  filters.",
};

pub const SOURCE_READ: ToolDescriptor = ToolDescriptor {
    name: "source_read",
    description: "Read a line range from an indexed crate source file.",
};

pub const SOURCE_CONTEXT: ToolDescriptor = ToolDescriptor {
    name: "source_context",
    description: "Return semantic context around a source location: module path, imports, \
                  containing impl, nearby types.",
};

// ---------------------------------------------------------------------------
// Symbol / Docs
// ---------------------------------------------------------------------------

pub const SYMBOL_SEARCH: ToolDescriptor = ToolDescriptor {
    name: "symbol_search",
    description: "Search indexed symbols by name with optional crate/version/kind filters.",
};

pub const DOCS_SEARCH: ToolDescriptor = ToolDescriptor {
    name: "docs_search",
    description: "Search indexed docs.rs pages by query with optional crate/version/path filters.",
};

/// All tool descriptors in registration order.
pub const ALL_TOOLS: &[ToolDescriptor] = &[
    // Core
    PING,
    SCHEMA_GET,
    // Index
    INDEX_CRATES,
    INDEX_STATUS,
    INDEX_REFRESH,
    // Crate
    CRATE_SEARCH,
    CRATE_INTEL,
    CRATE_FEATURES,
    CRATE_API_DIFF,
    CRATE_API,
    CRATE_TYPE_INFO,
    CRATE_TRAIT_IMPLS,
    CRATE_RE_EXPORTS,
    CRATE_IMPORT_PATH,
    CRATE_ERROR_TYPES,
    CRATE_DEPRECATED,
    CRATE_DERIVE_MACROS,
    CRATE_COMPARE,
    CRATE_COMPATIBILITY,
    CRATE_COMPATIBILITY_MATRIX,
    CRATE_MIGRATION_PATH,
    CRATE_LICENSE_CHECK,
    CRATE_ALTERNATIVES,
    CRATE_VERSIONS,
    CRATE_GRAPH,
    CRATE_HOTSPOTS,
    CRATE_USAGE_PATTERNS,
    // Dependency
    DEPENDENCY_AUDIT,
    DEPENDENCY_RESOLVE,
    DEPENDENCY_FEATURE_IMPACT,
    // Source
    SOURCE_SEARCH,
    SOURCE_READ,
    SOURCE_CONTEXT,
    // Symbol / Docs
    SYMBOL_SEARCH,
    DOCS_SEARCH,
];

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::ALL_TOOLS;

    #[test]
    fn all_tool_names_are_unique() {
        let names: BTreeSet<_> = ALL_TOOLS
            .iter()
            .map(|t| t.name)
            .collect();
        assert_eq!(names.len(), ALL_TOOLS.len(), "duplicate tool names found");
    }

    #[test]
    fn expected_tool_count() {
        assert_eq!(ALL_TOOLS.len(), 35, "unexpected tool count");
    }

    #[test]
    fn tool_descriptors_match_schemas() {
        use std::collections::BTreeSet;

        let descriptor_names: BTreeSet<_> = ALL_TOOLS
            .iter()
            .map(|t| t.name)
            .collect();
        let schema_names: BTreeSet<_> = crate::schemas::all_tool_schemas()
            .iter()
            .map(|s| s.tool_name)
            .collect();
        assert_eq!(
            descriptor_names, schema_names,
            "tool descriptors and schemas must list the same tools"
        );
    }
}
