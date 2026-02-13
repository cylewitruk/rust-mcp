# Implementation Checklist (Execution Order + Effort)

This checklist converts the draft spec into a delivery sequence with dependency order and rough effort.

## 1) Foundation and Control Plane

- [x] MCP HTTP server + health/readiness + Docker baseline
- [x] Core crate indexing/search/intel tools
- [x] Initial schema + FTS/trigram indexes
- [x] `index.status` and `index.refresh` control tools (first version)
- [x] Durable refresh job table and background worker loop
- [x] Retry lifecycle (`pending -> running -> finished|pending(retry)|failed`)
- [x] Jittered bounded retry backoff and due-time dequeue gating
- [x] Interaction-triggered freshness checks in `crate.search` and `crate.intel`
- [x] Queue/retry/failure telemetry in `index.status`

**Estimate:** complete

## 2) Complete M1 Crate Intelligence

- [x] `crate.versions` tool
- [x] `crate.graph` tool (depth-bounded, dependencies/dependents/both)
- [x] RustSec/OSV ingest pipeline into `advisory_matches` (initial OSV scope via `index.refresh` security)
- [x] Freshness metadata per source in all index and crate responses

**Estimate:** 2–4 days

## 3) Local Source Indexing (M2)

- [x] Cargo registry scanner (`~/.cargo/registry/src`) with change detection
- [x] `source_files` ingestion + content hashing + incremental updates
- [x] `source.search` tool (text + regex with bounds)
- [x] `source.read` tool (path + line range)

**Estimate:** 3–5 days

## 4) Symbol Intelligence (M3)

- [x] Parser-based symbol extraction (`syn`/tree-sitter fallback)
- [x] `symbol.search` tool with confidence and index source
- [x] Version-aware symbol queries and pagination limits

**Estimate:** 3–6 days

## 5) Docs Intelligence + Quality Contract (M4)

- [x] docs.rs ingest/index pipeline
- [x] `docs.search` tool
- [x] Standardize response envelope fields: `provenance`, `freshness`, `confidence`, `next_best_calls` (implemented across crate-search/intel/versions/graph)
- [x] Basic memoization via `query_cache`

**Estimate:** 3–6 days

## 6) Reliability + NFR Validation

- [x] Metrics (`query_count`, `latency`, `cache_hit_rate`, `index_lag`, `error_rate`)
- [x] Benchmarks for top crates (latency/correctness)
- [x] Concurrency and backpressure tests

**Estimate:** 2–4 days

## Suggested Immediate Sprint (next 3–5 coding sessions)

1. Concurrency and backpressure tests (parallel tool invocations)
2. RustSec native ingest parity (beyond OSV bridge)
3. Optional Prometheus metrics export
4. Cache invalidation hooks on refresh scopes
5. Benchmark trend reporting (7-day rolling)
