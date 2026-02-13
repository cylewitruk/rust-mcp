# Implementation Prep: Opus Gap + Tooling Analysis (2026-02-14)

Status: Ready for execution  
Owner: rust-mcp maintainers  
Source inputs: `docs/roadmap-2026-02.md`, `docs/agent-dependency-mcp-spec.md`, `README.md`

## 1) What was reviewed

- Active roadmap (`docs/roadmap-2026-02.md`) and milestone targets M9–M13.
- Spec tool contract + NFR expectations (`docs/agent-dependency-mcp-spec.md`).
- Current implementation-facing docs (`README.md`, `docs/README.md`).
- Current code touchpoints for confidence contract and tool registration (`src/mcp/models.rs`, `src/mcp/server.rs`, handlers under `src/mcp/`).

## 2) Current readiness snapshot

### Confirmed

- M5–M8 functionality appears represented in roadmap and in registered tools.
- Newer tools (M7+) already emit structured `confidence_assessment`.
- Roadmap and docs index were updated to make `docs/roadmap-2026-02.md` authoritative.

### Remaining implementation gaps to close first (M9)

1. **Confidence contract unification (A1)**
   - Many response models still expose string-only `confidence` fields in `src/mcp/models.rs`.
   - Acceptance target requires every tool response to include `confidence_assessment { level, reason }`.

2. **README parity (A2)**
   - Top-level positioning still says “scaffold”.
   - Tooling/ops sections need a concise but accurate production-style framing (Prometheus, rate limits, adaptive TTL already implemented).

## 3) Execution slices (ready to implement)

## Slice M9-1: Confidence contract unification (A1)

### Scope

- Add `confidence_assessment` to all legacy response types that currently only return `confidence`.
- Keep `confidence` string for compatibility in this milestone (alias/deprecation strategy), but make structured field first-class.

### Primary touchpoints

- `src/mcp/models.rs` (response structs)
- Legacy handlers likely requiring updates:
  - `src/mcp/search.rs`
  - `src/mcp/intel.rs`
  - `src/mcp/versions.rs`
  - `src/mcp/graph.rs`
  - `src/mcp/source.rs`
  - `src/mcp/symbol.rs`
  - `src/mcp/docs_intel.rs`
  - `src/mcp/index.rs` (if responses expose confidence)

### Implementation rules

- Continue returning `confidence` as `confidence_assessment.level.as_str()` during transition.
- Use explicit reasoning strings that are deterministic and data-driven (missing index data, sparse results, stale sources, etc.).
- Keep tool pattern: validate → query → transform → envelope.

### Exit criteria

- Every tool response model includes `confidence_assessment`.
- Every handler populates both `confidence` and `confidence_assessment` consistently.
- No handler emits confidence text not representable by `ConfidenceLevel`.

## Slice M9-2: README parity update (A2)

### Scope

- Replace scaffold lede with current product positioning.
- Keep tool list grouped and scannable.
- Add short operational behavior sections for:
  - Prometheus exposure
  - per-source rate limiting
  - adaptive TTL freshness strategy

### Primary touchpoints

- `README.md`
- Optional cross-link note in `docs/README.md` if needed

### Exit criteria

- README language aligns with deployed capabilities and roadmap baseline.
- New user can infer tool surface + operational behavior without reading roadmap/spec first.

## 4) M10 pre-work (do after M9 merge)

To reduce risk before `crate.type_info` / `crate.trait_impls`:

1. **Schema design decision**
   - Decide: extend `symbols` vs add dedicated relation tables for type-members and trait impls.
   - Recommended: dedicated tables for impl relations + methods to avoid overloading existing symbol semantics.

2. **Extractor design pass (`local_cache.rs`)**
   - Add AST capture for:
     - struct fields / enum variants
     - inherent impl blocks
     - trait impl blocks (`impl Trait for Type`)
     - derive attributes mapped to trait impl hints

3. **Migration planning**
   - Prepare one focused migration for M10 with indexes for type/trait lookup paths.

4. **Contract-first models**
   - Define request/response + row structs in `models.rs` before handler logic.

## 5) Verification gate for each slice

Use project-standard checks after each implementation slice:

- `just fmt`
- `just lint`
- `just test`

Optional before merge:

- `just cbuild`

## 6) Suggested PR breakdown

1. **PR-A (M9/A1)**: confidence model unification across legacy tools + tests.
2. **PR-B (M9/A2)**: README parity refresh (docs-only).
3. **PR-C (M10 prep)**: migration + extractor skeleton + model scaffolding (no tool exposure yet) _or_ start directly with `crate.type_info` if scope is stable.

## 7) Risks and mitigations

- **Risk:** response-schema churn for MCP clients.
  - **Mitigation:** keep `confidence` during transition and document deprecation window.
- **Risk:** confidence reasons become inconsistent across handlers.
  - **Mitigation:** centralize reason patterns or helper constructors.
- **Risk:** M10 parser complexity in `syn` extraction.
  - **Mitigation:** land schema + ingestion primitives first, then tool handlers.

## 8) Definition of “prepared for implementation”

- Milestone order is explicit (M9 first, then M10).
- File-level touchpoints and validation gates are defined.
- PR slicing is small enough for low-risk review and rollback.
