# Roadmap: Type Intelligence & Agent Accuracy (2026-02)

Status: Active
Owner: rust-mcp maintainers
Last updated: 2026-02-14

Supersedes: `docs/roadmap-gap-closure-2026-02.md`, `docs/m7-implementation-checklist.md` (both archived)

## Current baseline

All milestones M5–M8 from the prior roadmap are complete. The server ships 20 MCP tools across crate intelligence, source/symbol search, dependency audit, and docs search. Infrastructure includes Prometheus metrics, per-source rate limiting, adaptive TTL refresh, and a durable job queue.

### Completed milestone summary

| Milestone | Scope | Status |
| ----------- | ------- | -------- |
| M5 | docs.rs ingestion, `index.refresh` scopes, outbound rate limiting | Done |
| M6 | `crate.features`, MSRV surfacing, `crate.graph` cycle detection, Prometheus, request throttle | Done |
| M7 | `crate.api_diff`, `crate.license_check`, `crate.alternatives`, `crate.hotspots` | Done |
| M8 | `crate.api`, `crate.compare`, `dependency.audit` | Done |

## Backlog overview

Work is organized into three tracks:

- **Track A — Spec/README consistency fixes**: close cosmetic and contract gaps between spec claims and actual behavior.
- **Track B — Type-level intelligence**: new tools targeting the top agent failure modes in Rust (hallucinated signatures, incorrect trait assumptions, wrong import paths).
- **Track C — Resolution & ecosystem intelligence**: tools that help agents make correct dependency decisions before writing code.

## Track A: Spec/README consistency (housekeeping)

These are low-effort fixes to close the gap between what the spec/README claims and what the server actually does.

### A1. Unify confidence contract across all tools

**Problem**: M7+ tools use structured `confidence_assessment { level, reason }` while older tools (M5–M6 era) return string-only `confidence`. Spec section 10 implies a uniform contract.

**Work**:

- Migrate all legacy tool responses to the structured `ConfidenceAssessment` model.
- Remove or alias the old `confidence: String` fields for backward compatibility during transition.

**Acceptance**: every tool response includes `confidence_assessment` with `level` (enum) and `reason` (string).

### A2. Update README to match reality

**Problem**: README line 3 still calls the project a "scaffold." Tool list is an unreadable inline sentence. Several spec-mentioned details (adaptive TTL, rate limiting, Prometheus) have no README coverage.

**Work**:

- Replace "scaffold" framing with accurate project description.
- Restructure tool list into a scannable table or grouped list (already partially done in collapsible sections, but the lede is misleading).
- Add brief sections for observability (Prometheus port), rate limiting behavior, and adaptive TTL.

### A3. Cold-fetch progress streaming

**Problem**: Spec section 11 (NFR) says cold remote fetches should "stream progress." Currently they block with no client-visible progress.

**Work**:

- For long-running inline refresh (docs ingestion, bulk sync), emit SSE progress events on the MCP transport.
- Scope: `index.sync_crates`, `index.refresh` with large scope.

**Acceptance**: client receives at least one intermediate progress event before final result on operations exceeding 5 seconds.

### A4. `index.refresh` estimated completion

**Problem**: Spec section 9 says `index.refresh` output includes "estimated completion." Implementation returns job status but no ETA.

**Work**:

- Add `estimated_seconds_remaining` (nullable) to refresh job responses.
- Estimate from historical job duration by scope, or return null when insufficient data.

## Track B: Type-level intelligence (new tools)

These address the three most common Rust agent failure modes: hallucinated method signatures, incorrect trait assumptions, and wrong import paths.

### B1. `crate.type_info` — deep type introspection

**Priority**: High
**Rationale**: This is the single highest-leverage addition. When an agent writes code using `axum::Router` or `sqlx::PgPool`, it needs to know the methods available on that type, its generic parameters, and what traits it implements. Currently `crate.api` returns symbol-level entries (name + kind + signature line) but does not model the internal structure of types or associate methods with their `impl` blocks.

