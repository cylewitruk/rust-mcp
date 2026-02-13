# Rust Dependency Intelligence MCP Server

Status: Draft v0.2  
Owner: You  
Target: Rust-focused coding agents needing fast, local, low-friction dependency intelligence
Scope: Personal development machines (single user, local workstation)

## 1) Problem Statement

Rust coding agents frequently waste tool calls and latency on:

- Repeated web searches for crates and docs.
- Repeated `find`/`grep` over `~/.cargo/registry`.
- Symbol lookup via naive text search instead of semantic indexes.
- Fragmented retrieval (crates.io + docs.rs + local source) across many unspecialized tools.

The goal is a single MCP server optimized for "outside current project" dependency research, backed by local indexes and caches, so agents can answer most dependency questions in one or two tool calls.

## 2) Product Goals

### P0 goals

- Provide one local MCP server instance reusable by multiple local agents/processes on the same machine.
- Prefer read-through indexing and avoid broad periodic crawling by default.
- Support fast crate discovery and crate metadata retrieval from crates.io.
- Support fast local source search over cargo cache with indexed file trees and full-text search.
- Support symbol lookup with file + line locations (semantic where possible).
- Return dense, agent-optimized responses to reduce tool thrash.

### P1 goals

- Surface security posture per version (RustSec/OSV advisory matches).
- Index docs.rs content for local docs search.
- Provide dependency and reverse-dependency graph queries with ranking.

### Non-goals (initial)

- Acting as a package manager or modifying user dependencies.
- Replacing rust-analyzer in editors.
- Building every crate docs locally by default (too expensive for v1).
- Supporting internet-facing multi-tenant deployments in v1.

## 3) Users and Jobs-To-Be-Done

- Primary user: coding agents (Codex/Claude/etc.) operating on a Rust project.
- Secondary user: human developers validating versions/APIs/adoption risk.

Jobs:

- "Find a crate for X and justify why."
- "Tell me which version of crate Y is safe/popular/current."
- "Show where symbol Z is defined in crate Y."
- "Show how crate Y is used by dependents."
- "Give me README/docs/source snippet quickly."

## 4) High-Value Capabilities (Beyond Current Braindump)

- API diff between crate versions (new/removed/changed public symbols).
- Feature flag intelligence (default features, optional features, likely heavy features).
- MSRV signals (declared `rust-version`, ecosystem reports if available).
- License and policy checks (SPDX expressions, incompatible licenses).
- "Alternatives" suggestions (similar crates by category/tags/dependents/downloads).
- Unsafe/concurrency hotspots index (`unsafe`, `std::sync`, atomics, etc.).
- Confidence and provenance in every answer (what source, timestamp, freshness).
- Query memoization and dedupe so repeated agent queries are near-zero cost.

## 5) Transport and Deployment

### Recommendation

- Default transport: MCP Streamable HTTP bound to loopback for local shared access.
- Optional transport: stdio mode for single-agent local workflows.

Rationale:

- Loopback HTTP lets multiple local agents share one index/database without remote exposure.
- Stdio is process-scoped and awkward for multiple concurrent local agents.

### Deployment model

- One Docker container exposing MCP HTTP on loopback (example host bind `127.0.0.1:43173`) and running PostgreSQL internally.
- Read-only mount: host `~/.cargo/registry` into container.
- Persistent volume (or host mount) for PostgreSQL data and cache artifacts.
- Use a process supervisor (`tini` + supervisor/s6) so Postgres and MCP lifecycle/health checks are coordinated.
- In-process background jobs for refresh/indexing (no sidecar needed for v1 local scope).
- Default policy is interaction-driven refresh (read-through + queue), not full scheduled sync of all crates.

## 6) Data Sources

- crates.io API/index for crate metadata, versions, yanked status, dependencies, downloads.
- docs.rs pages/metadata for docs URLs and docs content indexing.
- RustSec advisory database (and optionally OSV) for version vulnerability matching.
- Local cargo registry source cache (`~/.cargo/registry/src/...`) for source and symbols.

## 7) Storage and Indexing Design

Use PostgreSQL as the sole data and indexing engine for v1:

- PostgreSQL for structured metadata and query cache.
- PostgreSQL full-text search (`tsvector` + `GIN`) for README/docs/source text.
- PostgreSQL trigram search (`pg_trgm`) for fuzzy crate/symbol/path matching.
- Tree-sitter Rust (or rust-analyzer-derived output) for symbol extraction fallback.
- Recursive CTEs and materialized projections for dependency/reverse-dependency traversal.

