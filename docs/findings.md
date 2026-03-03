# rust-mcp Comprehensive Review Report

**Date:** 2026-02-18
**Scope:** Full codebase review covering tool coverage, protocol conformance, security, performance, testing, and agent effectiveness gaps.

---

## Executive Summary

`rust-mcp` is a well-architected, production-quality MCP server providing local-first Rust dependency intelligence. The codebase demonstrates strong engineering fundamentals: clean separation of concerns, excellent SQL injection protection, consistent response envelope contracts, and solid MCP protocol conformance. The 34-tool surface area covers the most critical needs of Rust LLM agents, with a mature indexing pipeline, test infrastructure, and Docker-first deployment model.

This report identifies **31 actionable findings** organized by priority. The project is already past the "works correctly" bar; the findings below focus on gaps that would most move the needle for **agent effectiveness**, **operational reliability**, and **long-term maintainability**.

---

## 1. Tool Coverage Gaps for Rust LLM Agents

These are the highest-impact findings -- missing tools or capabilities that would materially improve an agent's ability to write correct Rust code.

### F1. No `crate.docs` or `crate.example` Tool (HIGH)

**Current state:** `docs.search` does full-text search over scraped docs.rs HTML pages. There is no tool that returns structured documentation for a specific item (e.g. "show me the docs for `serde_json::Value::as_str`"), nor a way to retrieve runnable examples/doctests from a crate.

**Impact:** When an agent needs to understand how to use an API, it must piece together information from `crate.api`, `crate.type_info`, and `docs.search` -- none of which return the actual doc comments or examples that a human would read.

**Recommendation:** Add a `crate.docs` tool (or `docs.item`) that, given a crate + item path, returns the rendered doc comment and any embedded examples. Rustdoc JSON already contains `docs` strings for each item -- this data is being ingested but not surfaced directly.

### F2. No Workspace-Aware Tools (HIGH)

**Current state:** Tools like `dependency.resolve` accept a `cargo_toml_path`, but there is no workspace-level analysis. Agents working in real projects (which are overwhelmingly workspace-based) cannot ask "what crates does this workspace use?" or "are there dependency conflicts across workspace members?"

**Impact:** Agents must manually parse workspace structure and issue per-member queries, which is token-expensive and error-prone.

**Recommendation:** Add a `workspace.analyze` or `dependency.workspace_audit` tool that accepts a workspace root and returns aggregated dependency/version/conflict information across all members.

### F3. No `crate.changelog` / Release Notes Tool (MEDIUM)

**Current state:** `crate.versions` returns version metadata and `crate.api_diff` shows API changes. Neither surfaces changelog/release notes text.

**Impact:** When upgrading dependencies, agents need to understand *what changed semantically*, not just *which symbols were added/removed*. Many crates publish CHANGELOG.md or GitHub release notes that describe migration steps.

**Recommendation:** Add a `crate.changelog` tool or enrich `crate.versions` with release notes content (from crate README diffs, GitHub release API, or CHANGELOG.md if present in source files).

### F4. `source.context` Only Supports Line-Based Navigation (MEDIUM)

**Current state:** `source.context` accepts `line` or `symbol_name` + file path. It cannot navigate by a fully-qualified type path (e.g., "show me the context around `serde::de::Deserialize`").

**Impact:** Agents often know a type/trait path but not which file/line it's in. They must first call `symbol.search`, then `source.context` -- a round-trip that could be a single call.

**Recommendation:** Allow `source.context` to accept a fully-qualified path as an alternative to file+line, performing the symbol lookup internally.

### F5. No `crate.msrv` or Rust Edition Compatibility Tool (MEDIUM)

**Current state:** MSRV is stored in `crate_versions.msrv` but is not surfaced through any dedicated tool. Agents have no way to ask "is this crate compatible with Rust 1.70?" or "what's the minimum supported Rust version for serde 1.0.200?"

