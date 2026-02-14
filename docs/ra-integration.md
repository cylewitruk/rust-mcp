# rust-analyzer Integration Design

Status: Draft
Owner: rust-mcp maintainers
Last updated: 2026-02-14

## Summary table

| Item | Type | Description | Priority | Status |
|------|------|-------------|----------|--------|
| `ra.type_at` | New tool | Inferred type at file:line:col in indexed crate source | P1 | Planned |
| `ra.definition` | New tool | Go-to-definition across module boundaries within a crate | P1 | Planned |
| `ra.completions` | New tool | Method/field completions on a type at a location | P1 | Planned |
| `ra.references` | New tool | Find all usages of a symbol within a crate | P2 | Planned |
| `ra.diagnostics` | New tool | Semantic diagnostics for a crate version (no cargo build) | P2 | Planned |
| `ra.expand_macro` | New tool | Expand a macro invocation and return generated code | P3 | Planned |
| `ra.import_path` | New tool | Canonical import path for a symbol from crate root | P2 | Planned |
| `source.context` | Enrichment | Upgrade from syn AST walk to RA-backed scope/import resolution | P1 | Planned |
| `crate.type_info` | Enrichment | Add resolved trait bounds, auto traits, blanket impl coverage | P1 | Planned |
| `crate.trait_impls` | Enrichment | Add blanket impls, where-clause resolution, auto trait detection | P1 | Planned |
| `crate.re_exports` | Enrichment | Replace syntactic `pub use` parsing with semantic path resolution | P2 | Planned |
| `crate.error_types` | Enrichment | Add `?`-propagation chain analysis via RA diagnostics | P3 | Planned |
| `crate.migration_path` | Enrichment | Ground breakage detection in RA diagnostics, not just API diff | P3 | Planned |
| RA lifecycle service | Infrastructure | Managed RA subprocess pool with startup, caching, timeouts | P0 | Planned |
| Sysroot provisioning | Infrastructure | Ship minimal sysroot (std metadata) in container image | P0 | Planned |

## Motivation

The server currently uses three indexing strategies with increasing authority:

1. **syn-based extraction** (`local_cache.rs`) — parses individual `.rs` files in isolation. Captures top-level symbols, struct fields, enum variants, impl blocks, trait impl associations, and derive macro attributes. Single-file scope; no cross-module resolution, no type inference, no macro expansion.

2. **Rustdoc JSON ingestion** (`index.refresh scope=rustdoc_json`) — imports pre-built rustdoc JSON for authoritative public API data. Requires the user to have generated the JSON locally. Covers public items only.

3. **Text-based signature snapshots** — signatures are stored as the literal source line (truncated to 240 chars), not as parsed AST. Method parameters and return types are opaque strings.

This produces a specific class of failures that agents encounter:

- **Wrong method signatures**: `crate.type_info` returns methods as text lines from source. If a method spans multiple lines, the signature is truncated or incomplete. Agents then hallucinate the missing parts.
- **Missing blanket/auto trait impls**: `crate.trait_impls` can only see explicitly written `impl Trait for Type` blocks and `#[derive(...)]` attributes. It cannot report that `Vec<u8>` implements `Send` (auto trait) or that anything implementing `Display` also implements `ToString` (blanket impl).
- **Incorrect import paths**: `crate.re_exports` parses `pub use` statements syntactically but cannot resolve glob re-exports (`pub use crate::internal::*`) or conditional re-exports (`#[cfg(feature = "...")]`).
- **No type inference**: there is no way to ask "what is the type of this variable at line 42?" — a question agents need answered constantly when reading unfamiliar code.
- **No macro expansion**: derive macros are detected by attribute name, but their generated code (impl blocks, methods, trait impls) is invisible to the index.

rust-analyzer (RA) can solve all of these because it performs full semantic analysis: name resolution, type inference, trait solving, and macro expansion. The question is whether the operational cost is justified and how to scope it.

## GPT proposal review and corrections

The original GPT analysis is directionally correct but has several issues:

### What GPT got right