Recommended Postgres extensions:

- `pg_trgm` for fuzzy lookup/ranking.
- `unaccent` for more robust text matching.
- `btree_gin` for composite indexing patterns.
- Optional: `pgvector` for later semantic retrieval.
- Optional: Apache AGE (or similar) only if graph workloads outgrow recursive SQL.

### Core entities

- `crate` (name, description, categories, keywords, repository/docs/homepage URLs).
- `crate_version` (semver, publish date, yanked, downloads, checksum, rust-version).
- `dependency_edge` (from crate/version -> to crate/version range, normal/build/dev, features).
- `reverse_dependency` (materialized or query-derived).
- `advisory_match` (crate/version/advisory id/severity/fixed versions/source).
- `source_file` (crate/version/path/hash/size/indexed_at).
- `symbol` (name, kind, crate/version/path/start_line/end_line/signature visibility).
- `docs_page` (crate/version/path/title/content hash/snippet index).
- `query_cache` (normalized query hash -> response payload/freshness).
- `refresh_job` (deduped queue row for crate/scope, priority, status, attempts, last_error, timestamps).
- Freshness metadata on crate records (e.g., `last_checked_at`, `next_check_at`, `ttl_hint_seconds`, `ttl_reason`).

### Indexes

- Exact indexes: crate name, version, semver sort keys.
- Full text: README/docs/source `tsvector` columns with weighted ranking.
- Symbol index: exact + fuzzy symbol search (prefix/suffix/snake/camel split).
- Path/tree index: file path and directory hierarchy for quick "where is X implemented?".

## 8) Ingestion Pipelines

### A. Remote metadata sync (crates/docs/security)

- Pull crate search metadata and crate details on demand with interaction-driven refresh.
- Fetch version history and dependency metadata per crate.
- Fetch advisories and compute version-range matches.
- Refresh policy:
  - On tool interaction (`crate.search`, `crate.intel`), if TTL expired: perform lightweight freshness probe first.
  - If unchanged: update freshness metadata only (`last_checked_at`/`next_check_at`) and serve indexed data.
  - If changed: do minimal inline refresh needed for the active request, and enqueue deep refresh.
  - Missing requested version bypasses TTL and triggers targeted inline fetch + queued deep backfill.
  - Manual `index.refresh` remains available for explicit operator/user intent.
  - Broad scheduled crawl is optional and disabled by default.

### A1. Adaptive TTL heuristic

- TTL is per crate and adaptive, with floor/cap and jitter.
- Suggested bounds:
  - min TTL: 1 hour
  - max TTL: 90 days
- Signals used:
  - recency of latest publish date
  - release frequency over trailing window (e.g., last 12 months)
  - recent probe outcomes (changed vs unchanged)
  - optional security pressure (temporary TTL reduction)
- Example banding:
  - highly active crates: 6–24h
  - moderate activity: 2–7d
  - long-stable crates: 14–90d
- Apply ±10–20% jitter to prevent synchronized refresh spikes.

### A2. Refresh queue behavior

- Queue is deduplicated by `(crate_name, scope)` for `pending|running` jobs.
- Priority tiers:
  - interactive requests / missing-version backfills: high
  - background deep refresh and maintenance: normal/low
- Worker executes deep refresh asynchronously with bounded concurrency and retries.
- Queue and worker state are surfaced by `index.status`.

### B. Local cargo cache scan

- Traverse mounted cargo registry source directories.
- Detect new/changed crate versions using path + hash metadata.
- Build/update:
  - File tree index
  - Full-text index
  - Symbol index (semantic if available, lexical fallback)

### C. Semantic symbol indexing strategy

Phased approach:

- Phase 1: fast lexical + parser-based symbol extraction (Tree-sitter/`syn`).
- Phase 2: rust-analyzer-driven semantic indexing (definitions/usages where feasible).
- Phase 3: optional rustdoc JSON integration for stable public API indexing/diff.

## 9) MCP Tool Surface (v1 Proposal)

Design principle: fewer, denser calls. Prefer one "intelligence" tool over many narrow calls.

### Tool: `ping`

Input:

- optional `message`

Output:

- connectivity/readiness echo suitable for MCP client handshake checks

### Tool: `index.sync_crates`

Input:

