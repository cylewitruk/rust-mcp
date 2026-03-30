# ROADMAP

Status: Active
Last updated: 2026-03-28

This file tracks open work, future direction, and outstanding findings from codebase review.

---

## ~~P0: Surface Doc Comments via Existing Tools~~ ✅

- [x] Add `include_docs: Option<bool>` parameter to tools that return symbols/types.
  - Doc strings from rustdoc JSON are already ingested and stored in `symbols.docs`, `crate_types.docs`, `crate_impls.docs`, and `crate_traits.docs` — but no tool currently SELECTs or returns them.
  - **Multi-item tools** (`crate_api`, `symbol_search`, `crate_trait_impls`, `crate_deprecated`): return only the **summary** (first paragraph, i.e. up to the first `\n\n`). This follows Rust doc convention where the first paragraph is the summary line.
  - **Single-item tools** (`crate_type_info`): return **full docs** when requested — naturally bounded to one item.
  - Default `false` to preserve existing response sizes and avoid token explosion (`crate_api` returns up to 1000 items).
  - Shared `doc_summary(docs: &str) -> &str` helper extracts the first paragraph.
  - Tools updated: `crate_api`, `crate_type_info`, `crate_trait_impls`, `symbol_search`, `crate_deprecated`.

## P1: Rustdoc Intelligence Quality

Canonical path resolution determines the shortest public import path for each symbol/type (e.g. `tokio::io::AsyncRead` instead of `tokio::io::async_read::AsyncRead`). This affects `crate_import_path`, `crate_api`, `crate_type_info`, and any tool that surfaces import paths.

- [ ] Hardcode well-known standard library path remappings.
  - External types from `std`/`core`/`alloc` sometimes appear with internal module paths (e.g. `std::collections::hash_map::HashMap` instead of `std::collections::HashMap`). A static lookup table for common std re-exports would fix these without loading external rustdoc JSON.
- [ ] Cross-crate canonical path resolution for non-std dependencies.
  - When a crate's API surfaces types from its dependencies (e.g. `serde::Serialize` in trait bounds), the path comes from `krate.external_crates` and may be an internal path of the external crate. Resolving this requires looking up the external crate's already-indexed canonical paths from the database. Adds complexity and a dependency-ordering constraint (external crate must be indexed first).

## P1: Tool Quality & Consistency

- [ ] Standardize freshness reporting across all tools.
  - Most crate tools include `freshness` arrays. However, `symbol_search`, `source_search`, and `docs_search` responses lack them entirely. Create a shared helper to build freshness arrays consistently.
- [ ] Expand tool descriptions with usage hints.
  - Current descriptions are concise (1-2 lines) but don't include "call this when..." guidance. Agents with 35 tools need stronger signal about when to use each.
- [ ] Fix N+1 query pattern in `crate_error_types`.
  - Fetches return signatures in nested per-error-type loops with per-row queries. Refactor to batch queries using `WHERE id = ANY($1::BIGINT[])`.
- [ ] Allow `source_context` to accept a fully-qualified type path.
  - Currently requires `path` (file path) + `line`/`symbol_name`. Agents often know a type/trait path but not which file/line it's in, requiring a `symbol_search` round-trip first.
- [ ] Add a standalone `crate_changelog` / release notes tool.
  - `crate_versions` returns version metadata without release notes. `crate_api_diff` includes `release_notes` but only when comparing two specific versions. A standalone tool would help agents evaluate upgrades.

## P1: Database & Indexing

- [x] ~~Add query cache invalidation on re-index.~~
  - ~~`query_cache` uses TTL-only expiration. When a crate is re-indexed via `index_refresh`, cached tool results are NOT invalidated. On successful re-index, issue targeted cache invalidation using structured cache keys with a crate name prefix.~~
- [ ] Add telemetry table retention policy.
  - `tool_invocations` and `query_cache_events` tables grow indefinitely. Add a periodic cleanup job (e.g., in the refresh worker loop) that deletes rows older than 30 days.
