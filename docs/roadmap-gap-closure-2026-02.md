# Roadmap: Gap Closure (2026-02)

Status: Active  
Owner: rust-mcp maintainers  
Last updated: 2026-02-13

## Goal

Close the spec-vs-implementation gaps identified in the 2026-02 analysis, with priority on functional completeness and operational safety.

## Current baseline (honest status)

- Core infrastructure and all MCP tools through M7 are implemented and usable.
- Prometheus metrics export is operational via standalone `metrics-exporter-prometheus` HTTP listener on a dedicated port (default 9090). Tool call counters, latency histograms, and refresh job gauges are all exposed.
- SQL-backed tool invocation history is retained for `index.status` operational metrics.
- Remaining planned gaps are concentrated in agentic tools (P3).
- Confidence signaling is now structured for new M7 tools (`confidence_assessment` with level + reason), while legacy tools may still return string-only confidence.

## Prioritization

### P0 (blocking completeness)

1. Complete docs.rs ingestion pipeline
2. Complete `index.refresh` scope handlers (`all`, `security`, `docs`, `local_cache`)
3. Add outbound per-source rate limiting for refresh traffic

### P1 (spec compliance + high-signal low-effort)

1. `crate.features` tool — feature flags, defaults, transitive enables (prevents the most common agent error in Rust)
2. Surface MSRV (`rust_version`) in `crate.intel` and `crate.versions`
3. Add cycle-safe traversal notes to `crate.graph` output
4. ~~Add Prometheus metrics export~~ — **Done.** Standalone `metrics-exporter-prometheus` HTTP listener on dedicated port; hand-rolled `/metrics` handler removed; refresh job gauges emitted from worker loop.
5. Add request-level throttling/abuse guardrails — consider a simple concurrency semaphore over full rate limiting

### P2 (high-value capabilities from spec section 4)

1. ~~API diff between crate versions (public symbol changes)~~ — **Done** via `crate.api_diff`
2. ~~License/policy checks~~ — **Done** via `crate.license_check`
3. ~~Alternatives suggestions~~ — **Done** via `crate.alternatives`
4. ~~Unsafe/concurrency hotspots index~~ — **Done** via `crate.hotspots`

### P3 (new agentic development tools — beyond original spec)

1. `crate.api` — public API surface of a crate version (pub re-exports, structs, traits, functions with signatures). Builds on existing `syn` symbol extraction. Prevents agents from hallucinating signatures.
2. `crate.compare` — side-by-side comparison of two crates (downloads, freshness, maintenance, MSRV, dep count, license, features). Structured decision support for agents choosing between competing crates.
3. `dependency.audit` — given a Cargo.toml path, check for yanked versions, known advisories, outdated deps, MSRV conflicts. All underlying data already exists in the index.

## Milestones and acceptance criteria

### M5: Completeness + Safety (P0)

Status: Completed (2026-02-13)

### Work items

- docs.rs ingest discovery implementation
  - Enumerate pages for target crate/version (not only pre-known candidates).
  - Ingest canonical page metadata and normalized content into `docs_pages`.
  - Ensure repeat runs are incremental by hash/content checks.
- `index.refresh` handler completion
  - Implement execution path for `all`, `security`, `docs`, `local_cache`.
  - Return explicit scope execution summary per request.
- Outbound rate limiting
  - Add per-source token bucket/interval limiter for crates.io, docs.rs, OSV/RustSec.
  - Enforce in worker and inline refresh paths.

### Acceptance criteria

- `docs.search` recall improves for known crate docs paths that were previously undiscoverable.
- `index.refresh` executes all documented scopes and reports actual work performed.
- Refresh traffic remains bounded under concurrent requests and bulk refresh usage.

### M6: Spec compliance + agent signal (P1)

Status: Completed (2026-02-13)

#### Work items

- `crate.features` tool: fetch and return feature flags, defaults, and transitive enables per crate/version.
- MSRV surfacing in `crate.intel` and `crate.versions` responses.
- `crate.graph` cycle detection + response notes.
- Prometheus metrics via standalone `metrics-exporter-prometheus` HTTP listener on dedicated port (default 9090). Refresh job gauges emitted from worker loop; tool call counters/histograms from `instrument_tool()`.
- Concurrency semaphore or simple request throttle.

#### Acceptance criteria

- `crate.features` returns accurate feature flag data for indexed crates.
- MSRV is present in `crate.intel` and `crate.versions` payloads where available.
- `crate.graph` includes cycle-safe notes when applicable.
- Prometheus exporter on dedicated port exposes tool call counters, latency histograms, and refresh job queue depth gauges.

### M7: High-value capabilities (P2)

Status: Completed (2026-02-13)

#### Work items

- API diff engine over indexed symbols across versions (`crate.api_diff`).
- License policy checks over crate metadata (`crate.license_check`).
- Alternatives ranking using categories/keywords/dependents/downloads (`crate.alternatives`).
- Unsafe/concurrency hotspot extraction from indexed source (`crate.hotspots`).

#### Acceptance criteria

- Each capability is accessible via MCP tool output or extended existing tool payloads.
- Results include provenance, freshness, and confidence contract fields.

### M8: Agentic development tools (P3)

#### Work items

- `crate.api`: filtered view of public symbols from lib.rs re-exports with full signatures.
- `crate.compare`: structured side-by-side crate comparison (downloads, freshness, MSRV, deps, license, features).
- `dependency.audit`: Cargo.toml analysis for yanked deps, advisories, outdated versions, MSRV conflicts.

#### Acceptance criteria

- `crate.api` returns accurate pub API surface matching actual crate exports.
- `crate.compare` provides actionable signals for agents choosing between crates.
- `dependency.audit` identifies real issues in a provided Cargo.toml against indexed data.

## Execution plan (first 2 sprints)

### Sprint 1 (focus: M5)

- [x] Implement docs.rs page discovery and incremental docs ingestion.
- [x] Implement remaining `index.refresh` scope handlers.
- [x] Add outbound per-source rate limiting hooks.
- [x] Add targeted tests for docs ingest discovery + refresh scope execution.

### Sprint 2 (focus: M6)

- [x] Implement `crate.features` tool.
- [x] Add MSRV fields to `crate.intel` and `crate.versions` responses.
- [x] Add cycle detection notes in `crate.graph`.
- [x] Wire Prometheus metrics via standalone `metrics-exporter-prometheus` HTTP listener (port 9090). Removed hand-rolled `/metrics` handler and SQL-on-scrape queries.
- [x] Add concurrency limiter for request throttling.

## Tracking and governance

- Source of truth for active work: this file.
- M7 completion note (2026-02-13): delivered `crate.api_diff`, `crate.license_check`, `crate.alternatives`, and `crate.hotspots` with provenance/freshness/confidence contract fields.
- Retired docs:
  - `docs/implementation-checklist.md`
  - `docs/unspecced-ideas.md`
- Keep spec intent in `docs/agent-dependency-mcp-spec.md` and decision rationale in `docs/adr-0001-refresh-strategy.md`.
- Update this roadmap at least once per milestone with:
  - completed items,
  - newly discovered gaps,
  - scope changes and rationale.