- RA would materially improve `source.context`, `crate.type_info`, `crate.trait_impls`, and `crate.re_exports` — these are the tools most limited by single-file syn parsing.
- The proposed `ra.type_at`, `ra.references`, `ra.definition`, and `ra.expand_macro` tools target real agent pain points.
- Operational boundaries (optional enablement, timeouts, memory caps, provenance tagging) are necessary.

### What GPT got wrong or understated

**1. Scoping confusion — dependency analysis vs project analysis**

GPT conflates two distinct use cases:

- **Analyzing dependency crate source** (what this server does): RA examines the source of e.g. `tokio 1.48` in the cargo cache to index its types, impls, and API surface.
- **Analyzing the user's project** (what an IDE does): RA examines the user's workspace code to find errors, suggest refactors, etc.

This server deliberately does not touch the user's project. Tools like `ra.diagnostics` and `crate.migration_path` enrichment only make sense if scoped to analyzing cached library source, not the user's workspace. GPT's framing of "edit exactly what is affected" implies project-level analysis which is out of scope.

**Correction**: RA integration should be scoped exclusively to analyzing dependency crate source in `/cargo/registry/src/`. The unit of analysis is one crate version, not a user workspace.

**2. Resource cost is severely understated**

GPT says "strict timeouts + memory caps" without quantifying the problem:

- RA analyzing a single moderately complex crate (e.g., `tokio` with features) can consume **2–8 GB of RAM** and take **30–120 seconds** for initial analysis.
- RA is designed for persistent, incremental editing sessions. Using it as a batch query tool means paying full startup cost per crate version unless we cache the analysis state.
- The current container runs lean (~200 MB RSS for the server + embedded Postgres). Adding RA analysis could 10x peak memory.
- The host machine is also running the user's IDE, likely with its own RA instance. Two concurrent RA processes on a laptop is painful.

**Correction**: RA integration must either (a) pre-compute and cache analysis results at index time, not query time, or (b) use a lightweight on-demand model with aggressive per-crate memory budgets and analysis timeouts.

**3. Missing the sysroot dependency**

RA requires a Rust sysroot (at minimum, metadata for `std`, `core`, `alloc`) to resolve standard library types. The runtime container has no Rust toolchain. GPT doesn't mention this.

**Correction**: Either ship a minimal sysroot in the container image (metadata only, ~50 MB), or mount the host's sysroot. This is a hard prerequisite.

**4. Version multiplexing problem**

This server analyzes many crate versions simultaneously. RA's project model assumes one version of each crate in a workspace. Analyzing `serde 1.0.220` and `serde 1.0.228` requires two separate RA sessions or careful VFS manipulation.

**Correction**: The RA integration layer must manage per-crate-version analysis isolation.

**5. `ra.diagnostics` value is overstated for this use case**

GPT frames `ra.diagnostics` as "compile/lint-like semantic diagnostics without full cargo build loops." But this server isn't building user code — it's indexing library source. Diagnostics on library crate source tell you about the library's internal code quality, not about how the user's code interacts with it.

**Correction**: Downgrade `ra.diagnostics` priority. It's useful for quality assessment (does this crate have internal type errors?) but is not the high-value agent workflow GPT implies.

**6. `ra.references` scope is ambiguous and expensive**

"Find all references to symbol X" within a single crate is a bounded, useful operation. But GPT's framing implies cross-crate reference finding (find all *dependents* that use symbol X), which would require RA to analyze every dependent — prohibitively expensive.

**Correction**: `ra.references` should be scoped to intra-crate references only. Cross-crate usage is already handled by `crate.usage_patterns` via text search.

### What GPT missed entirely

**7. `ra.completions` — the highest-value new tool**

GPT doesn't propose a completions tool, but this is arguably the single most useful RA capability for agents. Given a crate, a type, and a position context, "what methods and fields are available here?" is the question agents answer incorrectly most often. RA's completion engine handles method resolution through trait imports, auto-deref chains, and generic bounds — none of which syn can do.

**8. Alternative: `cargo doc --document-private-items` JSON**

