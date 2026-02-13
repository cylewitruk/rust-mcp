# M7 Implementation Checklist (Ordered)

Status: Completed (2026-02-13)  
Owner: rust-mcp maintainers  
Date: 2026-02-13

## Completion summary

- [x] PR-1: confidence model hardening + `crate.api_diff`
- [x] PR-2: license schema + ingest + `crate.license_check`
- [x] PR-3: `crate.alternatives`
- [x] PR-4: `crate.hotspots`
- [x] PR-5: roadmap/checklist updates + final response-contract consistency pass

M7 shipped tools:

- `crate.api_diff`
- `crate.license_check`
- `crate.alternatives`
- `crate.hotspots`

Response-contract note:

- M7 tools include provenance/freshness/confidence metadata. Newer tools include `confidence_assessment` (`level` + `reason`).

## Scope

This checklist covers roadmap M7 (P2):

1. API diff between crate versions
2. License/policy checks
3. Alternatives suggestions
4. Unsafe/concurrency hotspots index

It is ordered by dependency, implementation risk, and reuse of shared primitives in the existing codebase.

## Why this order is correct

- `API diff` and `unsafe/concurrency hotspots` both depend on reliable symbol/source extraction quality and shared comparison/indexing logic.
- `License/policy checks` needs a small schema extension and ingest path; this should land before tools that want to include license as a ranking signal.
- `Alternatives suggestions` benefits from already-available data (`categories`, `keywords`, downloads, dependents) and should incorporate license-policy constraints once policy checks exist.
- Delivering `API diff` first gives immediate high-value agent signal while requiring minimal new remote ingestion.

## Phase 0 — Foundation hardening (short, required)

### 0.1 Add a structured confidence model (recommended pre-work)

**Reasoning**:

- Current responses use free-form `String` confidence; roadmap/spec calls for `high|medium|low` with reason.
- M7 introduces nuanced heuristics (diff confidence, hotspot confidence, ranking confidence), so this should be standardized before adding new tools.

**Touchpoints**:

- `src/mcp/models.rs` (new confidence type + envelope helper fields)
- Existing handlers where practical (`src/mcp/*.rs`) for incremental adoption

**Tests**:

- Unit tests for confidence serialization (`snake_case` enum and reason presence)
- Regression check that existing tools still serialize expected fields

**Exit criteria**:

- New tools can return a typed confidence level plus reason without ad hoc strings.

---

## Phase 1 — API diff (M7 item 1)

### 1.1 Data model + request/response contracts

**Reasoning**:

- Define contract first so query/transform logic has a stable shape.
- Keeps implementation aligned with existing tool pattern (validate → query → transform → envelope).

**Touchpoints**:

- `src/mcp/models.rs`
  - `CrateApiDiffRequest`
  - `CrateApiDiffResponse`
  - row structs for symbol snapshots and diff entries

**Response shape (minimum)**:

- inputs: `crate_name`, `from_version`, `to_version`
- outputs:
  - summary counts: added/removed/changed symbols
  - `breaking_changes_detected` boolean
  - changed symbol list with old/new signature/visibility deltas
  - freshness/provenance/confidence/next_best_calls

### 1.2 Handler implementation

**Reasoning**:

- Use existing `symbols` table and avoid new ingestion pipeline for first increment.
- Enables fast local diff over indexed versions.

**Touchpoints**:

- New file: `src/mcp/api_diff.rs`
- `src/mcp/mod.rs` (module registration)
- `src/mcp/server.rs` (tool registration: `crate.api_diff`)

**Implementation notes**:

- Validate crate/version inputs with existing `utils.rs` normalization helpers.
- Resolve both target `crate_versions` rows.
- Compare symbols by stable key `(name, kind)` with optional signature+visibility delta classification.
- Mark likely breaking changes when:
  - public symbol removed
  - public→private visibility downgrade
  - signature changed (heuristic, non-semver-proof)

### 1.3 Query performance + correctness guardrails

**Reasoning**:

- Diffs can get large on popular crates; add bounded outputs and deterministic ordering.

**Touchpoints**:

- `src/mcp/utils.rs` (limit clamp helper for diff entries)
- Migration (optional but recommended): index for compare queries
  - new migration e.g. `migrations/0005_symbol_diff_indexes.sql`

**Suggested index**:

- `symbols (crate_version_id, name, kind)`

### 1.4 Tests

**Touchpoints**:

- `src/mcp/api_diff.rs` (`#[cfg(test)]`)
- Integration-level test style should mirror `graph.rs`, `features.rs`, `security.rs`

**Cases**:

- added symbol only
- removed symbol only
- signature changed
- visibility changed
- unknown version error path
- deterministic sort and count totals

**Exit criteria**:

- `crate.api_diff` produces stable, bounded results with clear breaking-change hints.

---

## Phase 2 — License/policy checks (M7 item 2)

### 2.1 Schema support for license metadata

**Reasoning**:

- Current schema does not store license expression in `crates` or `crate_versions`.
- Policy checks require normalized, queryable license fields.

**Touchpoints**:

- New migration e.g. `migrations/0006_license_metadata.sql`
  - add `license_expression TEXT` to `crate_versions` (preferred)
  - optional normalized helper columns/tables only if needed later

**Implementation decision**:

- Store per-version license; do not assume crate-level static license.

### 2.2 Ingestion wiring

**Reasoning**:

- Ensure `index.sync_crates` and refresh paths populate license fields.

