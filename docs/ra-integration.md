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
| Sysroot provisioning | Infrastructure | Host-mounted `rust-src` via MCP client-provided sysroot path | P0 | Planned |

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

**Correction**: Mount the host's `rust-src` into the container (see "Container image changes — Sysroot provisioning"). The MCP client provides the sysroot path for the active workspace toolchain. This is a hard prerequisite for RA analysis.

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

```text
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

**No `target/` directory explosion (in safe mode)**: When proc-macro expansion and build-script execution are both disabled (the safe default — see Threat Model below), RA performs purely in-memory analysis via the Salsa incremental computation database and a virtual filesystem (VFS). No `target/` directory is created. However, if macro mode is enabled (`procMacro.enable=true`, which implies `cargo.buildScripts.enable=true`), RA will invoke the proc-macro server and may create build artifacts. The configuration matrix:

| Mode | `procMacro.enable` | `buildScripts.enable` | `target/` created | Macro expansion |
| --- | --- | --- | --- | --- |
| Safe (default) | `false` | `false` | No | None |
| Macro mode (opt-in) | `true` | `true` | Yes (constrained) | Full |

**Why subprocess, not sidecar, host bridge, or library:**

- Subprocess keeps the single-container deployment model that defines this project.
- Host bridge (querying the user's RA instance) would create coupling to the user's IDE state, editor-specific LSP client behavior, and version skew. It also can't analyze crate versions that differ from what the user has in their lock file.
- Sidecar adds orchestration complexity for no benefit over a managed subprocess.
- In-process library integration (compiling RA crates directly into the rust-mcp binary) was rejected due to loss of process isolation, rayon/tokio thread pool conflicts, and catastrophic failure blast radius. A separate semantic worker sub-binary using RA crates is under evaluation as a potential alternative to LSP — see "Alternatives considered" for the full two-variant analysis and spike criteria.

### Container image changes

Modern rust-analyzer is a self-contained binary decoupled from the compiler frontend. It does **not** need `cargo`, `clippy`, or `librustc_driver` to run. It does need a sysroot containing standard library source (`rust-src`) and ideally compiled std artifacts (`.rlib` files) for trait resolution.

#### Sysroot provisioning: host-mounted (recommended)

The host machine already has `rust-src` installed as a rustup component. The source files (~74 MB) are platform-independent `.rs` files — the same on macOS, Linux, or Windows. They can be bind-mounted into the container read-only, just like the cargo registry already is.

Different projects may use different toolchains (via `rust-toolchain.toml` or `rustup override`), so the sysroot path varies per workspace:

```sh
# In project A (pinned to 1.93.0):
$ rustc --print sysroot
/Users/dev/.rustup/toolchains/1.93.0-aarch64-apple-darwin

# In project B (using stable):
$ rustc --print sysroot
/Users/dev/.rustup/toolchains/stable-aarch64-apple-darwin
```

Rather than hardcoding a single sysroot in `.env`, the MCP client should provide the sysroot path dynamically. The client already knows the workspace context and can run `rustc --print sysroot` for the active project. The server accepts this at initialization or per-request.

**Docker setup** (planned addition to `docker-compose.yml`): mount the entire toolchains directory so all sysroots are accessible:

```yaml
# docker-compose.yml (planned)
volumes:
  - ${HOME}/.rustup/toolchains:/rustup-toolchains:ro
```

The MCP client sends the sysroot path (e.g., `/rustup-toolchains/1.93.0-aarch64-apple-darwin/lib/rustlib/src/rust/library`), remapped to the container mount point. The server validates the path exists before passing it to RA.

**Compiled `.rlib` artifacts**: the host's compiled std artifacts are target-specific (e.g., `aarch64-apple-darwin`) and won't work in the Linux container. RA can analyze std from source instead — slightly slower on initial analysis but functionally equivalent. This is a non-issue in safe mode where RA performs purely static analysis.

**Advantages**: no additional OS packages or toolchain dependencies in the runtime image. No `apk add rust-src`, no `rust` compiler package. Works with any toolchain version the user has installed. The MCP client handles sysroot discovery transparently — no user configuration needed.

**Prerequisite**: the user must have `rust-src` installed for at least one toolchain (`rustup component add rust-src`). Most Rust developers already do. The server should validate on startup and return a clear error if the mounted sysroot lacks `rust-src`.

The following two options are alternatives for environments where a host mount is not available or practical (e.g., remote/CI deployments without a host rustup installation):

#### Option A: Alpine `apk` package (self-contained fallback)

Alpine ships `rust-analyzer` as a [community package](https://pkgs.alpinelinux.org/package/edge/community/x86_64/rust-analyzer). Installing it pulls in `rust-src` and `rust` (the compiler) as transitive dependencies:

| Package | Installed size |
| ------- | -------------- |
| `rust-analyzer` | ~21 MB (dynamically linked against musl + mimalloc) |
| `rust-src` | ~74 MB (std library source for sysroot) |
| `rust` | ~211 MB (rustc + compiled std `.rlib` artifacts — dep of `rust-src`) |
| **Total** | **~305 MB** (plus transitive shared libs: LLVM, gcc, musl-dev) |

```dockerfile
# RA integration layer (optional, behind build arg)
ARG ENABLE_RA=false
RUN if [ "$ENABLE_RA" = "true" ]; then \
      apk add --no-cache rust-analyzer; \
    fi