**Impact:** Rust edition and MSRV compatibility are critical for real project dependency decisions. Agents currently have no efficient way to check this.

**Recommendation:** Surface MSRV in `crate.intel` response, and consider a `crate.msrv_check` tool or adding MSRV filtering to `crate.search`.

---

## 2. Protocol & Transport

### F6. Protocol Version Gap: 2025-03-26 vs 2025-11-25 (MEDIUM)

**Current state:** `protocol.rs:7` declares `SUPPORTED_MCP_PROTOCOL_VERSION = "2025-03-26"` while acknowledging `LATEST_MCP_PROTOCOL_VERSION = "2025-11-25"`. The server correctly downgrades clients requesting newer versions.

**Impact:** As MCP clients update, the gap between supported and latest grows. Clients targeting 2025-11-25 features will silently lose them during negotiation.

**Recommendation:** Evaluate the 2025-11-25 spec additions and prioritize upgrading. This is already acknowledged in the roadmap but worth tracking as a concrete milestone.

### F7. Prometheus Bind Defaults to 0.0.0.0 (LOW)

**Current state:** `PROMETHEUS_BIND` defaults to `0.0.0.0:9090` (all interfaces), while `MCP_HTTP_BIND` correctly defaults to `127.0.0.1:43173`.

**Impact:** In non-Docker environments, the metrics endpoint would be publicly accessible by default. Docker Compose mitigates this with `127.0.0.1` port binding.

**Recommendation:** Change the default `PROMETHEUS_BIND` to `127.0.0.1:9090` for defense-in-depth. Users who need external access can override.

### F8. No MCP `resources` or `prompts` Capability (LOW -- INFORMATIONAL)

**Current state:** Only `tools` capability is advertised. MCP spec also supports `resources` (for streaming/subscribable data) and `prompts` (for template-based interactions).

**Impact:** Low for current use case. Resources could be useful if agents wanted to subscribe to index status changes. Prompts could provide guided workflows (e.g., "help me migrate from crate A to crate B").

**Recommendation:** No immediate action. Track as potential P2 enhancement.

---

## 3. Database & Indexing

### F9. Query Cache Has No Cross-Tool Invalidation (MEDIUM)

**Current state:** `query_cache` uses TTL-only expiration. When a crate is re-indexed via `index.refresh`, cached tool results for that crate are NOT invalidated -- they persist until TTL expires.

**Impact:** After a refresh, an agent could receive stale cached results until the TTL window passes. This is especially problematic for tools with longer cache TTLs.

**Recommendation:** On successful `persist_crate_sync`, issue a `DELETE FROM query_cache WHERE key LIKE '%{crate_name}%'` or use structured cache keys with a crate name prefix to enable targeted invalidation.

### F10. Unbounded Telemetry Table Growth (MEDIUM)

**Current state:** `tool_invocations` and `query_cache_events` tables grow indefinitely. There is no background job or retention policy to prune old rows.

**Impact:** Over time (weeks/months of continuous use), these tables will degrade query performance for `index.status` metrics and consume disk. The 24h operational metrics queries already touch these tables.

**Recommendation:** Add a periodic cleanup job (e.g., in the refresh worker loop) that deletes rows older than 30 days. Alternatively, add a migration with a retention policy index.

### F11. N+1 Query Patterns in Some Tools (MEDIUM)

**Current state:** `crate.error_types` fetches return signatures in a per-error-type loop. `crate.alternatives` has a similar pattern when fetching candidate details.

**Impact:** For crates with many error types or many alternatives, this generates proportional database round-trips, degrading latency.

**Recommendation:** Refactor to batch queries using `WHERE id = ANY($1::BIGINT[])` or equivalent `IN` clause patterns.

### F12. Missing Database Indexes for Refresh Jobs (LOW)

**Current state:** No index on `refresh_jobs(status, finished_at)` for cleaning up old jobs. The `prune_stale_source_file_index_rows` function uses `NOT (path = ANY($2::TEXT[]))` which can full-scan on large path arrays.

