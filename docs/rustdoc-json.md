# Rustdoc JSON Enrichment Plan

Status: Active
Owner: rust-mcp maintainers
Last updated: 2026-02-14

## Summary

Rustdoc JSON is the primary enrichment path for rust-mcp's type intelligence. It provides authoritative, compiler-generated data about public API surfaces — resolved types, trait implementations (including blanket and auto traits), generic bounds, deprecations — at lower operational cost and higher fidelity than any alternative. Import path resolution is derived from rustdoc's re-export graph (confidence-graded, not directly authoritative — see [import path caveat](#why-rustdoc-json)).

This document is the canonical implementation plan. It supersedes the RA integration approach for batch dependency indexing (see [Relationship to other docs](#relationship-to-other-docs)).

## Why rustdoc JSON

| Requirement | syn (current) | rustdoc JSON | RA worker |
| --- | --- | --- | --- |
| Resolved types with generics | Raw text | Fully resolved | Fully resolved |
| Blanket impls (e.g. `impl<T: Display> ToString for T`) | Not visible | Native, authoritative | Explicitly excluded by `Impl::all_for_type()` |
| Auto traits (Send/Sync/Copy/Unpin) | Not visible | Native | Per-type probes, incomplete |
| Import path candidates | Not possible (single-file) | Definition paths via `ItemSummary.path` + re-export mapping via `Use` items | Depends on `SemanticsScope` (unvalidated) |
| Where-clause / generic bounds | Raw text | Fully structured | Fully structured |
| Deprecation info | Not visible | `#[deprecated]` with message/since | Possible but not standard output |
| Trait dyn-compatibility | Not visible | `is_dyn_compatible` field | Via `Trait::dyn_compatibility()` |
| Supertrait chains | Not visible | Via trait `bounds` | Via `Trait::all_supertraits()` |
| Stability of data source | `syn` is stable, published | Format versioned (`FORMAT_VERSION=57`), `rustdoc-types` crate tracks it | Unstable internal RA API, no semver |
| Requires nightly | No | Yes (`-Z unstable-options`) | No |
| Requires source code | Yes | Yes (runs `cargo rustdoc`) | Yes |

**Key insight**: rustdoc JSON is the ground truth for public API. Every validation plan for RA worker extraction was "compare against rustdoc JSON." We should just use the ground truth directly.

**Caveat on import paths**: `ItemSummary.path` is the *definition path*, not the shortest public import path. The docs explicitly state "the one chosen is implementation defined" — e.g. `HashMap` appears as `std::collections::hash::map::HashMap`, not `std::collections::HashMap`. Building correct import paths requires combining `ItemSummary.path` with `Use` items (re-exports) and the module tree to find the shortest public path. This is solvable but not "correct by definition" — it's a graph traversal over the re-export tree.

**Nightly requirement**: accepted tradeoff. Host-provided pre-generated JSON via `RUSTDOC_JSON_DIR` is the default. Container-side nightly generation is opt-in (Phase 4).

## Current state

### What exists today

| Component | Status | Location |
| --- | --- | --- |
| `RUSTDOC_JSON_DIR` config | Working | `config.rs:105-107` |
| File discovery + crate/version matching | Working | `rustdoc_json.rs:47-104` |
| Basic symbol extraction | Working | `rustdoc_json.rs:106-182` — extracts name, kind, visibility, signature, span |
| Ingestion into `symbols` table | Working | `rustdoc_json.rs:205-508` — `index_source='rustdoc_json'` |
| `index.refresh scope=rustdoc_json` | Working | `index.rs:210-217` |
| `crate.api` prefers rustdoc symbols | Working | `api_surface.rs:200-212` |
| `rustdoc-types` v0.57.0 workspace dep | Available | `Cargo.toml:30` (crates.io) |

### What's missing (the gap)

| Component | Status | Impact |
| --- | --- | --- |
| Typed deserialization via `rustdoc-types` | Not used | Current code uses raw `serde_json::Value` — loses structure, misses fields |
| Type extraction → `crate_types` table | Empty for rustdoc source | `crate.type_info` has no rustdoc data: fields, generics, variants all missing |
| Impl extraction → `crate_impls` table | Empty for rustdoc source | `crate.trait_impls` has no rustdoc data: blanket impls, auto traits, methods all missing |
| Canonical path storage | No column | `symbols` table has no `canonical_path`; re-exports not modeled |
| Trait hierarchy storage | No columns | `crate_impls` lacks `supertraits`, `dyn_compatible`, `is_auto` |
| Deprecation storage | No columns | No table captures `#[deprecated]` info |
| Container-side generation | Not implemented | No `cargo rustdoc` invocation in container |
| `rustdoc-types` in crate dep | Not yet | In workspace `Cargo.toml` but not in `crates/rust-mcp/Cargo.toml` |

## Implementation phases

### Phase 0: Foundation (typed deserialization + schema)

**Goal**: Replace raw `serde_json::Value` parsing with typed `rustdoc_types::Crate` deserialization. Add schema columns for the data rustdoc JSON provides that we can't currently store.

#### 0a. Add `rustdoc-types` as workspace dependency

**Done.** `rustdoc-types = "0.57.0"` added to workspace `Cargo.toml`. Crate dependency uses `rustdoc-types = { workspace = true }`.

> The vendored copy at `vendor/rustdoc-types/` is no longer needed for this — the crates.io published version tracks the same `FORMAT_VERSION=57` and provides identical types.

#### 0b. Schema migration: enriched type intelligence

New migration `0007_rustdoc_enrichment.sql`:

```sql
-- ============================================================
-- 1. Column additions (all ALTER TABLEs before any index/table)
-- ============================================================

-- symbols: rustdoc item identity + import paths + deprecation
ALTER TABLE symbols ADD COLUMN IF NOT EXISTS rustdoc_item_id INTEGER;
ALTER TABLE symbols ADD COLUMN IF NOT EXISTS canonical_path TEXT;
ALTER TABLE symbols ADD COLUMN IF NOT EXISTS definition_path TEXT;
ALTER TABLE symbols ADD COLUMN IF NOT EXISTS deprecated_since TEXT;
ALTER TABLE symbols ADD COLUMN IF NOT EXISTS deprecated_note TEXT;

-- crate_types: rustdoc enrichment columns
ALTER TABLE crate_types ADD COLUMN IF NOT EXISTS rustdoc_item_id INTEGER;
ALTER TABLE crate_types ADD COLUMN IF NOT EXISTS canonical_path TEXT;
ALTER TABLE crate_types ADD COLUMN IF NOT EXISTS definition_path TEXT;
ALTER TABLE crate_types ADD COLUMN IF NOT EXISTS deprecated_since TEXT;
ALTER TABLE crate_types ADD COLUMN IF NOT EXISTS deprecated_note TEXT;
ALTER TABLE crate_types ADD COLUMN IF NOT EXISTS is_non_exhaustive BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE crate_types ADD COLUMN IF NOT EXISTS auto_traits JSONB NOT NULL DEFAULT '[]'::jsonb;
ALTER TABLE crate_types ADD COLUMN IF NOT EXISTS where_clauses JSONB NOT NULL DEFAULT '[]'::jsonb;

-- crate_impls: trait metadata
ALTER TABLE crate_impls ADD COLUMN IF NOT EXISTS rustdoc_item_id INTEGER;
ALTER TABLE crate_impls ADD COLUMN IF NOT EXISTS is_blanket BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE crate_impls ADD COLUMN IF NOT EXISTS is_synthetic BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE crate_impls ADD COLUMN IF NOT EXISTS is_negative BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE crate_impls ADD COLUMN IF NOT EXISTS blanket_type TEXT;
ALTER TABLE crate_impls ADD COLUMN IF NOT EXISTS generics JSONB NOT NULL DEFAULT '[]'::jsonb;
ALTER TABLE crate_impls ADD COLUMN IF NOT EXISTS where_clauses JSONB NOT NULL DEFAULT '[]'::jsonb;

-- ============================================================
-- 2. New table
-- ============================================================

CREATE TABLE IF NOT EXISTS crate_traits (
    id BIGSERIAL PRIMARY KEY,
    crate_version_id BIGINT NOT NULL REFERENCES crate_versions(id) ON DELETE CASCADE,
    trait_name TEXT NOT NULL,
    is_auto BOOLEAN NOT NULL DEFAULT FALSE,
    is_unsafe BOOLEAN NOT NULL DEFAULT FALSE,
    is_dyn_compatible BOOLEAN NOT NULL DEFAULT FALSE,
    supertraits JSONB NOT NULL DEFAULT '[]'::jsonb,
    required_methods JSONB NOT NULL DEFAULT '[]'::jsonb,
    provided_methods JSONB NOT NULL DEFAULT '[]'::jsonb,
    associated_types JSONB NOT NULL DEFAULT '[]'::jsonb,
    generics JSONB NOT NULL DEFAULT '[]'::jsonb,
    index_source TEXT NOT NULL,
    indexed_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    rustdoc_item_id INTEGER,
    UNIQUE (crate_version_id, rustdoc_item_id, index_source)
);

-- ============================================================
-- 3. Indexes (all columns/tables now exist)
-- ============================================================

CREATE INDEX IF NOT EXISTS crate_traits_lookup_idx
    ON crate_traits (crate_version_id, trait_name);

-- Partial unique indexes: enforce one rustdoc row per item per crate version.
-- These are the authoritative identity constraint for rustdoc-sourced rows.
CREATE UNIQUE INDEX IF NOT EXISTS symbols_rustdoc_item_uniq
    ON symbols (crate_version_id, rustdoc_item_id)
    WHERE index_source = 'rustdoc_json' AND rustdoc_item_id IS NOT NULL;

CREATE UNIQUE INDEX IF NOT EXISTS crate_types_rustdoc_item_uniq
    ON crate_types (crate_version_id, rustdoc_item_id)
    WHERE index_source = 'rustdoc_json' AND rustdoc_item_id IS NOT NULL;

CREATE UNIQUE INDEX IF NOT EXISTS crate_impls_rustdoc_item_uniq
    ON crate_impls (crate_version_id, rustdoc_item_id)
    WHERE index_source = 'rustdoc_json' AND rustdoc_item_id IS NOT NULL;

CREATE UNIQUE INDEX IF NOT EXISTS crate_traits_rustdoc_item_uniq
    ON crate_traits (crate_version_id, rustdoc_item_id)
    WHERE index_source = 'rustdoc_json' AND rustdoc_item_id IS NOT NULL;

-- ============================================================
-- 4. Legacy uniqueness: relax for rustdoc rows
-- ============================================================
-- The existing UNIQUE (crate_version_id, source_file_id, type_name, kind)
-- on crate_types (0006_type_intelligence.sql:15) will collide for rustdoc
-- rows: current ingestion uses one synthetic source_file_id per JSON blob,
-- so same-name types in different modules (e.g. foo::Error vs bar::Error)
-- hit the constraint. Drop the old unique index and replace with a partial
-- index that only constrains syn rows, while rustdoc rows use
-- rustdoc_item_id as their identity.
ALTER TABLE crate_types DROP CONSTRAINT IF EXISTS crate_types_crate_version_id_source_file_id_type_name_kind_key;
CREATE UNIQUE INDEX IF NOT EXISTS crate_types_syn_uniq
    ON crate_types (crate_version_id, source_file_id, type_name, kind)
    WHERE index_source != 'rustdoc_json';
```

> **Note on legacy constraint**: The original `UNIQUE (crate_version_id, source_file_id, type_name, kind)` from `0006_type_intelligence.sql` assumed one source file per module. Rustdoc ingestion uses a single synthetic `source_file_id` for the entire JSON blob, which causes same-name types in different modules to collide. The replacement above splits the constraint: syn rows keep the original uniqueness (scoped by `WHERE index_source != 'rustdoc_json'`), while rustdoc rows use `(crate_version_id, rustdoc_item_id)` via the partial unique index above.

#### 0c. Rewrite `extract_symbols()` to use typed deserialization

Replace the raw `Value`-based extraction in `rustdoc_json.rs` with:

```rust
use rustdoc_types::Crate as RustdocCrate;

fn parse_rustdoc_json(content: &str) -> Result<RustdocCrate, serde_json::Error> {
    serde_json::from_str(content)
}
```

This gives us typed access to every field in the schema — `Item`, `ItemEnum`, `Struct`, `Enum`, `Trait`, `Impl`, `Function`, `FunctionSignature`, `Type`, `Generics`, `Visibility`, `Deprecation`, etc.

**Acceptance**: `index.refresh scope=rustdoc_json` still works. All existing symbol ingestion continues. No data loss.

### Phase 1: Deep type extraction

**Goal**: Populate `crate_types` and `crate_impls` tables from rustdoc JSON. This is the core enrichment that makes `crate.type_info` and `crate.trait_impls` dramatically better.

**Identity model**: Each rustdoc `Item` has an `Id(u32)` that is unique within a single JSON blob. During ingestion, store this as `rustdoc_item_id` on enriched table rows. Use `(crate_version_id, rustdoc_item_id)` as the deterministic key for updates/upserts — enforced by partial unique indexes (`WHERE index_source = 'rustdoc_json'`) on `symbols`, `crate_types`, `crate_impls`, and `crate_traits`. For cross-referencing between tables (e.g. impl → type), resolve `Id` references against `krate.index` during extraction and store the resolved names/paths — do not store raw `Id` values, as they are not stable across regeneration.

#### 1a. Extract types (structs, enums, unions, type aliases)

For each `Item` where `inner` is `Struct`, `Enum`, `Union`, or `TypeAlias`:

| Rustdoc JSON field | Maps to |
| --- | --- |
| `item.id` | `crate_types.rustdoc_item_id` |
| `item.name` | `crate_types.type_name` |
| `inner` variant name | `crate_types.kind` |
| `item.visibility` | `crate_types.visibility` |
| `inner.struct.generics.params` | `crate_types.generic_params` (JSONB) |
| `inner.struct.kind.plain.fields` → resolve each `Id` | `crate_types.fields` (JSONB: `[{name, type, visibility}]`) |
| `inner.enum.variants` → resolve each `Id` | `crate_types.variants` (JSONB: `[{name, kind, fields, discriminant}]`) |
| `paths[id].path` | `crate_types.canonical_path` |
| `item.deprecation` | `crate_types.deprecated_since`, `deprecated_note` |
| `item.attrs` contains `NonExhaustive` | `crate_types.is_non_exhaustive` |
| `inner.struct.generics.where_predicates` | `crate_types.where_clauses` (JSONB) |
| `item.span` | `crate_types.start_line`, `end_line` |

Field type resolution uses the `Type` enum — render to display string using a recursive formatter (e.g. `Type::ResolvedPath` → `"Vec<String>"`, `Type::BorrowedRef` → `"&mut T"`).

#### 1b. Extract implementations

For each `Item` where `inner` is `Impl`:

| Rustdoc JSON field | Maps to |
| --- | --- |
| `item.id` | `crate_impls.rustdoc_item_id` |
| `inner.impl.for_` → render Type | `crate_impls.type_name` |
| `inner.impl.trait_.path` | `crate_impls.trait_name` (None = inherent) |
| `inner.impl.items` → resolve Function items | `crate_impls.methods` (JSONB) |
| `inner.impl.blanket_impl` | `crate_impls.is_blanket`, `blanket_type` |
| `inner.impl.is_synthetic` | `crate_impls.is_synthetic` |
| `inner.impl.is_negative` | `crate_impls.is_negative` |
| `inner.impl.generics` | `crate_impls.generics` (JSONB) |
| `inner.impl.generics.where_predicates` | `crate_impls.where_clauses` (JSONB) |
| presence of `trait_` | `crate_impls.impl_kind` (inherent/trait/derive) |

For each resolved method `Function` item within an impl:

```json
{
  "name": "from",
  "signature": "fn from(s: String) -> Self",
  "has_self": false,
  "is_const": false,
  "is_async": false,
  "is_unsafe": false,
  "params": [{"name": "s", "type": "String"}],
  "return_type": "Self",
  "visibility": "public"
}
```

#### 1c. Extract trait definitions

For each `Item` where `inner` is `Trait`:

| Rustdoc JSON field | Maps to |
| --- | --- |
| `item.id` | `crate_traits.rustdoc_item_id` |
| `item.name` | `crate_traits.trait_name` |
| `inner.trait.is_auto` | `crate_traits.is_auto` |
| `inner.trait.is_unsafe` | `crate_traits.is_unsafe` |
| `inner.trait.is_dyn_compatible` | `crate_traits.is_dyn_compatible` |
| `inner.trait.bounds` → render trait bounds | `crate_traits.supertraits` (JSONB) |
| `inner.trait.items` → partition by has_body | `required_methods`, `provided_methods` |
| `inner.trait.items` → filter AssocType | `associated_types` |
| `inner.trait.generics` | `crate_traits.generics` (JSONB) |

#### 1d. Import path resolution

The `Crate.paths` field maps every `Id` to an `ItemSummary` containing the definition path. `Use` items in the index represent re-exports:

**Important**: `ItemSummary.path` gives the *definition path*, not necessarily the shortest public import path. To find the best import path for each item:

1. Build a re-export graph from `Use` items in the index (items where `inner` is `ItemEnum::Use`)
2. For each `Use`, record `source` (original path), `name` (re-exported name), and `id` (target item)
3. Walk the module tree from the crate root, collecting all public paths that resolve to each item ID
4. Choose the shortest public path as the "canonical" import path
5. Store the definition path (from `ItemSummary`) alongside for provenance

```rust
// Step 1: collect definition paths from ItemSummary
let mut def_paths: HashMap<Id, String> = krate.paths.iter()
    .map(|(id, summary)| (*id, summary.path.join("::")))
    .collect();

// Step 2: collect re-export paths from Use items
let mut reexport_paths: HashMap<Id, Vec<String>> = HashMap::new();
for item in krate.index.values() {
    if let ItemEnum::Use(use_) = &item.inner {
        if let Some(target_id) = use_.id {
            // This Use re-exports target_id under a (possibly shorter) path
            // Build the full re-export path from the Use item's parent module
            reexport_paths.entry(target_id).or_default().push(/* ... */);
        }
    }
}

// Step 3: for each item, pick shortest public path
```

This is a graph traversal, not a simple lookup. Confidence should reflect whether re-export resolution succeeded.

**Acceptance**:

- `crate.type_info` returns resolved field types, generic params, trait impls including blanket impls for rustdoc-indexed crates
- `crate.trait_impls` returns blanket impls, auto traits, where-clauses for rustdoc-indexed crates
- Confidence is "high" when rustdoc data is present, "low" for syn-only

### Phase 2: Tool enrichment

**Goal**: Update existing tools to prefer rustdoc JSON data and add new tools enabled by the richer data.

#### 2a. `crate.type_info` enrichment

Update query to prefer `index_source='rustdoc_json'` rows from `crate_types` and `crate_impls`. When both syn and rustdoc rows exist for the same type, use rustdoc. Add to response:

- `auto_traits`: `["Send", "Sync", "Copy", "Unpin"]` (from synthetic impls)
- `where_clauses`: structured generic bounds
- `import_path`: best-known public import path (from re-export resolution; confidence-graded)
- `deprecation`: `{since, note}` if present
- `is_non_exhaustive`: boolean

#### 2b. `crate.trait_impls` enrichment

Add to response:

- `blanket_impls`: list with `blanket_type` (e.g. `"T"` for `impl<T: Display> ToString for T`)
- `auto_trait_impls`: Send/Sync/Unpin (from synthetic impls)
- `where_clauses`: per-impl generic bounds
- `is_negative`: negative impl marker

#### 2c. `crate.re_exports` enrichment

Replace syn-based `pub use` parsing with rustdoc JSON re-export graph. Build the re-export mapping from `Use` items in the index (each has `source`, `name`, `id`). Items reachable via a shorter public path than their definition path are re-exports. Store both the definition path (from `ItemSummary.path`) and the shortest public path.

#### 2d. `crate.import_path` (new tool, replaces `ra.import_path`)

Given a symbol name + crate, return the best-known public import path from the re-export graph built in Phase 1d.

**Input**: `crate_name`, `symbol_name`, optional `version`

**Output**: `import_path`, `definition_path`, `is_re_export`, `confidence` (high if re-export resolution succeeded, medium if only definition path available)

This directly addresses the #1 agent failure mode: hallucinated import paths.

#### 2e. `source.context` enrichment

When reading source and encountering a type name, cross-reference against indexed rustdoc data. Add to `source.context` response:

- `resolved_types`: map of type names found in source → their best-known import paths (from rustdoc re-export resolution)
- `candidate_traits`: traits implemented by types found nearby (from `crate_impls` data), not semantic scope — this is a heuristic, not RA-style scope resolution

### Phase 3: New tools

**Goal**: Tools uniquely enabled by having structured rustdoc JSON across versions.

#### 3a. `crate.api_diff` upgrade (existing tool)

Upgrade the existing `crate.api_diff` tool to use rustdoc JSON data instead of syn-level symbol comparison. Uses the same data already indexed in Phase 1.

**Input**: `crate_name`, `from_version`, `to_version`

**Output**:

- `added`: items present in `to` but not `from`
- `removed`: items present in `from` but not `to`
- `changed`: items with different signatures, bounds, or fields
- `deprecations_added`: newly deprecated items
- `breaking_changes`: removed public items, changed signatures, removed trait impls

This upgrades the existing `crate.api_diff` implementation (which currently uses syn-level symbol comparison) with compiler-authoritative data. It also strengthens the foundation for `crate.migration_path`.

#### 3b. `crate.deprecations` (new tool)

**Input**: `crate_name`, optional `version`

**Output**: all items with `#[deprecated]` — name, kind, `since`, `note`, replacement hint (parsed from note).

Trivial to implement once Phase 1 stores deprecation data.

#### 3c. `crate.error_types` enrichment

Use rustdoc JSON to:

- Find types implementing `std::error::Error` (from impl data)
- Extract `From<X>` impl chains for error conversion graphs
- Identify functions returning `Result<_, ErrorType>`
- Extract `Display` impl (from associated method signatures)

#### 3d. `crate.feature_api` (new tool, P3)

Generate rustdoc JSON with different feature flag combinations to show what API surface each feature enables.

**Input**: `crate_name`, `version`, `features` (list)

**Output**: items gated behind the specified features

Requires container-side `cargo rustdoc` with `--features` flag. Higher operational cost — only viable when container-side generation is available (Phase 4).

### Phase 4: Container-side generation (optional)

**Goal**: Generate rustdoc JSON inside the container for crates in the cargo registry cache, eliminating the dependency on host-provided JSON files.

#### 4a. Install nightly in container

Add to Dockerfile:

```dockerfile
RUN rustup toolchain install nightly --profile minimal
```

#### 4b. Generation pipeline

On `index.refresh scope=rustdoc_json` when `RUSTDOC_JSON_DIR` is not set or a specific crate is requested:

1. Locate crate source in cargo registry cache
2. Run `cargo +nightly rustdoc -p <crate> --offline -- --output-format json -Z unstable-options`
3. Parse output from `target/doc/<crate>.json`
4. Ingest via the Phase 1 pipeline

**Constraints**:

- Rate-limited: one generation at a time
- Timeout: 120s per crate (most complete in <30s)
- Disk: clean up `target/` after generation
- Network: `--offline` flag enforced — no network access. If transitive dependencies are missing from the local registry cache, generation fails gracefully (expected for incomplete caches). Fall back to `RUSTDOC_JSON_DIR` or syn-only data.
- Failure budget: expect 10-30% of crates to fail generation due to incomplete deps, broken build scripts, or missing native libraries. This is acceptable — syn remains the baseline.

#### 4c. Sysroot for container generation

The MCP client provides the sysroot path for the active workspace toolchain (existing design from ra-integration.md). For container-side generation, nightly's own sysroot is used:

```bash
cargo +nightly rustdoc ...
# Uses nightly's bundled std library source
```

No host sysroot mount needed for rustdoc generation specifically — nightly ships its own `rust-src`.

## Type rendering

Rustdoc JSON uses the `Type` enum with 14 variants. We need a renderer to produce human-readable type strings for storage in JSONB fields.

```rust
fn render_type(ty: &rustdoc_types::Type, krate: &rustdoc_types::Crate) -> String {
    match ty {
        Type::ResolvedPath(path) => {
            let mut s = path.path.clone();
            if let Some(args) = &path.args {
                s.push_str(&render_generic_args(args, krate));
            }
            s
        }
        Type::Generic(name) => name.clone(),
        Type::Primitive(name) => name.clone(),
        Type::BorrowedRef { lifetime, is_mutable, type_ } => {
            let mut s = String::from("&");
            if let Some(lt) = lifetime { s.push_str(lt); s.push(' '); }
            if *is_mutable { s.push_str("mut "); }
            s.push_str(&render_type(type_, krate));
            s
        }
        Type::Tuple(types) => {
            let inner: Vec<_> = types.iter().map(|t| render_type(t, krate)).collect();
            format!("({})", inner.join(", "))
        }
        Type::Slice(ty) => format!("[{}]", render_type(ty, krate)),
        Type::Array { type_, len } => format!("[{}; {}]", render_type(type_, krate), len),
        Type::RawPointer { is_mutable, type_ } => {
            let qual = if *is_mutable { "mut" } else { "const" };
            format!("*{} {}", qual, render_type(type_, krate))
        }
        Type::ImplTrait(bounds) => {
            let bs: Vec<_> = bounds.iter().map(|b| render_bound(b, krate)).collect();
            format!("impl {}", bs.join(" + "))
        }
        Type::DynTrait(dyn_trait) => {
            let ts: Vec<_> = dyn_trait.traits.iter()
                .map(|pt| render_path(&pt.trait_, krate))
                .collect();
            format!("dyn {}", ts.join(" + "))
        }
        Type::FunctionPointer(fp) => render_fn_pointer(fp, krate),
        Type::QualifiedPath { name, self_type, trait_, .. } => {
            let self_str = render_type(self_type, krate);
            match trait_ {
                Some(t) => format!("<{} as {}>::{}", self_str, render_path(t, krate), name),
                None => format!("{}::{}", self_str, name),
            }
        }
        Type::Infer => "_".to_string(),
        Type::Pat { type_, .. } => render_type(type_, krate),
    }
}
```

## Priority summary

| Phase | Priority | What | Impact |
| --- | --- | --- | --- |
| 0a | P0 | Add `rustdoc-types` dependency | Enables typed deserialization |
| 0b | P0 | Schema migration | Enables storage of enriched data |
| 0c | P0 | Rewrite extraction to typed | Correctness, access to all fields |
| 1a | P1 | Type extraction → `crate_types` | `crate.type_info` gets resolved fields, generics, variants |
| 1b | P1 | Impl extraction → `crate_impls` | `crate.trait_impls` gets blanket impls, auto traits, methods |
| 1c | P1 | Trait extraction → `crate_traits` | Supertrait chains, dyn-compatibility, required methods |
| 1d | P1 | Import path resolution | Re-export graph traversal for best public import paths |
| 2a | P1 | `crate.type_info` enrichment | Confidence jump: low → high for rustdoc-indexed crates |
| 2b | P1 | `crate.trait_impls` enrichment | Blanket/auto trait coverage |
| 2c | P1 | `crate.re_exports` enrichment | Correct import paths |
| 2d | P1 | `crate.import_path` new tool | Direct answer to #1 agent failure mode |
| 2e | P2 | `source.context` enrichment | Cross-reference type names → import paths, candidate traits |
| 3a | P2 | `crate.api_diff` upgrade (existing) | Compiler-authoritative API diffing |
| 3b | P2 | `crate.deprecations` new tool | Deprecation visibility |
| 3c | P2 | `crate.error_types` enrichment | Error conversion graph from impl data |
| 3d | P3 | `crate.feature_api` new tool | Feature-gated API surface |
| 4 | P3 | Container-side generation | Eliminates host-provided JSON dependency |

## Relationship to other docs

- **ra-integration.md**: Retained as reference for potential future LSP integration (user-workspace positional intelligence). Interactive `ra.*` tools remain deferred.
- **ra-worker.md**: Retained as reference design. The worker approach may be revisited if rustdoc JSON proves insufficient for a specific extraction target (e.g. private item analysis, macro expansion). For public API indexing, rustdoc JSON is strictly superior.
- **roadmap-2026-02.md**: This plan implements B1 (`crate.type_info`), B2 (`crate.trait_impls`), B3 (`crate.re_exports`), and B4 (`crate.error_types`) via rustdoc JSON rather than syn-only extraction. It also adds new tools (`crate.import_path`, `crate.deprecations`, `crate.feature_api`) and upgrades `crate.api_diff` to use rustdoc JSON data.
- **agent-dependency-mcp-spec.md**: Phase 3 of the spec's semantic indexing strategy ("optional rustdoc JSON integration") is promoted to primary enrichment path.