```

**Advantages**: zero manual wiring. Alpine places everything in standard paths — sysroot discovery, proc-macro server, library paths all work automatically. Version is managed by the Alpine package maintainers and tracks stable releases. The `rust` package also provides `rustc`, which RA can use for `proc-macro-srv` and sysroot detection via `rustc --print sysroot`.

**Disadvantage**: pulls in the full Rust compiler (~211 MB) which we don't need for anything else. The container image already uses a multi-stage build that discards the build toolchain, so adding it back in the runtime stage is conceptually untidy. However, since RA integration is opt-in behind a build arg, this only affects users who explicitly enable it.

#### Option B: Standalone binary from GitHub releases (leaner image)

Download the statically-linked musl binary directly and provision a minimal sysroot separately:

| Component | Size (approx) |
| --------- | -------------- |
| `rust-analyzer` binary (musl static, from [GitHub releases](https://github.com/rust-lang/rust-analyzer/releases)) | ~50-60 MB |
| `rust-src` (extracted from `apk` or rustup component) | ~74 MB |
| `rust-std` compiled `.rlib` artifacts | ~130 MB (needed for trait solving) |
| **Total** | **~255-265 MB** |

```dockerfile
ARG ENABLE_RA=false
ARG RA_VERSION=2026-02-09
RUN if [ "$ENABLE_RA" = "true" ]; then \
      wget -qO- "https://github.com/rust-lang/rust-analyzer/releases/download/${RA_VERSION}/rust-analyzer-x86_64-unknown-linux-musl.gz" \
        | gunzip > /usr/local/bin/rust-analyzer && \
      chmod +x /usr/local/bin/rust-analyzer && \
      apk add --no-cache rust-src; \
    fi
```

**Advantages**: avoids installing `rustc` and LLVM if we can extract `rust-src` independently. Pinned RA version decoupled from Alpine's release cycle.

**Disadvantages**: `rust-src` on Alpine depends on `rust`, so `apk add rust-src` pulls in the compiler anyway — negating the size savings. To truly avoid the compiler, we'd need to extract `rust-src` from a builder stage or rustup component, which adds build complexity. Additionally, without `rustc` in the image, RA cannot auto-detect the sysroot and we must wire `RUST_SYSROOT` or configure it in every `rust-project.json` we generate. The proc-macro server also won't be available without extra work.

#### Recommendation

**Use host-mounted sysroot** for local development (the primary use case). This requires no additional OS packages for sysroot, no user configuration beyond what the MCP client provides automatically, and works with any toolchain the user has installed.

For the **LSP subprocess approach** (Phase 0-1 default): use Option A (`apk add rust-analyzer`) gated behind `ENABLE_RA=false`. The ~305 MB cost is modest for an opt-in feature, and the operational simplicity is significant. Even with a host-mounted sysroot, the stock `rust-analyzer` binary must come from somewhere — Alpine's package is the simplest source. Note: while having `rustc` in the container opens the door to running `cargo doc --output-format json` for container-side rustdoc JSON generation, this is an [unstable feature](https://doc.rust-lang.org/rustdoc/unstable-features.html#json) requiring nightly Rust and `-Z unstable-options`. Host-provided rustdoc JSON remains the stable baseline.

For the **semantic worker approach** (Variant B, if spike succeeds): no additional OS packages or toolchain dependencies are needed. The worker binary is built from source in the Docker build stage and copied into the runtime image alongside rust-mcp. The only image size increase is the worker binary itself (estimated ~50-60 MB based on rust-analyzer release binary size). Combined with the host-mounted sysroot, there are no additional runtime package dependencies.

All approaches should be gated behind `RA_ENABLED=false` by default.

### RA session management

**Why multiple processes**: RA is an LSP server initialized for *one project*. A single RA process cannot analyze both `axum@0.7.5` and `serde@1.0.228` — they have different `rust-project.json` descriptors, different dependency graphs, and different analysis contexts. Switching the project model within one process means re-initialization, which is the expensive part (10-120 seconds depending on crate complexity). So the architecture uses separate RA processes, one per active crate version.

**What "cached session" means**: There are no disk artifacts beyond the generated `rust-project.json` (which is cheap). The "cache" is the RA process *itself* staying alive in memory with its Salsa-based incremental analysis database. After RA initializes for `axum@0.7.5` (expensive), subsequent requests against that same crate — hover, completions, goto-def — are fast (<100ms) because the full type graph is already computed in memory. Killing the process discards all of that; the next query pays full startup cost again.

**Session lifecycle**:

```text
1. First ra.* query for axum@0.7.5
   ├─ Generate rust-project.json → /var/lib/rust-mcp/ra-sessions/axum-0.7.5/
   ├─ Spawn RA subprocess (LSP over stdio)
   ├─ Send LSP initialize, wait for ready (10-120s)
   ├─ Handle request (<100ms)
   └─ Session enters idle state, TTL timer starts

