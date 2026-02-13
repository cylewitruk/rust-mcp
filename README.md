# rust-mcp

Local-first Rust dependency intelligence MCP server.

This server provides:

- Loopback-only HTTP exposure for local agent clients.
- PostgreSQL with persistent Docker volume.
- Initial schema/migration for crate/version/source/symbol/docs indexing.
- Rust server with structured config, logging, health/readiness endpoints, and graceful shutdown.
- `rmcp` streamable HTTP transport mounted at `/mcp` with `ping`, `index.sync_crates`, `index.status`, `index.refresh`, `crate.search`, `crate.intel`, `crate.features`, `crate.api_diff`, `crate.api`, `crate.compare`, `crate.license_check`, `crate.alternatives`, `crate.versions`, `crate.graph`, `crate.hotspots`, `dependency.audit`, `source.search`, `source.read`, `symbol.search`, and `docs.search` tools.

## Quick start

1. Create your env file:

```bash
cp .env.example .env
```

1. Start Postgres + server:

```bash
docker compose up --build
```

1. Verify health:

```bash
curl http://127.0.0.1:43173/healthz
curl http://127.0.0.1:43173/readyz
```

1. Verify MCP initialize over streamable HTTP:

```bash
curl -X POST http://127.0.0.1:43173/mcp \
  -H "Content-Type: application/json" \
  -H "Accept: application/json, text/event-stream" \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"manual-test","version":"0.1.0"}}}'
```

1. Ingest crates.io data, then query the local index:

- Run `index.sync_crates` first (for example, query `serde`, `page=1`, `per_page=10`).
- Use `crate.search` for local search by `query`/`category`/`keyword`.
- Use `crate.intel` for dense metadata on one crate (`crate_name`, optional `version`).

## Tools

- `ping`: MCP connectivity and DB readiness probe.
- `index.sync_crates`: fetches crates.io metadata and upserts local crate/version/dependency records.
- `index.status`: returns index freshness, coverage counts, operational metrics (`query_count`, `average_latency_ms`, `cache_hit_rate`, `index_lag_seconds`, `error_rate`), queue state (`pending`/`delayed`/`retrying`/`running`/`failed`), retry-attempt distribution, and failures by scope.
- `index.refresh`: refreshes index scope (`crate`, `all`, `security`, `local_cache`, and `docs` implemented).
- `crate.search`: searches local Postgres index and performs bounded interaction freshness checks on top-ranked hits; reports `freshness_checks_performed` and `refresh_jobs_enqueued`.
- `crate.intel`: returns selected/latest versions, readme, dependencies, dependents, and advisory matches from local index; performs read-through freshness checks and can trigger inline minimal refresh + queued deep refresh.
- `crate.features`: returns indexed feature flags, default features, and transitive feature enables for a crate version.
- `crate.api_diff`: compares indexed public symbols between two versions and reports added/removed/changed API entries with breaking-change hints.
- `crate.api`: returns indexed public API symbols for a selected crate version with optional kind/path filters.
- `crate.compare`: compares two crates across adoption/risk/maintenance signals and returns recommendation reasons.
- `crate.license_check`: evaluates indexed license expression against optional allow/deny policy lists.
- `crate.alternatives`: returns ranked alternative crates using taxonomy overlap, adoption, and risk signals.
- `crate.versions`: returns normalized version timeline with yanked/security/adoption markers and interaction freshness metadata.
- `crate.graph`: returns depth-bounded dependency/dependent graph nodes and edges for `dependencies`, `dependents`, or `both` directions.
- `crate.hotspots`: detects unsafe/concurrency hotspots from indexed source content for a selected version.
- `dependency.audit`: audits a Cargo.toml dependency set for yanked versions, advisories, outdated constraints, unresolved deps, and MSRV conflicts.
- `source.search`: searches indexed source files by `text` or `regex` mode, with optional `crate_name`, `version`, and `path_glob` filters.
- `source.read`: returns a line-bounded slice of an indexed source file by `crate_name` + `path` (optionally pinning `version`).
- `symbol.search`: searches indexed symbols by symbol name with optional `crate_name`, `version`, and `kind` filters; supports opaque cursor pagination (`cursor`/`next_cursor`) with `page`+`limit` fallback, `include_all_versions` (default `false`, latest-version only), and `collapse_by_canonical` (default `false`) to deduplicate by canonical symbol identity across versions.
- `docs.search`: searches indexed docs.rs pages with optional `crate_name`, `version`, and `path_prefix` filters.