**Recommendation:** Add targeted indexes: `CREATE INDEX ON refresh_jobs(status, finished_at)` and evaluate the path pruning query for large registries.

### F13. Rate Limiter Uses Mutex Instead of Semaphore (LOW)

**Current state:** `OutboundRateLimiter` uses a `Mutex<Instant>` per source, which serializes all concurrent callers through a single lock.

**Impact:** Under high concurrency (many parallel tool calls hitting crates.io), all tasks serialize on the mutex. Tokio semaphore would allow concurrent requests while still enforcing minimum interval.

**Recommendation:** Consider `tokio::sync::Semaphore` or a sliding-window rate limiter for better throughput under load. Current approach is adequate for typical single-user workloads.

---

## 4. Tool Implementation Quality

### F14. Inconsistent Freshness Reporting (MEDIUM)

**Current state:** Most tools report `freshness` arrays with `local_postgres_index` + external source entries. However, `symbol.search` is missing a `freshness` array entirely, and `derive_macros`, `source/context` only provide a single `crates.io` entry.

**Impact:** Agents that rely on freshness metadata to decide whether to trust a result will get inconsistent signals across tools.

**Recommendation:** Standardize all tool responses to include at least `[{source: "local_postgres_index", ...}, {source: "crates.io" | "docs.rs", ...}]`. Create a shared helper to build the freshness array.

### F15. Cursor Encoding Duplicated Across Files (LOW)

**Current state:** Base64 cursor encode/decode logic is repeated in `crate/search.rs`, `source/search.rs`, `docs.rs`, `symbol.rs`, and several other tools.

**Impact:** Maintenance burden and risk of divergent cursor formats.

**Recommendation:** Extract into a shared `cursor` module in `mcp/utils.rs` with versioned encode/decode functions.

### F16. `crate.hotspots` Loads Full File Content (LOW)

**Current state:** The hotspots tool loads entire file content for up to 1000 files to perform unsafe/concurrency pattern matching.

**Impact:** For large crates with many files, this could consume significant memory.

**Recommendation:** Consider streaming or limiting content to the first N bytes per file, or performing the pattern matching at index time rather than query time.

### F17. `source.context` 200-Line Distance Filter is Arbitrary (LOW)

**Current state:** `source/context.rs` filters out containing `impl` blocks more than 200 lines away from the query line.

**Impact:** In large files with big impl blocks, the containing impl may be filtered out, reducing context quality.

**Recommendation:** Make this threshold configurable via request parameter, or use a smarter heuristic based on impl block size.

---

## 5. Testing Gaps

### F18. 12 Tools Have No Direct Test Coverage (HIGH)

**Current state:** ~23 of 35 tools are directly tested. The following have no integration or e2e tests:

| Tool | Category |
|------|----------|
| `crate.compare` | Comparison |
| `crate.compatibility` | Comparison |
| `crate.compatibility_matrix` | Comparison |
| `crate.migration_path` | Comparison |
| `crate.license_check` | Policy |
| `crate.derive_macros` | Type Intel |
| `crate.error_types` | Type Intel |
| `crate.import_path` | Type Intel |
| `crate.graph` | Analysis |
| `crate.hotspots` | Analysis |
| `crate.usage_patterns` | Analysis |
| `source.context` | Source |

**Impact:** These tools can regress silently. Several (graph, hotspots, migration_path) have complex logic that is particularly regression-prone.

**Recommendation:** Prioritize adding integration tests for these tools, starting with `crate.graph` and `crate.migration_path` which have the most complex logic. The existing fixture infrastructure (seeded_mcp_context, MockCratesIoServer, rustdoc fixtures) makes this straightforward.

### F19. No Cache Hit/Miss Testing (MEDIUM)

**Current state:** The query cache layer is used throughout tools but has no dedicated tests verifying cache hit behavior, TTL expiration, or invalidation.