- [ ] Add refresh worker concurrency.
  - The refresh worker processes jobs sequentially. A single slow job blocks the entire queue. Allow configurable concurrency (e.g., 2-4 parallel jobs) using `tokio::JoinSet` or a semaphore-bounded task spawner.
  - Subsumes the existing "background refresh fairness/backpressure" item.

## P1: Architecture & Operations

- [ ] Add graceful request drain on shutdown.
  - `app.rs` uses `with_graceful_shutdown`, but there is no explicit drain period for in-flight MCP tool calls. Long-running tools may be killed mid-execution. Add a configurable drain timeout and ensure the refresh worker checkpoints on shutdown.
- [ ] Split `rust-mcp-types` into per-domain modules.
  - `types.rs` is ~2500+ lines containing all request/response types for 35 tools. Split into `types/krate.rs`, `types/dependency.rs`, `types/source.rs`, etc. with re-exports for backwards compatibility.

## P2: First-Touch Latency

- [ ] Evaluate whether first-touch latency needs further optimization.
  - On-demand indexing typically completes in 1-3s for most crates. The coordinator timeout ceiling is 45s, which could theoretically be hit for very large crates or slow docs.rs downloads, but this is uncommon in practice.
  - If latency becomes a problem for specific crates, options include: returning syn-only data immediately with a "rustdoc enrichment in progress" indicator, or adding `_meta.estimated_wait` to progress notifications based on stored indexing statistics.

## P2: GitHub Integration

- [ ] Community health metrics.
  - `GET /repos/{owner}/{repo}/community/profile` (authenticated) — community health percentage (has README, contributing guide, code of conduct, etc.).

## P2: Indexing and Operations

- [ ] Optional container-side rustdoc JSON generation workflow (nightly, bounded/isolated execution).
  - _**Background:** docs.rs does not have rustdoc-json built for all versions of all crates yet. Many crate versions published prior to May 2025 are not available for download._
  - Local rustdoc builds should be triggered automatically for discovered versions that lack docs.rs artifacts.
- [ ] More granular per-source/per-tool SLO metrics and alertable counters.
- [ ] CI split for faster feedback: lint, integration, and e2e lanes with artifact reuse.

## P2: Deployment Hardening

- [ ] Verify `MCP_HTTP_BIND` default (`127.0.0.1`) works correctly with Docker port mapping; document override to `0.0.0.0` for custom compose files.
- [ ] Add production troubleshooting guide (DB connection failures, outbound firewall issues, rustdoc 404s for old versions, first-run expectations).

## P2: Low-Priority Improvements

- [ ] Add adaptive rate limiting from API responses.
  - Rate limiters use fixed intervals. `crates.io`/`docs.rs` 429/Retry-After headers are not inspected. Add 429 response detection that feeds back into the rate limiter.
- [ ] Reduce `crate_hotspots` memory usage.
  - Reads entire file content via `fs::read_to_string()` for pattern matching at query time. Consider streaming, limiting to first N bytes per file, or performing pattern matching at index time.

## P2: Optional Advanced Intelligence

- [ ] Evaluate optional rust-analyzer-assisted enrichment for workspace-local, position-aware context where rustdoc/syn are insufficient.
- [ ] Consider feature-gated API surfacing tools once rustdoc JSON exposes structured feature-gate data (currently only raw `Attribute::Other(String)` strings — not actionable).

## P2: Testing Gaps

- [ ] Add integration/e2e tests for `crate_import_path`.
  - The only tool that still lacks both integration and e2e tests.
- [ ] Add cache hit/miss testing.
  - The query cache layer has no dedicated tests verifying cache hit behavior, TTL expiration, or invalidation.
- [ ] Add unicode/special character edge case tests.
  - Search tools are not tested with unicode crate names, emoji in descriptions, or special regex characters in queries.

## Out of Scope (For Now)

- Multi-tenant remote deployment model.
- Non-Rust ecosystems.
- Full Cargo resolver parity as a hard guarantee (current behavior remains best-effort with confidence signaling).
