# ROADMAP

Status: Active  
Last updated: 2026-02-17

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
- [ ] Improve `crate.api_diff`, `crate.type_info`, and `crate.trait_impls` prioritization logic when both syn and rustdoc-derived data exist.
  - [x] `crate.type_info` and `crate.trait_impls` now collapse duplicate dual-source impl rows and prioritize richer rustdoc-backed metadata over sparse duplicates.
  - [ ] Remaining: extend dual-source prioritization heuristics to `crate.api_diff`.
- [x] Add richer diagnostics for rustdoc ingestion failures (bad files, version mismatches, parse failures) with actionable error messages.
  - Rustdoc ingestion now reports unsupported `format_version` mismatches explicitly and appends targeted hints for fallback/configuration/decode failures.
- [ ] Define and document data freshness/confidence behavior specifically for rustdoc-backed responses.

## P1: Tooling and UX Improvements

- [x] Add `crate.import_path` tool (best-known public import path resolution).
  - `crate.import_path` now resolves best-known public import paths and alternative matches from indexed symbol metadata.
- [ ] Improve `crate.migration_path` heuristics beyond simple rename candidates.
- [ ] Strengthen response contracts for pagination/cursors and truncation indicators across all search-style tools.
- [x] Publish a machine-readable tool contract snapshot for client generation/testing.
  - `schema.get` now exposes per-tool request/response JSON Schemas over MCP.
  - HTTP endpoints now expose the same schema catalog at `/schemas` and `/schemas/{tool_name}`.
  - `SCHEMA_EXPORT_DIR` now writes `tool-schemas.json` plus per-tool artifacts at startup.
- [ ] Publish a separate `rust-mcp-stdio` adapter binary that bridges stdio MCP clients to a running rust-mcp HTTP instance.
  - _**Background:** As `rust-mcp` is designed to run as a single instance/server in Docker, MCP clients which do not support remote/HTTP(S) MCP server configurations will not be able to use it. The major players (Claude Code, Codex, Copilot) all support remote server configurations, so this is a compatibility-maximizing point to reach a broader userbase._

## P2: Indexing and Operations

- [ ] Optional container-side rustdoc JSON generation workflow (nightly, bounded/isolated execution).
  - _**Background:** docs.rs does not have rustdoc-json built for all versions of all crates yet. Many crate [versions] published prior to May, 2025-ish are not available for download._
- [ ] Background refresh fairness/backpressure tuning for large local registries.
- [ ] More granular per-source/per-tool SLO metrics and alertable counters.
- [ ] CI split for faster feedback: lint, integration, and e2e lanes with artifact reuse.

## P2: Optional Advanced Intelligence

- [ ] Evaluate optional rust-analyzer-assisted enrichment for workspace-local, position-aware context where rustdoc/syn are insufficient.
- [ ] Consider new tools for deprecations and feature-gated API surfacing after rustdoc data quality goals are met.

## Out of Scope (For Now)

- Multi-tenant remote deployment model.
- Non-Rust ecosystems.
- Full Cargo resolver parity as a hard guarantee (current behavior remains best-effort with confidence signaling).