2. Subsequent query for axum@0.7.5 (within TTL)
   ├─ Reuse living process
   ├─ Handle request (<100ms)
   └─ Reset idle TTL timer

3. Idle TTL expires (no queries for RA_SESSION_IDLE_SECS)
   └─ Kill RA process, reclaim memory

4. Query for different crate (e.g., tower@0.5.2) while axum session alive
   ├─ If pool has capacity: spawn new RA process for tower
   └─ If pool full: LRU-evict oldest idle session, then spawn
```

**Concurrency model**: Multiple concurrent sessions matter because agents typically work with a cluster of related crates. If you're building an axum handler, you'll query `axum`, `tower`, `http`, and `serde` within minutes. With a single slot, every crate switch kills the previous session and re-initializes (~30-120s penalty). With 2-3 slots, the hot working set stays alive.

However, each RA process can use 1-4 GB of RAM for a moderately complex crate. The host machine is also likely running the user's IDE with its own RA instance. So the default should be conservative.

**Configuration**:

| Variable | Default | Description |
| -------- | ------- | ----------- |
| `RA_ENABLED` | `false` | Master switch; `ra.*` tools return "not available" when false |
| `RA_MAX_SESSIONS` | `1` | Maximum concurrent RA processes. Each uses 1-4 GB RAM. Increase to 2-3 only after benchmarking peak RSS on target hardware |
| `RA_SESSION_IDLE_SECS` | `300` | Kill idle RA process after this many seconds of no queries |
| `RA_INIT_TIMEOUT_SECS` | `120` | Maximum time to wait for RA initialization before falling back |
| `RA_REQUEST_TIMEOUT_SECS` | `30` | Per-request timeout for individual LSP operations |
| `RA_MEMORY_LIMIT_MB` | `4096` | Per-process RSS limit; kill RA if exceeded |

With the default `RA_MAX_SESSIONS=1`, all queries are serialized through a single RA process. If the agent switches crates, the current session is killed and a new one spawns. This is the conservative default until benchmarks establish actual memory/latency profiles on representative hardware.

With `RA_MAX_SESSIONS=2-3`, the pool keeps multiple crate sessions alive simultaneously. Agent workflows that touch related crates in quick succession benefit from this, at the cost of higher peak memory. Only increase after measuring cold/warm p95 latency and peak RSS on target hardware (see Phase 0 exit criteria).

Requests for a crate whose session is still initializing will queue behind the initialization. Requests for a crate that requires evicting another session will block on the eviction + new initialization. The MCP handler should apply `RA_REQUEST_TIMEOUT_SECS` as an end-to-end ceiling including any initialization wait.

**Synthetic workspace construction**:

RA normally discovers project structure via `cargo metadata`, which requires `cargo` + network access. The cargo cache has individual `Cargo.toml` files per crate but no lock files or resolved dependency graph.

Instead, we use `rust-project.json` — RA's non-Cargo project format — populated from our indexed `dependency_edges` table:

1. Query `dependency_edges` for the target crate version's full transitive dependency tree.
2. For each crate version in the tree, resolve its source path in `/cargo/registry/src/index.crates.io-*/`.
3. Generate a `rust-project.json` mapping each crate to its `src/lib.rs` root, edition, features, and inter-crate dependency references.
4. Write to `/var/lib/rust-mcp/ra-sessions/<crate>-<version>/rust-project.json`.
5. Spawn RA with `--project-root` pointing at that directory.

This avoids the `cargo` dependency entirely and leverages data we already have. The `rust-project.json` files are small (a few KB even for large dependency trees) and can be cached alongside the session.

### Query flow

```text
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

**LSP mapping**: composed operation — no single RA extension provides this directly. The implementation:

1. Use `workspace/symbol` to find the symbol by name within the crate, returning all definition sites.
2. For each definition site, use `textDocument/definition` to resolve through re-exports to the canonical definition.
3. Reconstruct the shortest public module path from crate root using RA's document symbol hierarchy (`textDocument/documentSymbol`) and the definition location.
4. Cross-reference with indexed `crate.re_exports` data and rustdoc JSON (when available) to validate and rank paths.

This is more complex than a single RPC but produces higher-fidelity results by combining RA's semantic resolution with existing indexed data.

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

**With RA**: use `textDocument/hover` on type positions to extract the trait impl list from RA's hover output (which includes "Implementations" sections). For specific type-trait pairs, construct a synthetic expression and use `textDocument/completion` to verify trait method availability. RA's trait solver knows all impls including blanket, auto, and conditionally-bounded ones, but this information must be extracted via hover/completion — not via `rust-analyzer/viewItemTree`, which is a debugging endpoint with no stability guarantees.

