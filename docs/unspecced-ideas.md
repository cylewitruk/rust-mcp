# Unspecced Ideas (Parking Lot)

This file captures ideas aligned with project goals but not yet formally specced.

## 1) Prometheus Metrics Endpoint

- Add `/metrics` endpoint with Prometheus format for:
  - request throughput by tool,
  - p50/p95 latency by tool,
  - cache hit/miss rates,
  - refresh queue depth and failures.
- Keep current SQL-backed metrics for internal audit; expose Prometheus for ops dashboards.

## 2) Cache Invalidation Hooks

- Invalidate memoized `symbol.search`/`docs.search` entries after relevant refresh operations (`local_cache`, `docs`).
- Current TTL-only policy is simple but can keep stale cached responses briefly.

## 3) Materialized Dashboard Views

- Add DB views/materialized views for 24h and 7d performance windows.
- Reduces repeated aggregate cost on `index.status` and enables faster trend queries.

## 4) Adaptive Cache TTL

- Use dynamic TTL based on index freshness and tool volatility:
  - Shorter TTL during heavy refresh cycles,
  - Longer TTL when index is stable.

## 5) Benchmark Harness

- Build benchmark scripts for top crates and representative queries:
  - cold vs warm cache,
  - p95 latency targets,
  - throughput under concurrent tool calls.

## 6) Deeper Symbol Semantics

- Optional mode to collapse symbol results by canonical path/name across versions.
- Useful for “API discovery” without duplicate symbol results across versions.

## 7) Native RustSec DB Ingest

- Add direct RustSec advisory-db ingest (Git-backed or packaged snapshot), not only OSV bridge data.
- Enables richer metadata parity (withdrawn state, advisory categories, unaffected ranges) and deterministic advisory source provenance.