- optional `query`
- optional `page`
- optional `per_page`
- optional `include_dependencies`

Output:

- crates.io synchronization summary for crates, versions, and dependency edges
- selected versions and per-source freshness metadata

### Tool: `crate.search`

Input:

- `query` (string)
- optional filters: `category`, `keyword`, `sort` (`relevance|downloads|recent`), `limit`

Output:

- ranked crate hits with summary metadata and reasons for ranking
- stale-while-revalidate behavior: may trigger freshness probe and queue deep refresh for stale crates

### Tool: `crate.intel`

Input:

- `crate_name`
- optional `version` (default latest stable)

Output (single dense payload):

- latest version + publish date
- version history with publish dates and yanked flags
- advisory/CVE matches by version
- dependency list for selected version
- reverse dependency summary (top dependents + count)
- total downloads
- README text (or excerpt + full flag)
- repository/docs/homepage URLs
- freshness timestamps per source
- if TTL expired, performs lightweight freshness probe inline before serving
- if changed, performs minimum inline update for correctness and queues deep refresh

### Tool: `crate.features`

Input:

- `crate_name`
- optional `version`

Output:

- indexed feature flags
- default feature set
- transitive feature enables

### Tool: `crate.api_diff`

Input:

- `crate_name`
- `from_version`
- `to_version`
- optional `limit`

Output:

- API diff summary (`added`/`removed`/`changed`)
- per-symbol change records with breaking-change hints

### Tool: `crate.api`

Input:

- `crate_name`
- optional `version`
- optional `path_glob`
- optional `kinds`
- optional `limit`

Output:

- indexed public API symbols with signatures and source locations

### Tool: `crate.compare`

Input:

- `left_crate`
- `right_crate`
- optional `left_version`
- optional `right_version`

Output:

- side-by-side comparison of adoption/risk/maintenance signals
- recommendation with reason vector

### Tool: `crate.license_check`

Input:

- `crate_name`
- optional `version`
- optional `allow_licenses`
- optional `deny_licenses`

Output:

- selected version license expression
- matched SPDX-like identifiers
- policy decision (`allowed|denied|unknown`) and reasons

### Tool: `crate.alternatives`

Input:

- `crate_name`
- optional `version`
- optional `limit`
- optional `allow_licenses`
- optional `deny_licenses`

Output:

- ranked alternatives with scores and rationale vectors
- optional policy filtering outcomes

### Tool: `crate.versions`

Input:

- `crate_name`

Output:

- normalized version timeline with yanked/security/adoption markers

### Tool: `crate.graph`

Input:

- `crate_name`
- optional `version`
- `direction` (`dependencies|dependents|both`)
- `depth` (bounded, default 1)

Output:

- graph edges + node metadata + cycle-safe traversal notes

### Tool: `crate.hotspots`

Input:

- `crate_name`
- optional `version`
- optional `path_glob`
- optional `include_unsafe`
- optional `include_concurrency`
- optional `limit`

Output:

- unsafe/concurrency hotspot hits with path, line, severity, and snippet

### Tool: `dependency.audit`

Input:

- `cargo_toml_path`

Output:

- manifest dependency audit results
- issue list covering yanked versions, advisories, outdated selections, unresolved deps, and MSRV conflicts

### Tool: `source.search`

Input:

- `query`
- optional `crate`, `version`, `path_glob`, `limit`
- optional `mode` (`text|regex`)

Output:

- matching files/lines/snippets with crate/version context

Implementation note: use `ripgrep` in container for fallback, but serve primarily from index.

### Tool: `symbol.search`

Input:

- `symbol_query`
- optional `crate`, `version`, `kind` (`fn|struct|enum|trait|type|mod|const|macro`)

Output:

- symbols with signature, file path, start/end lines, confidence, index source (`semantic|lexical`)

### Tool: `source.read`

Input:

- `crate`, `version`, `path`, `start_line`, `end_line`

Output:

- requested snippet with stable location metadata

### Tool: `docs.search`

Input:

- `query`
- optional `crate`, `version`, `limit`

Output:

- ranked docs hits with snippets and canonical docs.rs URLs

### Tool: `index.status`

Output:

- index freshness, coverage stats, queue depth, running jobs, last errors

### Tool: `index.refresh`

Input:

- scope (`crate|all|security|docs|local_cache`)
- optional `crate_name`

Output:

- accepted job id + estimated completion + current status
- supports both immediate scoped refresh and queued deep refresh semantics