**Limitation**: RA still can't enumerate "all types that implement trait X" across a whole crate efficiently via standard LSP operations. The bidirectional "trait → types" direction remains best-effort, using indexed `impl` block data as primary source and RA only for contextual validation.

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

All limits are configurable via the environment variables documented in the session management section above (`RA_MAX_SESSIONS`, `RA_SESSION_IDLE_SECS`, `RA_INIT_TIMEOUT_SECS`, `RA_REQUEST_TIMEOUT_SECS`, `RA_MEMORY_LIMIT_MB`). Defaults are chosen for a developer laptop running an IDE alongside the MCP server.

### Threat model and hardening

#### Container-wide network firewall

The container should enforce an outbound network allowlist regardless of whether RA is enabled. The server has a small, well-defined set of external dependencies:

| Destination | Purpose |
| --- | --- |
| `crates.io` | Crate metadata API |
| `static.crates.io` | Crate download CDN (redirected from crates.io) |
| `docs.rs` | Documentation page crawling |
| `api.osv.dev` | OSV vulnerability database queries |

All other outbound traffic (including from RA subprocesses and any proc-macro/build-script code) should be blocked by default.

**Implementation**: `iptables` rules applied in `docker-entrypoint.sh` before dropping privileges. The entrypoint resolves allowed hostnames to IPs at startup and creates OUTPUT chain rules:

```sh
# Configurable allowlist with sensible defaults
OUTBOUND_ALLOWLIST="${OUTBOUND_ALLOWLIST:-crates.io,static.crates.io,docs.rs,api.osv.dev}"

# Always allow: loopback, established connections, DNS
iptables -A OUTPUT -o lo -j ACCEPT
iptables -A OUTPUT -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT
iptables -A OUTPUT -p udp --dport 53 -j ACCEPT
iptables -A OUTPUT -p tcp --dport 53 -j ACCEPT

# Allow HTTPS to each allowed host
IFS=',' ; for host in $OUTBOUND_ALLOWLIST; do
  for ip in $(getent ahosts "$host" 2>/dev/null | awk '{print $1}' | sort -u); do
    iptables -A OUTPUT -p tcp --dport 443 -d "$ip" -j ACCEPT
  done
done

# Drop everything else outbound
iptables -A OUTPUT -p tcp --dport 1:65535 -j DROP
iptables -A OUTPUT -p udp --dport 1:65535 -j DROP
```

**Configuration**:

| Variable | Default | Description |
| --- | --- | --- |
| `OUTBOUND_ALLOWLIST` | `crates.io,static.crates.io,docs.rs,api.osv.dev` | Comma-separated hostnames permitted for outbound HTTPS |
| `OUTBOUND_FIREWALL` | `true` | Set to `false` to disable the firewall entirely (development/debugging) |

**IPv6 policy**: The `iptables` rules above only cover IPv4. To prevent IPv6 bypass, the entrypoint should also apply matching `ip6tables` rules, or disable IPv6 in the container entirely (`sysctl net.ipv6.conf.all.disable_ipv6=1`). Since the allowlist destinations are resolved via `getent ahosts` (which returns both A and AAAA records), the firewall setup should iterate over both address families. The simpler approach is to disable IPv6 at the container level (planned addition to `docker-compose.yml`):

```yaml
# docker-compose.yml (planned)
sysctls:
  net.ipv6.conf.all.disable_ipv6: 1
```

**Caveats**:

- Requires `iptables` package in the runtime image and `NET_ADMIN` capability (or `--cap-add=NET_ADMIN` in Docker). Alternatively, if the container runs as `--privileged` or with `--cap-add=NET_RAW,NET_ADMIN`, this works out of the box.
- DNS resolution at startup means CDN IP changes during long container lifetimes won't be tracked. A periodic re-resolution cronjob or TTL-aware refresh could address this, but adds complexity. For typical container lifetimes (hours to days), startup resolution is sufficient.
- Docker's own network policies (`--network=none`, custom bridge rules) are complementary and can provide an additional layer, but the in-container firewall is self-documenting and portable.

#### RA-specific hardening