**Recommendation:** Add integration tests that call a tool twice and verify the second call returns cached results faster, then wait for TTL and verify fresh fetch.

### F20. No Performance Regression Baselines (MEDIUM)

**Current state:** No benchmark tests or latency baselines. Test infrastructure exists (tool_invocations metrics) but isn't used in CI to detect regressions.

**Recommendation:** Add a `bench/` or `benches/` target with criterion benchmarks for the most-used tools (search, intel, type_info). Track latency in CI.

### F21. No Unicode/Special Character Edge Cases (LOW)

**Current state:** Search tools are not tested with unicode crate names, emoji in descriptions, or special regex characters in queries.

**Recommendation:** Add edge case tests with non-ASCII inputs to verify PostgreSQL trigram/FTS and query parameter escaping handle them correctly.

---

## 6. Security

### F22. SQL Injection Protection: EXCELLENT (INFORMATIONAL)

All user-provided inputs use `QueryBuilder::push_bind()` consistently. No string interpolation into SQL was found across any tool handler. This is the gold standard.

### F23. Outbound Firewall DNS Resolution is Static (LOW) -- RESOLVED

**Previous state:** The Docker entrypoint resolved domain names to IP addresses at container startup and pinned them in iptables rules. CDN IP rotation caused stale rules and connection failures.

**Resolution:** Replaced with tinyproxy-based domain-level egress filtering. tinyproxy resolves DNS on each CONNECT request, so IP rotation is handled transparently. The `OUTBOUND_FIREWALL` and `OUTBOUND_ALLOWLIST` env vars remain unchanged; `NET_ADMIN` capability and `iptables` are no longer required.

### F24. `dependency.resolve` Accepts `cargo_toml_path` -- Path Traversal (LOW)

**Current state:** `resolve_manifest_path()` normalizes the path and checks `starts_with("/cargo/registry")`, effectively sandboxing reads to the mounted cargo registry.

**Impact:** The sandboxing is correct. However, if `CARGO_REGISTRY_DIR` were misconfigured to `/`, this would allow reading arbitrary files. The check is tied to the configured directory.

**Recommendation:** No code change needed. Ensure documentation clearly notes that `CARGO_REGISTRY_DIR` must be set correctly for security.

---

## 7. Architecture & Operations

### F25. No Graceful Request Drain on Shutdown (MEDIUM)

**Current state:** `app.rs` handles SIGTERM/SIGINT and calls `axum::serve::with_graceful_shutdown`, but there is no explicit drain period for in-flight MCP tool calls. Long-running tools (e.g., `index.sync_crates` with network fetches) may be killed mid-execution.

**Impact:** In Docker with orchestrators, stop signals have a default 10s timeout. Tools that take >10s risk incomplete state.

**Recommendation:** Add a configurable drain timeout (e.g., 30s) and ensure the refresh worker gracefully checkpoints on shutdown.

### F26. Refresh Worker Has No Concurrency Limit (MEDIUM)

**Current state:** The refresh worker claims and processes jobs sequentially in a single loop. If many crates need refresh simultaneously (e.g., after a large `index.sync_crates`), the queue grows unboundedly.

**Impact:** Fresh data availability lags. A single slow job (e.g., large rustdoc JSON fetch) blocks the entire queue.

**Recommendation:** Allow configurable worker concurrency (e.g., 2-4 parallel refresh jobs) using `tokio::JoinSet` or a semaphore-bounded task spawner.

### F27. No Adaptive Rate Limiting from API Responses (LOW)

**Current state:** Rate limiters use fixed intervals. Neither `crates.io` nor `docs.rs` 429/Retry-After headers are inspected.

**Impact:** If crates.io begins rate limiting the server, it will continue hammering at the configured interval rather than backing off.

**Recommendation:** Add 429 response detection in the HTTP clients that feeds back into the rate limiter, temporarily increasing the minimum interval.

---

## 8. Type System & API Design