**Input**:

- `crate_name`
- `type_name` (e.g., `Router`, `PgPool`)
- optional `version`
- optional `include_methods` (default true)
- optional `include_trait_impls` (default true)

**Output**:

- type definition: kind (struct/enum/union), generic parameters, visibility
- fields (structs) or variants (enums) with types
- inherent methods with full signatures
- trait implementations (trait name + implemented methods)
- `From`/`Into`/`TryFrom` conversions
- provenance, confidence, next_best_calls

**Implementation notes**:

- Extend `syn`-based extraction in `local_cache.rs` to capture:
  - struct field lists and enum variant definitions
  - `impl Type { ... }` method blocks associated with their target type
  - `impl Trait for Type` blocks with trait identity
- New DB table or extended `symbols` schema to store type-method and type-trait associations.
- Query joins symbols by target type within a crate version.

**Acceptance**: given an indexed crate, `crate.type_info` returns the correct fields, methods, and trait impls for a named type matching what `cargo doc` would show.

### B2. `crate.trait_impls` — bidirectional trait lookup

**Priority**: High
**Rationale**: "What types implement `serde::Deserialize`?" and "What traits does `reqwest::Response` implement?" are among the most frequent agent queries. Currently requires `source.search` for `impl Trait for` patterns and manual result parsing.

**Input**:

- `crate_name`
- optional `version`
- one of:
  - `trait_name` — find all types implementing this trait
  - `type_name` — find all traits implemented by this type
- optional `limit`

**Output**:

- list of `(type, trait, impl_location)` tuples
- includes both inherent and derived trait impls
- blanket impls noted but not expanded

**Implementation notes**:

- Relies on the same `impl` block extraction as B1. Can share the DB schema extension.
- For derive macros (`#[derive(Debug, Clone, Serialize)]`), parse derive attributes to infer trait impls without seeing the expanded code.

**Acceptance**: for an indexed crate, returns correct trait-type relationships matching `cargo doc` output.

### B3. `crate.re_exports` — public API boundary mapping

**Priority**: Medium
**Rationale**: Agents frequently use incorrect import paths (e.g., internal module paths instead of re-exported public paths). `crate.api` gives symbols but doesn't model the re-export tree that determines correct `use` statements.

**Input**:

- `crate_name`
- optional `version`
- optional `path_prefix` filter

**Output**:

- list of `(canonical_path, original_definition_path, kind, visibility)` entries
- identifies the shortest public path for each re-exported item

**Implementation notes**:

- Parse `pub use` and `pub mod` statements in `lib.rs` and module tree.
- Build a re-export graph mapping definition locations to their public access paths.
- This is challenging with `syn` alone for complex re-export patterns (`pub use crate::internal::*`); accept best-effort coverage and mark confidence accordingly.

**Acceptance**: for common crates with straightforward re-export patterns, returns correct canonical import paths.

### B4. `crate.error_types` — error type enumeration

**Priority**: Medium
**Rationale**: Error handling is one of the most boilerplate-heavy parts of Rust. Agents need to know what errors a function can return and what conversions exist. Currently requires manual source search and pattern matching.

**Input**:

- `crate_name`
- optional `version`
- optional `type_name` (filter to a specific error type)

**Output**:

- error types: name, variants (for enums), `Display` message patterns
- `From` impl chains (what converts into this error)
- which functions/methods return this error type
- `source()` chain if detectable

**Implementation notes**:

- Identify error types heuristically: types named `*Error` or `*Err`, types implementing `std::error::Error`, types appearing in `Result<_, E>` return positions.
- Extract `From<X> for ErrorType` impls to build conversion graph.
- Builds on B1's `impl` block extraction.

**Acceptance**: for an indexed crate, correctly identifies the primary error types and their `From` conversion sources.

### B5. `crate.derive_macros` — procedural macro discovery