RA upstream explicitly states: ["rust-analyzer assumes that all code is trusted."](https://rust-analyzer.github.io/book/security.html) By default, RA executes proc macros and build scripts, both of which run arbitrary code. Since this server analyzes third-party crate source from the cargo registry, this is a real attack surface. The container-wide firewall above provides the network layer; the controls below address RA-specific risks.

**Safe mode (default)**: All RA sessions launch with proc-macro expansion and build-script execution disabled:

```json
{
  "rust-analyzer.procMacro.enable": false,
  "rust-analyzer.cargo.buildScripts.enable": false
}
```

These settings are passed via LSP `initialize` params. In safe mode, RA performs purely static analysis — name resolution, type inference, trait solving — without executing any crate code. The tradeoff is that derive macro expansions and build-script-generated code are invisible to analysis.

**Macro mode (opt-in)**: When `RA_MACRO_EXPANSION=true` is set, RA sessions launch with `procMacro.enable=true` (which implies `buildScripts.enable=true`). This enables full macro expansion but requires additional hardening beyond the network firewall:

- **Filesystem**: RA process restricted to read-only access on `/cargo/registry` and its session directory. No write access to other paths. Enforced via container user permissions (the `rust-mcp` user has no write access outside `/var/lib/rust-mcp/`).
- **Network**: Already blocked by the container-wide firewall. Proc-macro and build-script code cannot make outbound connections to arbitrary hosts.
- **Resource limits**: `RA_MEMORY_LIMIT_MB` enforced via RSS monitoring + SIGKILL. `RA_REQUEST_TIMEOUT_SECS` as hard wall-clock ceiling.
- **`targetDir` isolation**: When macro mode creates build artifacts, constrain them to `/var/lib/rust-mcp/ra-sessions/<crate>-<version>/target/` via `rust-analyzer.cargo.targetDir` configuration. This directory is per-session and cleaned up on session eviction.
- **Kill-on-timeout**: If RA does not respond to a cancellation request within 5 seconds, SIGKILL the process.

**RA-specific configuration**:

| Variable | Default | Description |
| --- | --- | --- |
| `RA_MACRO_EXPANSION` | `false` | Enable proc-macro expansion and build scripts in RA sessions |

Macro mode is not required for Phase 1-2 tools (`ra.type_at`, `ra.definition`, `ra.completions`, `ra.references`, `ra.import_path`). It only becomes necessary for `ra.expand_macro` (Phase 3) and full-fidelity `crate.trait_impls` enrichment for derive-heavy crates.

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

- Add optional RA binary to Dockerfile (build arg gated for LSP approach; built from source for worker approach).
- Add `~/.rustup/toolchains` volume mount to `docker-compose.yml`.
- Implement MCP client sysroot handshake: accept sysroot path at initialization, validate `rust-src` presence in mounted toolchains.
- Implement `RaSessionManager`: spawn, pool, LRU evict, timeout, health check.
- Implement `rust-project.json` generation from indexed `dependency_edges` for a target crate version.
- Add `ra_enabled` flag to `index.status` response.
- Add config env vars as documented in session management section.

**Exit criteria**:

- RA subprocess can be spawned, sent an `initialize` LSP request, and return capabilities for a cached crate version. Session pool manages lifecycle correctly.
- `rust-project.json` generation from `dependency_edges` produces descriptors that RA accepts without initialization errors for a corpus of at least 10 popular crates with varying dependency complexity (e.g., `serde`, `tokio`, `axum`, `reqwest`, `sqlx`). Initialization success rate >= 80% on the test corpus, with explicit fallback taxonomy for failure modes (missing transitive deps, edition mismatches, feature resolution gaps).
- Baseline benchmarks recorded: cold initialization p95 latency, warm query p95 latency, and peak RSS per session for the test corpus. These inform `RA_MAX_SESSIONS` and timeout defaults.

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

### Library integration (using RA crates directly)

Instead of running RA as an LSP subprocess, pull its internal crates (`ide`, `hir`, `load-cargo`, `project-model`) as git dependencies and invoke analysis functions via Rust API rather than LSP JSON-RPC.

**Background**: The `ide` crate provides `AnalysisHost` / `Analysis` types with direct function calls — `analysis.hover()`, `analysis.completions()`, `analysis.goto_definition()`, etc. The `load-cargo` crate provides `load_workspace()` which returns a `RootDatabase` that can be wrapped in `AnalysisHost`. The `project-model` crate supports `ProjectJson::new()` for programmatic project descriptor construction. The `analysis-stats` CLI command in the RA repo demonstrates this pattern working outside LSP.

**Why it was considered**: RA is optimized for IDE environments and normally builds/checks all dependencies, causing `target/` directory explosion. Using RA crates directly would skip LSP transport overhead and could target only the semantic functions needed for dependency intelligence (type/impl/import/macro insights), emitting structured artifacts directly for DB indexing rather than parsing LSP-shaped responses.

**Key finding: the `target/` concern is already solved in safe mode.** When RA loads via `rust-project.json` with proc-macro and build-script execution disabled (the default), it does **not** invoke `cargo check` or `cargo build`. Analysis is purely in-memory via the Salsa incremental computation database and a virtual filesystem (VFS). No `target/` directory is created. This applies to all integration approaches — `rust-project.json` + safe mode is the mechanism that avoids build artifact explosion, not the integration model. (When macro mode is enabled, build artifacts are created regardless of approach — see Threat Model section.)

There are two distinct variants of library integration with different risk profiles.

#### Variant A: In-process RA crates inside main rust-mcp binary (rejected)

Compile RA crates directly into the rust-mcp binary and call `Analysis` methods from MCP tool handlers on the tokio runtime.

**Rejected because**:

1. **No process isolation.** RA's Salsa database for a moderately complex crate uses 1-4 GB of RAM. In-process, a panic in RA analysis code crashes the entire MCP server. Memory cannot be reclaimed without dropping the entire `AnalysisHost`. With a subprocess, `SIGKILL` provides hard guarantees on resource reclamation.

2. **Thread pool conflicts.** RA uses rayon internally for parallel type inference. rust-mcp uses tokio. Two competing runtimes in the same process creates CPU scheduling contention and complicates resource accounting.

3. **Blast radius.** A bug or OOM in RA analysis takes down the MCP server, PostgreSQL health checks, Prometheus metrics, and all in-flight tool requests. The server's reliability contract requires that RA failures degrade gracefully, not catastrophically.

These are hard blockers for in-process embedding. The remaining risks (dependency chain, sysroot, version coupling) also apply but would be survivable in isolation.

#### Variant B: RA semantic worker as a separate sub-binary (candidate for spike)

Build a dedicated `rust-mcp-ra-worker` binary that links the RA crates and runs as a managed subprocess of rust-mcp. It communicates with the main process over a purpose-built protocol (e.g., structured IPC, not LSP), and emits semantic artifacts directly for DB indexing.

**How it differs from the LSP subprocess approach**:

| Aspect | LSP subprocess (current default) | RA semantic worker (Variant B) |
| --- | --- | --- |
| Binary | Stock `rust-analyzer` from Alpine apk | Custom `rust-mcp-ra-worker` built from RA crates |
| Protocol | LSP JSON-RPC over stdio | Purpose-built IPC (e.g., bincode over stdio, or shared-memory) |
| Output shape | LSP responses (hover markdown, completion items) that must be parsed and re-structured | Structured semantic data (types, impls, paths, signatures) written directly in rust-mcp's model types |
| Analysis scope | Full LSP server surface; we use a subset | Only the semantic functions needed: type resolution, trait solving, import path resolution, macro expansion |
| Container deps | `apk add rust-analyzer` (~305 MB opt-in) | Zero — binary built from source in build stage; sysroot from host mount |
| RA version coupling | Independent; upgrade via `apk upgrade` | Pinned to a git commit; upgrade requires rebuild |

**Potential advantages**:

- **Targeted extraction.** The worker can invoke exactly the `hir` and `ide` functions needed for dependency intelligence — `Type::iterate_method_candidates()`, `Module::find_use_path()`, `Module::declarations()` — and emit results as structured data directly compatible with the `symbols`, `type_members`, and `trait_impls` DB tables. No parsing hover markdown or completion item labels.
- **Batch indexing.** Instead of answering individual LSP queries, the worker can walk an entire crate's type graph in one pass, producing a complete semantic index. This amortizes RA initialization cost across all types in the crate, rather than paying per-query overhead.
- **Process isolation preserved.** As a separate binary, it still runs as a subprocess. Panics, OOM, and runaway analysis are contained. `SIGKILL` still works for resource enforcement.
- **No additional OS packages or toolchain dependencies.** The worker binary is built from source in the Docker build stage and copied alongside rust-mcp. No `apk add rust-analyzer`, no `rust` compiler package, no `rust-src` in the image. The only image size increase is the worker binary itself (estimated ~50-60 MB). Combined with the host-mounted sysroot (see Container image changes), there are no additional runtime package dependencies.

**Risks that remain**:

1. **Proc-macro expansion still needs a proc-macro server.** RA's proc-macro expansion loads dylibs via a separate server binary. The worker would either bundle `proc-macro-srv` (additional complexity) or operate in safe-mode-only for initial phases.

2. **Version coupling and upgrade cost.** RA crates are versioned `0.0.0`, unpublished, and exist only in the rust-analyzer workspace. Depending on them requires git dependencies pinned to a specific commit hash. There are no stability guarantees — function signatures, types, and module structure can change between commits. This is a manageable but real maintenance cost. Mitigation strategies:
   - Pin to tagged RA releases (weekly cadence) rather than arbitrary commits.
   - Depend on the highest-level crate APIs (`ide::Analysis`, `hir::Semantics`) which change less frequently than internal modules.
   - Budget periodic upgrade work (estimated: 2-4 hours per quarterly RA version bump for API migration, based on observed RA changelog cadence).
   - Maintain a focused integration surface — fewer call sites into RA crates means less code to update on version bumps.

**Build cost is an accepted tradeoff.** The `ide` crate transitively pulls in ~30 internal RA crates. A full `cargo build --release` of the entire rust-analyzer binary completes in ~54 seconds on a 16-core laptop (critical path: `hir-ty` 21s → `hir` → `ide-db` → `ide`). A `rust-mcp-ra-worker` with a narrower surface would be comparable or faster. This is acceptable given the release policy:

- Image rebuilds are infrequent — monthly cadence, or when Rust releases new versions / critical RA bugfixes.
- Docker layer caching amortizes RA crate compilation across builds where the RA pin is unchanged.
- Users are expected to consume prebuilt images by default; source builds are for contributors.

Build time is explicitly a non-goal for decision-making. The real decision criteria are semantic extraction quality, runtime memory/latency, RA API churn maintenance cost, and security posture.

#### Decision: default path and spike evaluation

The recommended default path remains **RA subprocess via LSP** for initial rollout (Phase 0-1). This is lower-risk, operationally simpler, and validates the core value proposition (does RA-backed semantic data measurably improve agent accuracy?) without committing to a custom binary.

A bounded spike should evaluate the semantic worker approach in parallel with or after Phase 1. The spike should:

1. Build a minimal `rust-mcp-ra-worker` binary that loads a single crate via `load-cargo` + `rust-project.json` and extracts the type/impl/trait graph via `hir` APIs.
2. Compare the extracted data against the same crate processed via LSP subprocess.
3. Measure against the acceptance criteria below.

**Concrete prototype call chain** (pinned to `vendor/rust-analyzer` @ `c75729db68`):

```rust
// 1. Construct ProjectJson from dependency_edges data
let project_json = ProjectJson::new(
    None,                          // no manifest file
    &crate_source_root,            // AbsPath to crate source
    project_json_data,             // ProjectJsonData (serde struct)
);

// 2. Load workspace into RA's database
let workspace = ProjectWorkspace::load_inline(
    project_json,
    &cargo_config,                 // CargoConfig with sysroot path
    &|msg| tracing::debug!("{}", msg),
);
let load_config = LoadCargoConfig {
    load_out_dirs_from_check: false, // safe mode: no build scripts
    with_proc_macro_server: ProcMacroServerChoice::None,
    prefill_caches: false,
    proc_macro_processes: 0,       // no proc-macro server in safe mode
};
let (db, vfs, _) = load_workspace(workspace, &extra_env, &load_config)?;

// 3. Wrap in AnalysisHost for semantic queries
let host = AnalysisHost::with_database(db);
let db = host.raw_database();

// 4. Extract semantic data via hir APIs
let all_impls = hir::Impl::all_in_crate(db, krate);
let methods = ty.iterate_method_candidates(db, &scope, None, |f| { ... });
let import_path = module.find_use_path(db, item, prefix_kind, cfg);
```

Key types: `ProjectJsonData` is the serde-deserializable struct matching `rust-project.json` format. `ProjectWorkspace::load_inline()` accepts a `ProjectJson` directly (no file I/O). `LoadCargoConfig` controls proc-macro/build-script behavior. `Module::find_use_path()` is a method on `hir::Module`, not a free function. This flow mirrors `analysis-stats` in `vendor/rust-analyzer/crates/rust-analyzer/src/cli/analysis_stats.rs`.

Adopt the worker approach only if it demonstrates material improvement over LSP mode. If the LSP approach proves sufficient for the target accuracy and latency goals, the additional build complexity of a custom binary is not justified.

**Spike acceptance criteria**:

| Metric | Threshold | How to measure |
| --- | --- | --- |
| `crate.type_info` extraction coverage | Worker captures >= 15% more methods/trait-impls per type than LSP hover parsing, measured across 10-crate corpus | Diff symbol counts between worker and LSP output for each type |
| `crate.trait_impls` completeness | Worker captures blanket/auto trait impls that LSP mode misses, for >= 50% of public types in the corpus | Count impls per type, compare against rustdoc JSON as ground truth |
| `crate.re_exports` / import path accuracy | Worker produces correct canonical paths for >= 95% of public symbols (vs rustdoc JSON ground truth) | Path comparison against `cargo doc --output-format json` |
| Cold indexing latency | Full crate semantic index in <= 2x the time of RA LSP initialization for the same crate | Wall-clock time for batch extraction vs LSP init + equivalent queries |
| Peak RSS | Worker process stays within `RA_MEMORY_LIMIT_MB` for all corpus crates | Monitor RSS via `/proc/<pid>/status` during indexing |
| Failure/timeout rate | <= 20% of corpus crates fail to produce a semantic index (matching Phase 0's `rust-project.json` fidelity bar) | Count indexing failures across the corpus |
| Follow-up tool call reduction | Agent benchmark tasks require >= 20% fewer tool calls when using worker-indexed data vs LSP-indexed data | Run standardized agent task suite, count `ra.*` and `crate.*` calls per task |

The last metric — follow-up tool call reduction — is a proxy for token/tool efficiency. If the worker's batch indexing produces richer pre-computed data in the DB, agents should need fewer round-trips to `ra.type_at`, `ra.completions`, etc. because more information is already available in `crate.type_info` and `crate.trait_impls` responses.

**RA crate API surface** (pinned to `vendor/rust-analyzer` submodule, currently `c75729db68`):

| Our tool / extraction target | RA function | Crate |
| --- | --- | --- |
| `ra.type_at` / type inference | `Analysis::hover()` | `ide` |
| `ra.definition` / goto-def | `Analysis::goto_definition()` | `ide` |
| `ra.completions` / method enumeration | `Analysis::completions()` | `ide` |
| `ra.references` / intra-crate usage | `Analysis::find_all_refs()` | `ide` |
| `ra.diagnostics` / semantic errors | `Analysis::full_diagnostics()` | `ide` |
| `ra.expand_macro` / macro expansion | `Analysis::expand_macro()` | `ide` |
| `ra.import_path` / canonical paths | `hir::Module::find_use_path()` | `hir` |
| Batch type graph extraction | `hir::Type::iterate_method_candidates()`, `hir::Impl::all_in_crate()` | `hir` |
| Batch trait impl enumeration | `hir::Impl::all_for_trait()`, `hir::Impl::all_in_crate()` + trait filtering, `hir::Type::impls_trait()` | `hir` |
| Module/re-export tree | `hir::Module::declarations()`, `hir::Module::children()` | `hir` |

Note: `hir::Trait` has no `all_in_crate()` method. Trait discovery uses `Impl::all_in_crate()` (returns all impls in a crate) with filtering, or `Impl::all_for_trait()` (returns all impls of a specific trait across crates). This table should be re-verified when the submodule pin is updated.

This table applies to the LSP subprocess approach as well — the LSP request (`textDocument/hover`, etc.) is the transport for the same underlying `ide` / `hir` function call.

### Deepen rustdoc JSON instead of RA

Rustdoc JSON (`cargo doc --output-format json`) provides authoritative public API data including:

- Resolved types with full generic parameters
- All trait implementations (including auto traits, blanket impls)
- Canonical import paths
- Associated types and constants

This covers ~60-70% of the value RA would provide for `crate.type_info`, `crate.trait_impls`, and `crate.re_exports`, at much lower operational cost (one-time generation, no persistent process).

**Why not sufficient alone**: rustdoc JSON only covers public items. It doesn't help with `ra.type_at` (needs type inference in function bodies), `ra.completions` (needs context-aware method resolution), `ra.expand_macro` (needs the compiler's macro expander), or intra-crate references.

**Important prerequisite**: `cargo doc --output-format json` is an [unstable feature](https://doc.rust-lang.org/rustdoc/unstable-features.html#json) requiring nightly Rust and `-Z unstable-options`. Container-side rustdoc JSON generation (enabled by the `rust` apk package in RA-enabled builds) therefore requires nightly toolchain installation. Host-provided rustdoc JSON via `RUSTDOC_JSON_DIR` / `index.refresh scope=rustdoc_json` remains the stable baseline and does not require nightly in the container.

**Recommendation**: continue deepening rustdoc JSON integration as the primary enrichment path, with host-generated JSON as the stable default and container-side generation as an optional nightly-only capability. Use RA as a complementary layer for the capabilities rustdoc can't provide (type inference, completions, macro expansion, intra-crate navigation).

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

## Resolved decisions

**Applies to both approaches (LSP subprocess and semantic worker)**:

1. **Default session concurrency**: `RA_MAX_SESSIONS=1` until Phase 0 benchmarks establish safe concurrency levels on representative hardware. Promote to 2 only if measured peak RSS per session stays under 2 GB for the test corpus.

2. **`rust-project.json` fidelity**: Promoted to Phase 0 hard exit criterion. Must demonstrate >= 80% initialization success rate on a 10-crate test corpus before proceeding to Phase 1.

3. **Safe mode as default**: `procMacro.enable=false`, `cargo.buildScripts.enable=false` for all RA sessions unless `RA_MACRO_EXPANSION=true` is explicitly set.

4. **Sysroot via host mount**: `~/.rustup/toolchains` mounted read-only into the container. The MCP client provides the sysroot path for the active workspace toolchain (via `rustc --print sysroot`). No `rust-src` or `rust` compiler package installed in the container image for sysroot purposes.

**LSP subprocess mode only**:

1. **RA version policy (LSP mode)**: Pin a specific Alpine `rust-analyzer` package version in the Dockerfile for reproducible builds (`apk add rust-analyzer=<version>`). Scheduled quarterly review to update the pin, aligned with Alpine stable release cadence. The Dockerfile should document the pinned version and the date of last review. This installs `rust-src` and `rust` (~305 MB) as transitive dependencies of the Alpine package.

**Semantic worker mode only**:

1. **RA version policy (worker mode)**: Pin a `rust-analyzer` git commit hash in `Cargo.toml` dependencies. The worker binary is built from source in the Docker build stage — no Alpine RA/rust/rust-src packages are installed in the runtime image. Quarterly review to update the pin, aligned with tagged RA releases.

## Open questions

1. **Session isolation mechanism**: cgroups v2 memory limits (requires container privileges) vs simple RSS monitoring + SIGKILL? The former is more reliable; the latter is simpler.
2. **Dependency source availability**: what percentage of the typical user's cargo cache has complete transitive dependency source? If low, RA analysis will frequently fail on unresolved imports. Need to measure this empirically during Phase 0.
3. **Feature flag handling**: how do we tell RA which features to enable when analyzing a crate? Default features only? All features? User-configurable?
4. **Warm cache strategy**: should RA analysis be triggered proactively during `index.refresh scope=local_cache`, or only on first query? Proactive is better UX but consumes resources even for crates the user may never query.