## 10) Response Quality Contract

Every tool response should include:

- `provenance`: source(s) used (crates.io/docs.rs/local cache/rustsec).
- `freshness`: timestamp(s) and staleness indicators.
- `confidence`: high/medium/low with reason.
- `next_best_calls`: suggested follow-up tools only when needed.

This lets agents avoid blind iterative probing.

For interaction-driven refresh specifically, include when relevant:

- `freshness_check_performed`: boolean
- `freshness_check_result`: `unchanged|changed|failed|skipped`
- `refresh_enqueued`: boolean
- `refresh_job_id`: optional string

## 11) Non-Functional Requirements

- Query latency targets:
  - cached metadata query: <200 ms p50
  - local source/symbol query: <500 ms p50
  - cold remote fetch allowed to exceed, but must stream progress
- Concurrent local clients: at least 3 active agent sessions on one workstation.
- Deterministic pagination and stable sorting keys.
- Bounded memory with backpressure on indexing jobs.
- Bounded outbound refresh traffic with per-source rate limits and queue backpressure.
- Observability:
  - structured logs
  - metrics (`query_count`, `latency`, `cache_hit_rate`, `index_lag`, `error_rate`)
  - health/readiness endpoints

## 12) Security and Safety

- Run container as non-root.
- Mount cargo registry read-only.
- Bind MCP HTTP to loopback by default; no external exposure.
- Do not expose PostgreSQL externally; keep DB reachable only from MCP process in-container.
- Disable arbitrary command execution from MCP inputs.
- Strict input validation for regex/glob/path parameters.
- Egress allowlist where possible (`crates.io`, `docs.rs`, advisory sources).
- Max query/result limits to prevent abuse.

## 13) Suggested Tech Stack (Rust)

- MCP server framework: `rmcp` (or equivalent Rust MCP library).
- HTTP transport: `axum`/`hyper` integration.
- DB: `sqlx` + PostgreSQL.
- Search/indexing: PostgreSQL `tsvector`/`GIN` + `pg_trgm`.
- Local scan/search fallback: `ripgrep` binary in container.
- Parser/symbol fallback: `tree-sitter-rust` and/or `syn`.
- Optional semantic layer: rust-analyzer protocol integration.
- Background jobs: `tokio` tasks + durable job table.

## 14) Phased Delivery Plan

### Milestone M0: Skeleton and local shared daemon

- MCP HTTP server (loopback default), health endpoints, Docker image, persistent volume.
- `index.status` and basic job plumbing.

### Milestone M1: Crate intelligence core

- `crate.search`, `crate.intel`, `crate.versions`, `crate.graph` (depth 1).
- crates.io + RustSec integration.

Exit criteria:

- One call returns all core metadata requested in your braindump.

### Milestone M2: Local cache indexing/search

- cargo cache scanner, file tree index, full-text search.
- `source.search`, `source.read`.

Exit criteria:

- Agent can find and read dependency source without repeated `find`/`grep` tool loops.

### Milestone M3: Symbol intelligence

- `symbol.search` with file+line start/end.
- confidence flag indicating semantic vs lexical index source.

Exit criteria:

- Agent can jump directly to symbol definitions in dependencies.

### Milestone M4: Docs intelligence and advanced signals

- `docs.search` and docs content indexing.
- adoption ranking improvements, alternatives, API diff (optional stretch).

## 15) Open Design Decisions

- Whether to require specific Postgres extensions at startup or degrade gracefully when missing.
- How deep rust-analyzer integration should be in v1 (cost/complexity tradeoff).
- Whether docs indexing is pull-based (on demand) or prebuilt for hot crates.
- Whether reverse dependencies come entirely from crates.io API or local materialization.
- Whether graph queries remain recursive SQL only or adopt a graph extension later.
- Exact adaptive-TTL function, weights, and default caps/floors.
- Whether to persist per-source ETag/Last-Modified hints where available.

## 16) Immediate Next Implementation Tasks

1. Add durable `refresh_job` schema + worker with dedupe/priority/retry.
2. Add freshness metadata fields and adaptive TTL calculation per crate.
3. Update `crate.search`/`crate.intel` to run lightweight freshness probes inline.
4. Implement minimal-inline-refresh + queued deep refresh flow for changed or missing-version crates.
5. Add benchmarks/telemetry for freshness checks, queue latency, and remote call volume.