### F28. `rust-mcp-types` is Large and Monolithic (MEDIUM)

**Current state:** `types.rs` contains ~133 structs/enums with ~285 Optional fields in a single 1700+ line file. Request and response types for all 34 tools live here.

**Impact:** Difficult to navigate, increases compile times for downstream crates that only need a subset.

**Recommendation:** Split `types.rs` into per-domain modules mirroring the tool structure: `types/krate.rs`, `types/dependency.rs`, `types/source.rs`, etc. Re-export from `types/mod.rs` for backwards compatibility.

### F29. `crate.intel` Response is Very Large (LOW)

**Current state:** `crate.intel` aggregates versions, dependencies, dependents, advisories, features, and statistics into a single response. For popular crates, this can be token-expensive for agents.

**Impact:** Agents pay a high token cost to receive information they may not need.

**Recommendation:** Consider adding optional `include` or `exclude` fields to `crate.intel` to let agents request only the sections they need (e.g., `include: ["advisories", "latest_version"]`).

---

## 9. Documentation & Developer Experience

### F30. Tool Descriptions Could Include Usage Hints (MEDIUM)

**Current state:** Tool descriptions (in `#[tool(description = "...")]`) are concise but don't include the typical "call this when..." guidance that helps agents decide which tool to use.

**Impact:** Agents with access to 34 tools need strong signal about when to use each. Better descriptions reduce tool selection errors.

**Recommendation:** Expand tool descriptions to include trigger conditions. Example: `"Search locally indexed crates by name, category, or keyword. Call this first when the user mentions a dependency by name or when exploring alternatives to a known crate."` The existing `instructions` field in `ServerInfo` provides high-level guidance; per-tool hints would complement it.

### F31. `suggested_next_tools` Could Be Context-Sensitive (LOW)

**Current state:** Each tool returns static `suggested_next_tools` arrays (e.g., `crate.search` always suggests `["crate.intel", "crate.features"]`).

**Impact:** The suggestions don't adapt to what the agent has already learned. After calling `crate.intel`, suggesting it again as a next step is unhelpful.

**Recommendation:** Consider making `suggested_next_tools` context-sensitive based on the response content. For example, if a crate has advisories, include `"dependency.audit"`. If it has many features, include `"crate.features"`. This requires modest logic but significantly improves agent workflow efficiency.

---

## Priority Summary

| Priority | Finding IDs | Theme |
|----------|-------------|-------|
| **P0** | F1, F18 | Missing item-level docs tool; 12 untested tools |
| **P1** | F2, F9, F10, F11, F14, F25, F26, F30 | Workspace tools, cache invalidation, telemetry retention, N+1 queries, freshness consistency, graceful shutdown, worker concurrency, tool descriptions |
| **P2** | F3, F4, F5, F6, F7, F15, F19, F20, F27, F28, F29, F31 | Changelogs, path-based context, MSRV, protocol version, cursor dedup, cache testing, benchmarks, rate limiting, types split, intel filtering, dynamic suggested_next_tools |
| **P3** | F8, F12, F13, F16, F17, F21, F23, F24 | MCP resources/prompts, minor indexes, rate limiter internals, hotspot memory, context threshold, unicode tests, DNS resolution, path docs |

---

## What's Already Excellent

To be clear about what **doesn't need fixing:**

- **SQL safety** -- universal parameterized queries, zero injection vectors found
- **MCP protocol conformance** -- session lifecycle, error envelopes, progress notifications, version negotiation all well-tested
- **Response envelope consistency** -- `confidence`, `confidence_assessment`, `suggested_next_tools`, `provenance` present on 27/27 applicable tools
- **Indexing pipeline** -- adaptive TTL freshness, SHA256-based dedup, retry with exponential backoff + jitter
- **Docker deployment model** -- privilege separation, optional outbound firewall, health/readiness probes
- **Test infrastructure** -- testcontainers, mock registries, deterministic fixtures, strong protocol assertion coverage