**Touchpoints**:

- `src/mcp/index.rs` (upsert queries for versions)
- `src/mcp/models.rs` (crates.io version payload field if not already present)

### 2.3 Tool surface

**Reasoning**:

- Ship as dedicated tool for clear policy contract and easier agent usage.

**Touchpoints**:

- New file: `src/mcp/license.rs`
- `src/mcp/mod.rs`
- `src/mcp/server.rs` (tool registration: `crate.license_check`)
- `src/mcp/models.rs` request/response types

**Policy model (MVP)**:

- Input:
  - `crate_name`, optional `version`
  - `allow_licenses: Vec<String>` optional
  - `deny_licenses: Vec<String>` optional
- Output:
  - detected `license_expression`
  - `policy_result`: `allowed|denied|unknown`
  - `policy_reasons`
  - suggested alternatives/next calls

### 2.4 Tests

**Cases**:

- exact allow match
- deny match
- unknown/missing license
- version-specific license differences

**Exit criteria**:

- Policy verdict reproducible and driven from indexed data, not remote calls.

---

## Phase 3 — Alternatives suggestions (M7 item 3)

### 3.1 Ranking model (deterministic, transparent)

**Reasoning**:

- Needs explainable ranking to keep agent trust high.
- Reuse already-indexed data: `categories`, `keywords`, downloads, dependents.

**Touchpoints**:

- New file: `src/mcp/alternatives.rs`
- `src/mcp/models.rs` for request/response + ranking reasons
- `src/mcp/server.rs` (tool registration: `crate.alternatives`)
- `src/mcp/mod.rs`

**Scoring inputs (MVP)**:

- lexical similarity to query or seed crate name
- category/keyword overlap
- download/dependent signal
- freshness and yanked/advisory penalties
- optional license-policy penalty (if Phase 2 shipped)

### 3.2 Query and ranking implementation

**Reasoning**:

- Keep SQL mostly for candidate retrieval; do final weighted score in Rust for readability and tests.

**Touchpoints**:

- `src/mcp/search.rs` (reuse or shared helper extraction if useful)
- `src/mcp/utils.rs` (score normalization helpers)

### 3.3 Tests

**Cases**:

- same-category better than unrelated crates
- advisory/yanked penalties lower rank
- deterministic ordering on ties
- optional policy filters remove disallowed entries

**Exit criteria**:

- `crate.alternatives` returns ranked candidates with explicit reason vectors.

---

## Phase 4 — Unsafe/concurrency hotspots (M7 item 4)

### 4.1 Detection primitives

**Reasoning**:

- Build simple lexical detectors first, then enrich with symbol context where available.
- Uses existing `source_files` and `symbols` indexes; no external dependencies needed.

**Touchpoints**:

- New file: `src/mcp/hotspots.rs`
- `src/mcp/models.rs`
- `src/mcp/server.rs` (tool registration: `source.hotspots` or `crate.hotspots`)
- `src/mcp/mod.rs`

**Initial detectors**:

- unsafe usage: `unsafe`, `extern "C"`, raw pointers (`*const`, `*mut`)
- concurrency: `std::sync`, `parking_lot`, atomics, `Mutex`, `RwLock`, channels
- async sync-primitive misuse heuristics (best-effort): blocking mutex in async contexts

### 4.2 Ranking and reporting

**Reasoning**:

- Agents need triage, not raw grep output.

**Output fields (minimum)**:

- file path + line
- hotspot kind + matched pattern
- short snippet
- severity heuristic (`low|medium|high`)
- confidence + provenance + next_best_calls

### 4.3 Tests

**Cases**:

- detect unsafe blocks and raw pointer patterns
- detect concurrency primitives in realistic snippets
- avoid duplicate hits on same line/pattern
- respects crate/version/path filters and limits

**Exit criteria**:

- Hotspot tool returns bounded, explainable findings from indexed local source.

---

## Phase 5 — Cross-cutting polish and release gate

### 5.1 Observability and metrics

**Touchpoints**:

- New tools automatically instrumented via `instrument_tool()` in `server.rs`.
- Add any tool-specific counters only if needed (avoid metric cardinality blowups).

### 5.2 Contract consistency

**Reasoning**:

- Ensure all M7 responses include confidence/freshness/provenance and recommended next calls.

**Touchpoints**:

- `src/mcp/models.rs`
- each new handler file

### 5.3 Validation gate

Run:

- `just fmt`
- `just lint`
- `just test`

Optional pre-merge:

- `just cbuild`

**Exit criteria**:

- All M7 tools compile, pass tests, and satisfy response contract fields.

---

## Recommended execution slices (small PR strategy)

1. PR-1: confidence model hardening + `crate.api_diff` (+ tests)
2. PR-2: license schema + ingest + `crate.license_check` (+ tests)
3. PR-3: `crate.alternatives` (+ tests, policy integration if available)
4. PR-4: hotspot tool (+ tests)
5. PR-5: docs/roadmap update + final contract consistency pass

This keeps each PR reviewable and minimizes risk from schema + tool changes landing together.

## M8 sequencing note (after M7)

After M7, the most efficient M8 order is:

1. `crate.api` (reuses symbol extraction and visibility/signature normalization improved by M7)
2. `crate.compare` (reuses alternatives/policy/freshness scoring)
3. `dependency.audit` (builds on advisory, yanked, version, and MSRV signals already unified)