For many of the enrichments GPT proposes (resolved types, trait impls including blanket/auto, canonical paths), rustdoc JSON already provides the data with much lower operational cost than RA. The server already has rustdoc JSON bootstrap. Deepening that integration covers 60-70% of the RA value proposition at a fraction of the complexity.

**9. Cargo registry source does not include dependency source**

The mounted cargo cache (`~/.cargo/registry/src/`) contains extracted source for crates the user has previously built. But RA needs the *dependencies of the crate being analyzed* to resolve types. If you're analyzing `axum`, RA needs access to `tower`, `http`, `hyper`, etc. These may or may not be in the cache.

**Correction**: Either accept that RA analysis only works for crates whose full dependency trees are cached (likely true for actively-used crates), or implement on-demand source fetching for transitive dependencies.

## Architecture

### Deployment model

RA runs as a managed subprocess inside the container, communicating via LSP over stdio. The MCP server owns the RA lifecycle.

```
┌─────────────────────────────────────────────────────┐
│  Docker container                                   │
│                                                     │
│  ┌──────────────┐   LSP/stdio   ┌───────────────┐  │
│  │  rust-mcp    │◄─────────────►│ rust-analyzer  │  │
│  │  (main proc) │               │ (subprocess)   │  │
│  └──────┬───────┘               └───────┬────────┘  │
│         │ SQL                           │ read      │
│  ┌──────▼───────┐               ┌───────▼────────┐  │
│  │  PostgreSQL  │               │ /cargo/registry │  │
│  │  (embedded)  │               │ (ro mount)      │  │
│  └──────────────┘               └────────────────┘  │
└─────────────────────────────────────────────────────┘
```

**Why subprocess, not sidecar or host bridge:**

