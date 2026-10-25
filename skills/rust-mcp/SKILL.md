---
name: rust-mcp
description: Rust dependency intelligence for published crates.io crates via the rust-mcp MCP server. Look up crate APIs, types, trait impls, import paths, features, versions, docs and source; compare or choose crates; plan version upgrades from API diffs; check Cargo.toml dependencies for advisories, yanked versions and MSRV conflicts. Use for questions about third-party Rust crates and Cargo dependencies, before web search or reading ~/.cargo/registry.
---

# rust-mcp

The rust-mcp MCP server indexes crates.io crates (metadata, rustdoc API, source, docs) and answers structured queries about them. Its tools are named like `crate_intel`; your client may prefix them (e.g. `mcp__rust-mcp__crate_intel`).

**Scope:** published crates.io crates only. It does not cover your own workspace code, unpublished/path/git dependencies, or the standard library; read those directly.

## Ground rules

- **Run only the steps needed to answer the question.** The workflows below are menus, not checklists.
- **Pick the tool by question type:** for "what exists, how is it typed, how do I import it", use the API tools (`crate_api`, `crate_type_info`, `crate_import_path`, `symbol_search`). For "how is it implemented", go straight to the source tools.
- **Parameter names:** crates are `crate_name` (two-crate tools use `left_crate`/`right_crate`). `version` defaults to the latest indexed version; pass the version the project uses.
- **Missing data:** unknown crates are indexed on demand, so a first call can be slow. An empty result can be correct. If `confidence_assessment.reason` says data is missing, index it (`index_crates`, or `index_refresh` with `scope: "docs"` for docs.rs pages) and retry once. If coverage is still incomplete, say so rather than retrying again.
- **Responses:** consider relevant `suggested_next_tools`; paginate with `next_cursor` only when you need more results.
- **Heuristic outputs:** migration actions, usage snippets and dependency resolution are estimates from indexed data. Verify before changing code; only Cargo can confirm that a project resolves and builds.

## Workflows

| Task | Read |
| ---- | ---- |
| Understand or evaluate a crate | [workflows/evaluate-crate.md](workflows/evaluate-crate.md) |
| Find an API, type, import path, or real usage of a symbol | [workflows/find-api.md](workflows/find-api.md) |
| Upgrade a dependency or fix breakage after a version bump | [workflows/upgrade-dependency.md](workflows/upgrade-dependency.md) |
| Choose between crates or find alternatives | [workflows/choose-crate.md](workflows/choose-crate.md) |
| Check a project's dependencies | [workflows/audit-dependencies.md](workflows/audit-dependencies.md) |

## Tool reference

| Family | Tools | Read |
| ---- | ---- | ---- |
| Discovery & overview | `crate_search`, `crate_intel`, `crate_versions`, `crate_features`, `crate_graph` | [tools/discovery.md](tools/discovery.md) |
| API surface | `crate_api`, `crate_type_info`, `crate_trait_impls`, `crate_import_path`, `crate_re_exports`, `crate_error_types`, `crate_deprecated`, `crate_derive_macros`, `crate_api_diff` | [tools/api.md](tools/api.md) |
| Comparison & ecosystem | `crate_compare`, `crate_alternatives`, `crate_compatibility`, `crate_compatibility_matrix`, `crate_license_check`, `crate_migration_path`, `crate_usage_patterns`, `crate_hotspots` | [tools/ecosystem.md](tools/ecosystem.md) |
| Dependencies | `dependency_audit`, `dependency_resolve`, `dependency_feature_impact` | [tools/dependencies.md](tools/dependencies.md) |
| Source, symbols & docs | `source_search`, `source_read`, `source_context`, `symbol_search`, `docs_search` | [tools/source.md](tools/source.md) |
| Index & server | `index_crates`, `index_status`, `index_refresh`, `ping`, `schema_get` | [tools/index.md](tools/index.md) |

Parameter details and defaults are in each tool's input schema (or `schema_get`); the references here cover when and how to use each tool.
