# rust-mcp

Local-first Rust dependency intelligence MCP server scaffold.

This scaffold gives you:

- Loopback-only HTTP exposure for local agent clients.
- PostgreSQL with persistent Docker volume.
- Initial schema/migration for crate/version/source/symbol/docs indexing.
- Rust server skeleton with structured config, logging, health/readiness endpoints, and graceful shutdown.
- `rmcp` streamable HTTP transport mounted at `/mcp` with `ping`, `index.sync_crates`, `index.status`, `index.refresh`, `crate.search`, `crate.intel`, `crate.versions`, `crate.graph`, `source.search`, `source.read`, `symbol.search`, and `docs.search` tools.

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
- `crate.versions`: returns normalized version timeline with yanked/security/adoption markers and interaction freshness metadata.
- `crate.graph`: returns depth-bounded dependency/dependent graph nodes and edges for `dependencies`, `dependents`, or `both` directions.
- `source.search`: searches indexed source files by `text` or `regex` mode, with optional `crate_name`, `version`, and `path_glob` filters.
- `source.read`: returns a line-bounded slice of an indexed source file by `crate_name` + `path` (optionally pinning `version`).
- `symbol.search`: searches indexed symbols by symbol name with optional `crate_name`, `version`, and `kind` filters; supports opaque cursor pagination (`cursor`/`next_cursor`) with `page`+`limit` fallback, `include_all_versions` (default `false`, latest-version only), and `collapse_by_canonical` (default `false`) to deduplicate by canonical symbol identity across versions.
- `docs.search`: searches indexed docs.rs pages with optional `crate_name`, `version`, and `path_prefix` filters.

Quality contract fields now included in `crate.search`, `crate.intel`, `crate.versions`, and `crate.graph` responses:

- `confidence` (`high|medium|low`)
- `next_best_calls` (ordered suggested follow-up tools)

## Tool examples

`crate.versions` request (MCP `tools/call` payload `arguments`):

```json
{
  "crate_name": "serde",
  "limit": 20
}
```

`crate.versions` response shape (abbreviated):

```json
{
  "crate_name": "serde",
  "count": 20,
  "versions": [
    {
      "version": "1.0.228",
      "published_at": "2026-01-12T10:20:30+00:00",
      "yanked": false,
      "downloads": 12345678,
      "advisory_count": 0,
      "release_age_days": 32,
      "is_latest": true,
      "adoption_signal": "high",
      "markers": ["latest"]
    }
  ],
  "freshness_check_performed": true,
  "freshness_check_result": "unchanged",
  "refresh_enqueued": false,
  "refresh_job_id": null,
  "provenance": "local_postgres_index"
}
```

`crate.graph` request (MCP `tools/call` payload `arguments`):

```json
{
  "crate_name": "serde",
  "direction": "dependencies",
  "depth": 2
}
```

`index.refresh` local cache request (MCP `tools/call` payload `arguments`):

```json
{
  "scope": "local_cache",
  "crate_name": "serde",
  "query": "src/",
  "page": 1,
  "per_page": 25
}
```

`docs.search` request (MCP `tools/call` payload `arguments`):

```json
{
  "query": "serialize",
  "crate_name": "serde",
  "limit": 10
}
```

`symbol.search` request (MCP `tools/call` payload `arguments`):

```json
{
  "query": "Serializer",
  "crate_name": "serde",
  "include_all_versions": false,
  "collapse_by_canonical": true,
  "cursor": "<next_cursor_from_previous_response>",
  "page": 1,
  "limit": 25
}
```

`symbol.search` response shape (abbreviated):

```json
{
  "query": "Serializer",
  "crate_name": "serde",
  "include_all_versions": false,
  "collapse_by_canonical": true,
  "cursor": null,
  "next_cursor": "eyJ2IjoxLCJvZmZzZXQiOjI1LCJsaW1pdCI6MjUsInF1ZXJ5IjoiU2VyaWFsaXplciIsImNyYXRlX25hbWUiOiJzZXJkZSIsInZlcnNpb24iOm51bGwsImtpbmQiOm51bGwsImluY2x1ZGVfYWxsX3ZlcnNpb25zIjpmYWxzZX0",
  "page": 1,
  "limit": 25,
  "total_count": 73,
  "has_more": true,
  "count": 25,
  "hits": [
    {
      "crate_name": "serde",
      "version": "1.0.228",
      "source_path": "src/ser/mod.rs",
      "name": "Serializer",
      "kind": "trait",
      "start_line": 116,
      "end_line": 116
    }
  ]
}
```

`symbol.search` cursor contract:

- `cursor` must be treated as opaque; clients should only pass through `next_cursor` values from prior responses.
- If `cursor` is provided, filter fields (`query`, `crate_name`, `version`, `kind`, `include_all_versions`, `collapse_by_canonical`) must match the original request.
- If `cursor` is provided and `limit` is also provided, it must match the original page size.
- Invalid, stale, or mismatched cursor tokens return a request error.

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
- `MCP_TRANSPORT` (`http` or `stdio`, current scaffold focuses on `http`)
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