**Priority**: Low
**Rationale**: Procedural macros are invisible to static analysis and are a major source of agent confusion. Knowing that `serde` provides `#[derive(Serialize)]` with attributes like `#[serde(rename_all = "...")]` is critical context.

**Input**:

- `crate_name`
- optional `version`

**Output**:

- derive macros: name, attributes accepted (best-effort)
- attribute macros: name, expected usage pattern
- function-like macros: name, signature pattern

**Implementation notes**:

- Parse `proc_macro_derive`, `proc_macro_attribute`, `proc_macro` function annotations.
- Attribute extraction is best-effort from doc comments and source patterns.
- This is inherently limited without running the macro; mark confidence accordingly.

**Acceptance**: correctly identifies which proc macros a crate exports and their names.

## Track C: Resolution & ecosystem intelligence (new tools)

### C1. `dependency.resolve` — prospective resolution check

**Priority**: High
**Rationale**: `dependency.audit` checks existing deps for problems, but agents also need to check *prospective* additions before recommending them. "Can I add `tower 0.5` alongside `axum 0.7`?" is a question that currently requires trial-and-error with `cargo add`.

**Input**:

- `dependencies`: list of `{ name, version_req }` entries (or a `cargo_toml_path` plus additions)
- optional `check_features` (include feature unification)

**Output**:

- `resolvable`: bool
- resolved version set (if resolvable)
- conflicts with explanations (if not)
- feature unification summary (if requested)

**Implementation notes**:

- Build a local resolution simulation using indexed `dependency_edges` and version data.
- This is not a full Cargo resolver — it's a best-effort compatibility check against indexed data.
- Mark confidence based on index completeness for the involved crate graph.

**Acceptance**: correctly identifies known-incompatible dependency pairs and provides actionable conflict explanations.

### C2. `crate.usage_patterns` — cross-crate usage examples

**Priority**: Medium
**Rationale**: When learning how to use a type or function, real-world usage in popular dependents is more useful than docs alone. Currently `source.search` is scoped to a single crate.

**Input**:

- `crate_name` (the crate whose API is being used)
- `symbol_name` (the type/function/trait to find usage of)
- optional `version`
- optional `limit`

**Output**:

- usage examples from indexed dependent crates' source code
- ranked by dependent popularity
- includes file path, line range, and snippet context

**Implementation notes**:

- Requires indexed source of *dependent* crates (not just the target crate).
- Cross-reference `dependency_edges` to find dependents, then `source.search` within their indexed source for the target symbol.
- Only works for dependents whose source is in the local cargo cache and has been indexed.

**Acceptance**: returns real usage snippets from popular dependents for common crate APIs.

### C3. `dependency.feature_impact` — feature flag cost analysis

**Priority**: Low
**Rationale**: Feature flags are Rust's primary knob for controlling build scope. Agents frequently enable too many or too few features. `crate.features` shows what features exist but not their cost.

**Input**:

- `crate_name`
- `version`
- `features`: list of feature names to evaluate

**Output**:

- per-feature: additional transitive dependency count, additional crates pulled in
- combined: total dependency count with vs without the selected features
- heavy features flagged (those pulling >N additional deps)

**Implementation notes**:

- Walk `dependency_edges` with feature conditions to compute the transitive closure with and without each feature.
- Requires feature-conditional dependency data to be indexed (partially available from `crate_version_features` and `dependency_edges`).

**Acceptance**: correctly identifies which features are "heavy" in terms of transitive dependency count.

### C4. `source.context` — semantic context around a location

**Priority**: Low
**Rationale**: When `symbol.search` returns a function at file:line, agents often need the surrounding context (imports, module path, containing `impl` block) to use it correctly.

**Input**:

- `crate_name`
- `path`
- `line` (or `symbol_name`)
- optional `version`

**Output**:

- module path (e.g., `tokio::sync::mpsc`)
- `use` imports in scope at that location
- containing `impl` block (if any) with its target type and trait
- surrounding type definitions

**Implementation notes**:

- Parse module tree structure from `mod` declarations.
- For a given line, walk upward through AST to find enclosing `impl`, `mod`, `fn`.
- Best-effort with `syn`; mark confidence based on parse success.

**Acceptance**: for a symbol location, returns the correct module path and enclosing context.

## Milestone plan

### M9: Confidence unification + README (Track A)

**Scope**: A1, A2
**Effort**: Small
**Risk**: Low — purely cosmetic/contract changes, no new tools.

**Exit criteria**:

- All 20 tools return structured `confidence_assessment`.
- README accurately describes the server's capabilities and operational behavior.

### M10: Type intelligence foundation (Track B core)

**Scope**: B1, B2
**Effort**: Medium-large — requires extending the `syn` parser and DB schema.
**Dependencies**: M9 (so new tools use the unified confidence contract from day one).

**Key technical work**:

- Extend `local_cache.rs` symbol extraction to capture `impl` blocks, struct fields, enum variants, and trait-impl associations.
- New migration for type-member and trait-impl relationship tables.
- Two new tool handlers + registration.

**Exit criteria**:

- `crate.type_info` returns fields/methods/trait-impls for indexed types.
- `crate.trait_impls` returns correct bidirectional lookups.
- Coverage: works for crates in local cargo cache that have been indexed via `index.refresh scope=local_cache`.

### M11: Resolution & ecosystem tools (Track C core)

**Scope**: C1, C2
**Effort**: Medium
**Dependencies**: None (can run in parallel with M10 if desired).

**Key technical work**:

- Local dependency resolution simulation over indexed `dependency_edges`.
- Cross-crate source search joining `dependency_edges` with `source_files`.

**Exit criteria**:

- `dependency.resolve` correctly identifies incompatible dependency pairs from indexed data.
- `crate.usage_patterns` returns real dependent source snippets for common APIs.

### M12: Extended intelligence (Track B + C remaining)

**Scope**: B3, B4, C3 (B5 and C4 deferred to backlog)
**Effort**: Medium
**Dependencies**: M10 (B3 and B4 build on `impl` block extraction from M10).

**Exit criteria**:

- `crate.re_exports` returns canonical import paths for straightforward re-export patterns.
- `crate.error_types` identifies error types and `From` conversion chains.
- `dependency.feature_impact` reports per-feature transitive dependency cost.

### M13: Progress streaming + operational polish (Track A remaining)

**Scope**: A3, A4
**Effort**: Small-medium
**Dependencies**: None.

**Exit criteria**:

- Long-running operations emit SSE progress events.
- `index.refresh` returns estimated completion time when historical data is available.

## Backlog (unscheduled)

Items deferred from the milestone plan, to be prioritized based on agent feedback:

- **B5** `crate.derive_macros` — proc macro discovery. Useful but inherently limited without macro expansion.
- **C4** `source.context` — semantic context around a source location. Valuable but overlaps with IDE tooling.
- **`crate.migration_path`** — breaking change analysis with fix suggestions between versions. High value but requires heuristics beyond simple API diff (correlating renames, deprecation notices, changelog parsing).
- **`crate.compatibility`** — pairwise crate compatibility matrix. Partially covered by `dependency.resolve`; dedicated tool may be warranted if resolution simulation proves insufficient.
- **Rustdoc JSON integration** — phase 3 of the symbol indexing strategy from the spec. Would provide authoritative public API data but requires building crate docs locally.
- **Rust-analyzer protocol integration** — phase 2 of symbol indexing. Semantic definitions/usages. High complexity, unclear ROI for a local MCP server vs in-editor tooling.

## Tracking and governance

- Source of truth for active work: this file.
- Retired documents (moved to `docs/archived/`):
  - `docs/roadmap-gap-closure-2026-02.md`
  - `docs/m7-implementation-checklist.md`
- Keep spec intent in `docs/agent-dependency-mcp-spec.md` and decision rationale in `docs/adr-0001-refresh-strategy.md`.
- Update this roadmap at each milestone boundary with completed items, newly discovered gaps, and scope changes.