Quality contract fields:

- `confidence` (`high|medium|low`)
- `confidence_assessment` (`level` + `reason`)
- `next_best_calls` (ordered suggested follow-up tools)

`confidence` is retained for backward compatibility; `confidence_assessment` is the primary structured contract.

## Observability

- Prometheus metrics are exposed on a dedicated listener (default `127.0.0.1:9090` via `PROMETHEUS_BIND` in local runs).
- Tool invocation counts/latency are emitted via `instrument_tool()` and refresh worker gauges expose queue depth/state.

## Rate limiting and request throttling

- Outbound refresh traffic is rate-limited per source using interval controls (`CRATES_IO_MIN_INTERVAL_MS`, `DOCS_RS_MIN_INTERVAL_MS`, `OSV_MIN_INTERVAL_MS`).
- Inbound tool concurrency is bounded by `MAX_CONCURRENT_REQUESTS`.

## Adaptive TTL freshness

- Freshness is interaction-driven (`crate.search`, `crate.intel`) with stale-while-revalidate behavior.
- TTL is adaptive by crate activity (with bounded windows and jitter) to keep active crates fresh while reducing refresh pressure for stable crates.
- Deep refresh work is deduplicated and processed asynchronously by the durable `refresh_jobs` queue.

## Tool quick examples

All examples below are MCP `tools/call` `arguments` payloads.

### Root + index tools

- `ping`

```json
{ "message": "hello" }
```

- `index.sync_crates`

```json
{ "query": "serde", "page": 1, "per_page": 10, "include_dependencies": true }
```

- `index.status`

```json
{}
```

- `index.refresh`

```json
{ "scope": "local_cache", "crate_name": "serde", "page": 1, "per_page": 25 }
```

### Crate intelligence tools (`crate.*`)

- `crate.search`

```json
{ "query": "json", "category": "encoding", "limit": 10 }
```

- `crate.intel`

```json
{ "crate_name": "serde", "version": "1.0.228" }
```

- `crate.features`

```json
{ "crate_name": "tokio", "version": "1.48.0" }
```

- `crate.api_diff`

```json
{ "crate_name": "serde", "from_version": "1.0.220", "to_version": "1.0.228", "limit": 200 }
```

- `crate.api`

```json
{ "crate_name": "serde", "kinds": ["trait", "struct"], "path_glob": "src/**", "limit": 200 }
```

- `crate.compare`

```json
{ "left_crate": "anyhow", "right_crate": "thiserror" }
```

- `crate.license_check`

```json
{ "crate_name": "axum", "allow_licenses": ["MIT", "Apache-2.0"], "deny_licenses": ["GPL-3.0"] }
```

- `crate.alternatives`

```json
{ "crate_name": "reqwest", "limit": 5, "allow_licenses": ["MIT", "Apache-2.0"] }
```

- `crate.versions`

```json
{ "crate_name": "serde", "limit": 20 }
```

- `crate.graph`

```json
{ "crate_name": "serde", "direction": "dependencies", "depth": 2 }
```

- `crate.hotspots`

```json
{ "crate_name": "tokio", "include_unsafe": true, "include_concurrency": true, "limit": 100 }
```

### Dependency + source + symbol + docs tools

- `dependency.audit`

```json
{ "cargo_toml_path": "./Cargo.toml" }
```

- `source.search`

```json
{ "query": "unsafe", "crate_name": "tokio", "mode": "text", "limit": 20 }
```

- `source.read`

```json
{ "crate_name": "serde", "path": "src/ser/mod.rs", "start_line": 100, "end_line": 140 }
```

- `symbol.search`

```json
{ "query": "Serializer", "crate_name": "serde", "collapse_by_canonical": true, "limit": 25 }
```

- `docs.search`

```json
{ "query": "serialize", "crate_name": "serde", "path_prefix": "serde/", "limit": 10 }
```

