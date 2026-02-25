# ROADMAP

Status: Active
Last updated: 2026-02-24

This file tracks only open work and future direction.

## Baseline Completed

The previously planned milestone set through M13 is complete, including:

- core indexing and refresh pipeline
- expanded crate/dependency/source intelligence tools
- confidence contract and progress notifications
- modular integration/e2e test layout and broad tool-call coverage

## P0: Correctness and Protocol Completeness

- [x] Remove `stdio` from core runtime config until a dedicated adapter exists.
  - `rust-mcp` is now explicitly HTTP-only (Streamable HTTP).
  - `MCP_TRANSPORT` and `TransportMode` were removed from runtime config.
- [x] Add stricter MCP protocol conformance checks around session lifecycle edge cases (invalid ordering, missing/expired session headers, malformed request behavior).
  - Added e2e checks for missing session headers after successful initialize/initialized flow (`tools/list`, `tools/call`).
  - Added e2e checks for malformed JSON request bodies and invalid JSON-RPC request shapes.
- [x] Expand e2e protocol assertions for non-happy-path JSON-RPC/MCP error envelopes and status mappings.
  - Added e2e matrix assertions for JSON-RPC error envelope shape (`jsonrpc`, `id`, `error.code`, `error.message`) on successful transport responses.
  - Added stricter 400/401/422 status assertions for malformed and unauthorized protocol paths.
- [x] Add clear compatibility policy for supported MCP protocol versions.
  - Policy is now documented in `README.md` (`MCP Protocol Version Policy`).
  - Latest published version and currently negotiated server version are both explicitly documented.
  - Default initialize examples/test harness constants now use a shared protocol constant to avoid drift.

## P1: Rustdoc Intelligence Quality

- [ ] Complete rustdoc JSON enrichment for type/impl/trait fidelity (including better canonical path handling for re-exports).
  - [x] Canonical-path mapping now prefers shortest public re-export path from rustdoc `Use` graph.
  - [x] `crate.re_exports` now prefers rustdoc canonical/definition-path pairs when available.
  - [x] `crate.type_info` and `crate.trait_impls` now expose richer rustdoc impl metadata (`is_blanket`, `is_synthetic`, `is_negative`, `blanket_type`, generics, where-clauses).
  - [x] `crate_traits` metadata is now surfaced in `crate.type_info` and `crate.trait_impls` responses as `trait_definitions`.
  - [ ] Remaining: improve canonicalization coverage for complex glob/re-export edge cases and external-path remapping nuances.
- [x] Improve `crate.api_diff`, `crate.type_info`, and `crate.trait_impls` prioritization logic when both syn and rustdoc-derived data exist.
  - [x] `crate.type_info` and `crate.trait_impls` now collapse duplicate dual-source impl rows and prioritize richer rustdoc-backed metadata over sparse duplicates.
  - [x] `crate.api_diff` now prefers rustdoc-backed symbol rows when dual-source duplicates exist.
- [x] Add richer diagnostics for rustdoc ingestion failures (bad files, version mismatches, parse failures) with actionable error messages.
  - Rustdoc ingestion now reports unsupported `format_version` mismatches explicitly and appends targeted hints for fallback/configuration/decode failures.
- [x] Define and document data freshness/confidence behavior specifically for rustdoc-backed responses.
  - Documented in `README.md` (`Rustdoc Freshness and Confidence`) with tool-level behavior notes for `crate.re_exports`, `crate.import_path`, `crate.type_info`, `crate.trait_impls`, and `crate.api_diff`.

## P1: Tooling and UX Improvements

- [x] Add `crate.import_path` tool (best-known public import path resolution).
  - `crate.import_path` now resolves best-known public import paths and alternative matches from indexed symbol metadata.
- [x] Improve `crate.migration_path` heuristics beyond simple rename candidates.
  - `crate.migration_path` now scores replacement candidates using kind matching, token overlap, and signature compatibility (including normalized function-shape matching).
  - Migration rationales now include richer guidance for signature and visibility changes, plus likely replacement suggestions for removed symbols.
  - Added focused unit coverage for replacement-candidate scoring and rationale enrichment behavior.
- [x] Strengthen response contracts for pagination/cursors and truncation indicators across all search-style tools.
  - [x] Standardized `page`/`cursor`/`next_cursor` plus `has_more`/`truncated` metadata for `crate.search`, `source.search`, and `docs.search` (matching existing `symbol.search` behavior).
  - [x] Extended the same metadata contract to `crate.versions`, `crate.alternatives`, `crate.hotspots`, and `crate.usage_patterns`.
  - [x] Extended the same metadata contract to remaining limit-based crate intelligence tools: `crate.api`, `crate.re_exports`, `crate.import_path`, `crate.error_types`, and `crate.trait_impls`.
- [x] Publish a machine-readable tool contract snapshot for client generation/testing.
  - `schema.get` now exposes per-tool request/response JSON Schemas over MCP.
  - HTTP endpoints now expose the same schema catalog at `/schemas` and `/schemas/{tool_name}`.
  - `SCHEMA_EXPORT_DIR` now writes `tool-schemas.json` plus per-tool artifacts at startup.
- [x] Publish a separate `rust-mcp-stdio` adapter binary that bridges stdio MCP clients to a running rust-mcp HTTP instance.
  - Added `rust-mcp-stdio` crate with MCP stdio framing, Streamable HTTP forwarding, session header propagation, and JSON/SSE response passthrough.
  - Adapter behavior is tool-agnostic pass-through, so all tools exposed by the upstream `rust-mcp` HTTP server are supported.

## P1: On-Demand Indexing