- Subprocess keeps the single-container deployment model that defines this project.
- Host bridge (querying the user's RA instance) would create coupling to the user's IDE state, editor-specific LSP client behavior, and version skew. It also can't analyze crate versions that differ from what the user has in their lock file.
- Sidecar adds orchestration complexity for no benefit over a managed subprocess.

### Container image changes

The Dockerfile runtime stage would need:

```dockerfile
# RA integration layer (optional, behind build arg)
ARG ENABLE_RA=false
RUN if [ "$ENABLE_RA" = "true" ]; then \
      apk add --no-cache rustup && \
      rustup-init -y --default-toolchain stable --profile minimal --no-modify-path && \
      ~/.cargo/bin/rustup component add rust-analyzer rust-src && \
      ln -s ~/.cargo/bin/rust-analyzer /usr/local/bin/rust-analyzer; \
    fi
```

**Image size impact**: ~400–500 MB additional (toolchain + rust-src + rust-analyzer binary). This should be an opt-in build variant, not the default image.

**Sysroot**: The `rust-src` component provides the sysroot metadata RA needs. No additional provisioning required if the toolchain is present.

### RA session management

The core challenge is that RA is designed for long-running editor sessions, not batch queries. We need a session management layer.

**Proposed model: per-crate-version cached sessions**

```
                    ┌────────────────────────────┐
                    │      RA Session Pool        │
                    │                             │
   analyze(tokio)──►│  tokio@1.48 ──► RA proc 1  │
                    │  serde@1.0  ──► RA proc 2  │
   analyze(serde)──►│  (LRU evict)               │
                    └────────────────────────────┘
```

- On first query for a crate version, spawn an RA subprocess pointed at a synthetic workspace rooted at the crate's source directory in `/cargo/registry/src/`.
- Keep the session alive for a configurable TTL (e.g., 5 minutes) to amortize startup cost across multiple queries for the same crate.
- LRU-evict sessions when pool size exceeds a cap (e.g., 3 concurrent sessions).
- Each session has a hard memory limit (e.g., 2 GB via cgroups or OOM score adjustment) and a per-request timeout (e.g., 30 seconds).

**Synthetic workspace construction**:

For RA to analyze a crate, it needs a `Cargo.toml` at the workspace root. The cargo cache already contains one per crate version. However, RA also needs the *dependency source* to resolve types. The cargo cache structure (`registry/src/index.crates.io-*/<crate>-<version>/`) places all crate sources as siblings, so RA can find transitive deps if they're present.

The tricky part: RA normally uses `cargo metadata` to discover the dependency graph, which requires `cargo` and network access. For cached analysis, we'd need to either:

1. **Generate a synthetic `cargo metadata` JSON** from our indexed `dependency_edges` table and feed it to RA via `rust-analyzer.cargo.buildScripts.overrideCommand` / `rust-project.json`.
2. **Use `rust-project.json`** (RA's non-Cargo project format) to manually specify crate roots and dependency relationships from our DB.

Option 2 (`rust-project.json`) is more reliable and avoids the cargo dependency entirely.

### Query flow

```
Agent calls crate.type_info(tokio, Router)
  │
  ▼
MCP handler checks: is RA enabled?
  │
  ├─ No: fall back to current syn/rustdoc index (existing behavior)
  │
  ├─ Yes: check RA session pool for tokio@latest
  │    │
  │    ├─ Session exists and healthy: send LSP request
  │    │
  │    └─ No session: construct rust-project.json, spawn RA, wait for initialization
  │         │
  │         └─ Send LSP request (textDocument/hover, textDocument/completion, etc.)
  │
  ▼
Merge RA results with indexed data
  │
  ▼
Return response with provenance: "rust_analyzer" or "syn+rust_analyzer"
```

**Fallback is mandatory**: RA may fail to analyze a crate (missing deps, parse errors, timeout). Every RA-enriched tool must degrade gracefully to the current syn/rustdoc behavior with a confidence downgrade and a note in the response.

## New tools

### `ra.type_at`

**Priority**: P1

Infers the type of an expression at a specific location in crate source. Prevents agent hallucinations about variable types, return types, and closure parameter types.

**Input**:
- `crate_name`, optional `version`
- `path` (source file within the crate)
- `line`, `column`

**Output**:
- `inferred_type`: fully qualified type string
- `type_display`: human-readable short form
- `generic_args`: resolved generic parameters (if applicable)
- `provenance`: `"rust_analyzer"`
- `confidence_assessment`: high if RA resolved, low on fallback

**LSP mapping**: `textDocument/hover` at the specified position, extract type from hover content.

**Agent value**: when reading dependency source via `source.read`, agents can now ask "what type is `self.inner` on line 47?" and get a compiler-verified answer instead of guessing from context.

### `ra.definition`

**Priority**: P1

Resolves go-to-definition across module boundaries within a crate. Replaces text-based grep for "where is this symbol actually defined?"

**Input**:
- `crate_name`, optional `version`
- `path`, `line`, `column`

**Output**:
- `definition_path`: file path within the crate
- `definition_line`, `definition_column`
- `definition_kind`: function/struct/trait/etc.
- `definition_signature`: full signature from RA

**LSP mapping**: `textDocument/definition`.

**Agent value**: when `symbol.search` returns a re-exported symbol, the agent can follow to its actual definition site. More reliable than text search for common names like `new`, `build`, `default`.

### `ra.completions`

**Priority**: P1

Returns available methods, fields, and associated items on a type at a given position. This is the single highest-impact RA tool because it answers "what can I do with this value?" with compiler authority.

**Input**:
- `crate_name`, optional `version`
- `path`, `line`, `column`
- optional `trigger_character` (`.`, `::`, etc.)
- optional `limit`

**Output**:
- list of completion items, each with:
  - `label`: method/field name
  - `kind`: method/field/function/const/etc.
  - `detail`: full signature
  - `documentation`: doc comment (if available)
  - `insert_text`: what would be inserted (useful for generic parameters)

**LSP mapping**: `textDocument/completion`.

**Agent value**: given a `Router` value, the agent can ask "what methods are available?" and get the definitive list including methods from trait imports, auto-deref chains, and blanket impls — none of which syn can provide. This directly prevents the most common Rust agent error: calling methods that don't exist or using wrong signatures.

### `ra.references`

**Priority**: P2

Finds all usages of a symbol within a single crate. Scoped to intra-crate references only — cross-crate usage is handled by `crate.usage_patterns`.

**Input**:
- `crate_name`, optional `version`
- `path`, `line`, `column`
- optional `include_declaration` (default false)

**Output**:
- list of `(path, line, column, context_snippet)` reference sites

**LSP mapping**: `textDocument/references`.

**Agent value**: understanding how a type or function is used *within its own crate* helps agents understand patterns and conventions. Useful for "how does this crate use its own error type?" or "where is this internal helper called from?"

### `ra.diagnostics`

**Priority**: P2

Returns semantic diagnostics for a crate version's source. Not a build — RA produces diagnostics from its own analysis engine.

**Input**:
- `crate_name`, optional `version`
- optional `path` (scope to one file)
- optional `severity_filter` (`error`, `warning`)

**Output**:
- list of diagnostics: path, line, severity, message, code
- summary counts by severity

**LSP mapping**: `textDocument/publishDiagnostics` (collected after workspace load).

**Agent value**: primarily useful for quality assessment of a dependency ("does this crate have unresolved type errors?") and for `crate.migration_path` enrichment (see below). Not as high-value as GPT implied since we're analyzing library source, not user code.

### `ra.import_path`

**Priority**: P2

Returns the canonical public import path for a symbol, accounting for re-exports, glob re-exports, and `#[doc(hidden)]` items.

**Input**:
- `crate_name`, optional `version`
- `symbol_name`
- optional `kind` filter

**Output**:
- list of valid import paths, ranked by canonicality (shortest public path first)
- `is_public`: whether each path is accessible from outside the crate
- `is_deprecated`: if the path traverses deprecated modules

**LSP mapping**: custom RA extension `rust-analyzer/resolveImport` or workspace symbol search + path resolution.

**Agent value**: directly fixes the incorrect-import-path problem. When an agent needs to use `tokio::sync::mpsc::Sender`, this tool confirms that's the right path vs `tokio::sync::mpsc::bounded::Sender` (internal) or some other re-export. Supersedes much of what `crate.re_exports` does with syn-only parsing.

### `ra.expand_macro`

**Priority**: P3

Expands a macro invocation and returns the generated code. Critical for understanding derive macros and proc macros.

**Input**:
- `crate_name`, optional `version`
- `path`, `line`, `column` (position of the macro invocation or derive attribute)

**Output**:
- `expanded_code`: the generated Rust source
- `macro_name`: which macro was expanded
- `expansion_kind`: derive/attribute/function-like

**LSP mapping**: `rust-analyzer/expandMacro`.

**Agent value**: when a struct has `#[derive(Serialize, Deserialize)]`, the agent can see exactly what code serde generates — the `impl Serialize for Foo` block with all its methods. This eliminates the "invisible code" problem for proc-macro-heavy crates. Lower priority because the common derives (Debug, Clone, Serialize, etc.) are well-known, but valuable for custom proc macros.

## Existing tool enrichments

### `source.context` (P1)

**Current state**: syn-based AST walk to find enclosing `impl`/`mod`/`fn` + heuristic import scanning within the same file.

**With RA**: exact scope resolution, resolved import paths, inferred types for local variables, full module path from crate root. Uses `textDocument/hover` + `textDocument/documentSymbol` at the target location.

**Fallback**: current syn-based behavior when RA is unavailable.

### `crate.type_info` (P1)

**Current state**: struct fields, enum variants, inherent methods (text signatures), and explicitly-written trait impls from syn. Missing: auto traits (`Send`, `Sync`, `Unpin`), blanket impls, resolved generic bounds, method return types as structured data.

**With RA**: full method resolution including methods available through trait imports and auto-deref, auto trait detection, blanket impl enumeration, and structured parameter/return types.

**Approach**: for an indexed type, use `textDocument/completion` with a synthetic `.` trigger to discover all available methods, then `textDocument/hover` on each to get full signatures and trait provenance.

### `crate.trait_impls` (P1)

**Current state**: explicit `impl Trait for Type` blocks + derive attribute inference. Cannot detect blanket impls, conditional impls with where clauses, or auto traits.

**With RA**: RA's trait solver knows all impls including blanket, auto, and conditionally-bounded ones. Use RA's `rust-analyzer/viewItemTree` or hover-based trait resolution.

**Limitation**: RA still can't enumerate "all types that implement trait X" across a whole crate efficiently. The bidirectional "trait → types" direction remains best-effort.

### `crate.re_exports` (P2)

**Current state**: syntactic `pub use` parsing. Fails on glob re-exports, `#[cfg]`-gated re-exports, and complex path aliases.

**With RA**: replaced entirely by `ra.import_path` for individual symbol lookups. Bulk re-export mapping can use RA's workspace symbol index to build the complete path map.

### `crate.error_types` (P3)

**Current state**: heuristic detection of `*Error` types, `From<X>` impl extraction via syn.

**With RA**: add `?` operator propagation chain analysis — RA can trace which error types flow through `?` in a function body, giving agents the complete "what errors can this function return?" answer.

### `crate.migration_path` (P3)

**Current state**: API diff between versions + basic rename-candidate hints from added/removed symbol pairs.

**With RA**: analyze the *old version's* public API against the *new version's* source to identify concrete breakage sites (type mismatches, missing trait impls, changed generics). This is the most complex enrichment because it requires RA to analyze cross-version relationships.

**Approach**: generate synthetic "usage" code that exercises the old API, then run RA diagnostics against the new version to find what breaks. Expensive but high value for major version upgrades.

## Operational boundaries

### Resource limits

| Resource | Limit | Rationale |
|----------|-------|-----------|
| Max concurrent RA sessions | 3 | Each session uses 1–4 GB RAM; 3 sessions cap peak at ~12 GB |
| Per-session memory cap | 4 GB | Kill RA process if RSS exceeds this |
| Per-request timeout | 30 seconds | Prevent runaway analysis on pathological crates |
| Session idle TTL | 5 minutes | Amortize startup across related queries |
| RA initialization timeout | 120 seconds | Large crates (tokio, bevy) take time to analyze |
| Analysis cache TTL | 24 hours | Re-analyze if crate source changes (hash check) |

### Graceful degradation

Every tool that uses RA must work without it:

1. RA disabled at container build time (`ENABLE_RA=false`) — `ra.*` tools return a clear "not available" error; enriched tools use syn/rustdoc only.
2. RA enabled but session fails to start — fall back to syn/rustdoc, set `confidence` to medium/low, add `"ra_fallback": true` to response.
3. RA enabled, session healthy, but specific query times out — return partial results from syn/rustdoc with timeout note.

### Provenance contract

RA-backed responses include:

```json
{
  "provenance": "rust_analyzer",
  "ra_session_info": {
    "crate_analyzed": "tokio",
    "version_analyzed": "1.48.0",
    "analysis_cached": true,
    "analysis_age_seconds": 3421
  }
}
```

Merged results (syn + RA) use `"provenance": "syn+rust_analyzer"`.

## Phased rollout

### Phase 0: Infrastructure (P0)

**Scope**: Container, session management, no user-facing tools.

**Work**:
- Add optional RA + toolchain to Dockerfile (build arg gated).
- Implement `RaSessionManager`: spawn, pool, LRU evict, timeout, health check.
- Implement `rust-project.json` generation from indexed `dependency_edges` for a target crate version.
- Implement sysroot detection and validation at startup.
- Add `ra_enabled` flag to `index.status` response.
- Add config: `RA_ENABLED`, `RA_MAX_SESSIONS`, `RA_SESSION_TTL_SECS`, `RA_REQUEST_TIMEOUT_SECS`, `RA_MEMORY_LIMIT_MB`.

**Exit criteria**: RA subprocess can be spawned, sent an `initialize` LSP request, and return capabilities for a cached crate version. Session pool manages lifecycle correctly.

### Phase 1: Core query tools (P1)

**Scope**: `ra.type_at`, `ra.definition`, `ra.completions` + enrichments for `source.context` and `crate.type_info`.

**Rationale**: These are the highest-value tools targeting the most common agent failures (wrong types, wrong signatures, wrong methods). They also exercise the most common LSP request paths (hover, definition, completion), validating the infrastructure.

**Exit criteria**:
- `ra.type_at` returns correct inferred types for variables in indexed crate source.
- `ra.definition` resolves cross-module definitions within a crate.
- `ra.completions` returns method lists matching `cargo doc` output.
- `source.context` and `crate.type_info` use RA when available and fall back cleanly.

### Phase 2: References, diagnostics, import paths (P2)

**Scope**: `ra.references`, `ra.diagnostics`, `ra.import_path` + enrichment for `crate.re_exports` and `crate.trait_impls`.

**Exit criteria**:
- `ra.references` finds intra-crate usages accurately.
- `ra.import_path` returns canonical paths matching rustdoc output.
- `crate.trait_impls` includes blanket and auto trait impls when RA is available.

### Phase 3: Advanced analysis (P3)

**Scope**: `ra.expand_macro` + enrichments for `crate.error_types` and `crate.migration_path`.

**Exit criteria**:
- `ra.expand_macro` returns expanded derive macro code.
- `crate.migration_path` can identify concrete breakage sites beyond API diff.

## Alternatives considered

### Deepen rustdoc JSON instead of RA

Rustdoc JSON (`cargo doc --output-format json`) provides authoritative public API data including:
- Resolved types with full generic parameters
- All trait implementations (including auto traits, blanket impls)
- Canonical import paths
- Associated types and constants

This covers ~60-70% of the value RA would provide for `crate.type_info`, `crate.trait_impls`, and `crate.re_exports`, at much lower operational cost (one-time generation, no persistent process).

**Why not sufficient alone**: rustdoc JSON only covers public items. It doesn't help with `ra.type_at` (needs type inference in function bodies), `ra.completions` (needs context-aware method resolution), `ra.expand_macro` (needs the compiler's macro expander), or intra-crate references. It also requires `cargo doc` which needs network access for dependency resolution on first run.

**Recommendation**: continue deepening rustdoc JSON integration as the primary enrichment path. Use RA as a complementary layer for the capabilities rustdoc can't provide (type inference, completions, macro expansion, intra-crate navigation).

### Host-side RA bridge (MCP-to-LSP proxy)

Leverage the user's existing rust-analyzer installation by proxying LSP requests from the MCP server to the host's RA.

**Rejected because**:
- Couples the MCP server to the user's editor state and RA version.
- Can only analyze the exact crate versions in the user's current lock file, not arbitrary indexed versions.
- Requires the user's IDE to be running and RA to be initialized.
- Introduces cross-container networking complexity.

### On-demand RA per request (no session pool)

Spawn RA fresh for each query, analyze, return, kill.

**Rejected because**: RA initialization takes 10–120 seconds depending on crate complexity. This makes every query unacceptably slow. The session pool with TTL-based reuse is mandatory for usable latency.

## Open questions

1. **Session isolation mechanism**: cgroups v2 memory limits (requires container privileges) vs simple RSS monitoring + SIGKILL? The former is more reliable; the latter is simpler.
2. **Dependency source availability**: what percentage of the typical user's cargo cache has complete transitive dependency source? If low, RA analysis will frequently fail on unresolved imports. Need to measure this empirically.
3. **`rust-project.json` fidelity**: can we generate accurate enough project descriptors from `dependency_edges` alone, or do we need to parse the cached `Cargo.toml` + `Cargo.lock` per crate version?
4. **RA version pinning**: should the container pin a specific RA release, or track stable? Pinning avoids surprises but requires manual updates.
5. **Feature flag handling**: how do we tell RA which features to enable when analyzing a crate? Default features only? All features? User-configurable?
6. **Warm cache strategy**: should RA analysis be triggered proactively during `index.refresh scope=local_cache`, or only on first query? Proactive is better UX but consumes resources even for crates the user may never query.