`symbol.search` cursor notes:

- treat `cursor` as opaque and pass through `next_cursor`
- keep filters and page size consistent across paged calls

Query memoization:

- `symbol.search` and `docs.search` responses are memoized in `query_cache` with a short TTL (currently 300 seconds) for repeated identical requests.

## Refresh behavior (ADR-0001)

- Interaction-driven refresh: `crate.search` and `crate.intel` are freshness triggers.
- Stale-while-revalidate flow: stale crates may be minimally refreshed inline, with deep refresh enqueued in `refresh_jobs`.
- Missing requested version flow: targeted inline backfill is attempted, then deep refresh is queued.
- Background worker: processes due jobs from `refresh_jobs`, retries failures with jittered bounded backoff, and marks terminal failures after max attempts.
- Security refresh: `index.refresh` with `scope=security` ingests OSV advisory data into `advisory_matches` for indexed crates, and also ingests native RustSec advisory-db metadata when `RUSTSEC_DB_DIR` points to a local advisory-db checkout (`crates/<crate_name>/*.md`), including withdrawn/category context and patched/unaffected ranges.
- Local source refresh: `index.refresh` with `scope=local_cache` scans `CARGO_REGISTRY_DIR/src`, incrementally upserts text source files into `source_files`, parses Rust files via `syn` to index symbols into `symbols`, and prunes stale rows for scanned crate versions.
- Docs refresh: `index.refresh` with `scope=docs` fetches docs.rs pages for indexed crate versions and stores normalized page text in `docs_pages` for `docs.search`.

## Developer commands

Using `make`:

```bash
make fmt
make check
make up
make logs
```

Using `just`:

```bash
just fmt
just check
just up
just logs
```

Benchmark harness (requires running server):

```bash
cargo run --bin benchmark_mcp -- --endpoint http://127.0.0.1:43173/mcp --crate-name serde --iterations 10
```

Concurrent load/backpressure harness:

```bash
cargo run --bin load_test_mcp -- --endpoint http://127.0.0.1:43173/mcp --crate-name serde --concurrency 8 --requests-per-worker 20
```

## Configuration

Core env vars:

- `DATABASE_URL` (default `postgres://postgres:postgres@postgres:5432/rust_mcp`)
- `MCP_HTTP_BIND` (default `0.0.0.0:43173` in container)
- `MCP_HTTP_PORT` (default `43173`, loopback-published)
- `MAX_CONCURRENT_REQUESTS` (default `128`)
- `MCP_TRANSPORT` (`http` or `stdio`; default local shared deployment uses `http`)
- `MCP_SSE_KEEP_ALIVE_SECS` (default `15`)
- `MCP_SSE_RETRY_MS` (default `3000`)
- `CRATES_IO_BASE_URL` (default `https://crates.io`)
- `CRATES_IO_USER_AGENT` (default `rust-mcp/0.1.0 (local dev machine)`)
- `CRATES_IO_TIMEOUT_SECS` (default `20`)
- `CRATES_IO_MIN_INTERVAL_MS` (default `100`)
- `DOCS_RS_BASE_URL` (default `https://docs.rs`)
- `DOCS_RS_MIN_INTERVAL_MS` (default `120`)
- `OSV_MIN_INTERVAL_MS` (default `150`)
- `CARGO_REGISTRY_DIR` (container path for mounted host cargo cache)
- `MCP_DATA_DIR` (container path for local index/cache artifacts)
- `AUTO_MIGRATE` (default `true`)
- `RUST_LOG`, `LOG_FORMAT`

## Project layout

- `docker-compose.yml`: local orchestration, loopback binding, persistent volumes.
- `Dockerfile`: multi-stage build for app container.
- `migrations/`: SQL migrations for PostgreSQL schema/indexes.
- `src/config.rs`: typed runtime configuration.
- `src/logging.rs`: tracing subscriber setup.
- `src/state.rs`: shared state + DB pool + migrations + readiness check.
- `src/http.rs`: health/readiness/MCP route wiring.
- `src/mcp/`: `rmcp` streamable HTTP server split into focused submodules (server/router, ingest/sync, search, intel, transport).