- [x] Transparent on-demand crate indexing so MCP clients never need to call `index.*` tools explicitly.
  - [x] Added `IndexingCoordinator` (`mcp/indexing/coordinator.rs`) with `tokio::sync::Notify` for worker wake-up and `tokio::sync::watch` channels for per-job completion signaling.
  - [x] `fetch_crate_context()` now calls `ensure_crate_indexed()` before the DB lookup — all crate tools gain on-demand indexing transparently with no signature changes.
  - [x] Refresh worker idle loop replaced with `select!{notified, sleep(2s)}` so on-demand jobs are picked up immediately.
  - [x] Concurrent requests for the same unindexed crate coalesce via `enqueue_or_get_refresh_job_id` deduplication and shared `watch` channels.
  - [x] Fixed pre-existing bug in `enqueue_or_get_refresh_job_id` (`fetch_one` → `fetch_optional` for the existence check).
  - [x] Integration tests: happy-path on-demand indexing, nonexistent-crate error, concurrent coalescing.
  - [x] Follow-up: add MCP progress notifications (`notifications/progress` SSE) during on-demand waits by upgrading tool handler signatures to accept `meta: Meta, client: Peer<RoleServer>`.
    - All 33 instrumented tools now use `instrument_tool_with_progress` with `meta: Meta` and `client: Peer<RoleServer>` parameters.
    - Heartbeat improved from single-shot 5s pulse to periodic 5s interval for sustained client feedback during long on-demand waits.
  - [x] Follow-up: extend on-demand indexing to version-level (`backfill_missing_requested_version`) and source/rustdoc scopes.
    - `enqueue_on_demand` now accepts a `scope` parameter (crate, local_cache, rustdoc_json) instead of hardcoding `"crate"`.
    - `backfill_missing_requested_version` reworked from inline `sync_single_crate` to coordinator enqueue+wait for consistency and observability.
    - Added `ensure_source_indexed` and `ensure_rustdoc_indexed` best-effort helpers in `queries.rs` — failures are logged as warnings but do not error out tool calls.
    - Source tools (`source.search`, `source.read`, `source.context`) now trigger on-demand `local_cache` indexing when source files are missing for a version.
    - Rustdoc-backed tools (`crate.type_info`, `crate.trait_impls`, `crate.re_exports`, `crate.import_path`, `crate.api_diff`, `crate.api`, `crate.error_types`) now trigger on-demand `rustdoc_json` indexing when rustdoc symbols are missing.
    - `crate.derive_macros` triggers on-demand source indexing (uses syn parsing, not rustdoc).

## P1: Protocol and Reliability

- [ ] Audit and close MCP protocol version gap (negotiated: `2025-03-26`, latest published: `2025-11-25`).
  - Newer MCP clients may expect features or behaviors from later spec revisions.
  - Requires reviewing the spec changelog between the two versions, updating protocol constants, and adjusting transport/session handling if needed.
- [ ] Replace `.expect()` calls in signal handler setup (`app.rs`) with proper error propagation.
  - `ctrl_c()` and `signal(SignalKind::terminate())` both use `.expect()` today, which panics on failure rather than reporting a clean startup error.

## P1: First-Touch Latency

- [ ] Reduce first-interaction latency for on-demand rustdoc indexing.
  - On-demand `rustdoc_json` jobs currently block tool responses for up to 45s (coordinator timeout). Tools that trigger both crate + rustdoc on-demand can see 130–180s wall time on first touch.
  - Options to evaluate: configurable pre-warm list at startup, returning syn-only data immediately with a "rustdoc enrichment in progress" indicator, adding `_meta.estimated_wait` to progress notifications.
    - `_meta.estimated_wait` should be based on statistics stored by the indexing process to give accurate results.

## P1: Test Coverage

- [x] Add standalone integration tests for untested crate tools.
  - [x] Added standalone tests for `crate.type_info`, `crate.trait_impls`, `crate.error_types`, `crate.versions`, `crate.usage_patterns`, `crate.hotspots`, `crate.migration_path`, `crate.derive_macros`, `crate.compatibility`, `crate.compatibility_matrix`, `crate.license_check`, `crate.alternatives`, and `schema.get`.
  - [x] All 13 previously combo-only tools now have isolated test functions; combo tests retained for cross-tool workflow coverage.
  - Updated coverage: ~35/35 tools have direct standalone or focused-pair test cases.

## P2: Indexing and Operations

- [ ] Optional container-side rustdoc JSON generation workflow (nightly, bounded/isolated execution).
  - _**Background:** docs.rs does not have rustdoc-json built for all versions of all crates yet. Many crate [versions] published prior to May, 2025-ish are not available for download._
- [ ] Background refresh fairness/backpressure tuning for large local registries.
- [ ] More granular per-source/per-tool SLO metrics and alertable counters.
- [ ] CI split for faster feedback: lint, integration, and e2e lanes with artifact reuse.

## P2: Deployment Hardening

- [ ] Verify `MCP_HTTP_BIND` default (`127.0.0.1`) works correctly with Docker port mapping; document override to `0.0.0.0` for custom compose files.
- [ ] Add production troubleshooting guide (DB connection failures, outbound firewall issues, rustdoc 404s for old versions, first-run expectations).

## P2: Optional Advanced Intelligence

- [ ] Evaluate optional rust-analyzer-assisted enrichment for workspace-local, position-aware context where rustdoc/syn are insufficient.
- [ ] Consider new tools for deprecations and feature-gated API surfacing after rustdoc data quality goals are met.

## Out of Scope (For Now)

- Multi-tenant remote deployment model.
- Non-Rust ecosystems.
- Full Cargo resolver parity as a hard guarantee (current behavior remains best-effort with confidence signaling).
