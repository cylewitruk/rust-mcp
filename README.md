# rust-mcp

Local-first Rust dependency intelligence MCP server.

`rust-mcp` runs as a single Docker container that includes:

- the MCP server
- an embedded PostgreSQL instance
- background refresh workers
- Prometheus metrics export

`rust-mcp` is HTTP-only and serves MCP via Streamable HTTP.

## Current Status

- Active development, with 34 MCP tools currently registered.
- Health/readiness endpoints are exposed on the same HTTP listener as MCP.
- The index can ingest crates.io metadata, local cargo registry source, docs.rs pages, and optional rustdoc JSON files.

## Tool Surface

Core and indexing:

- `ping`
- `schema.get`
- `index.sync_crates`
- `index.status`
- `index.refresh`

Crate intelligence:

- `crate.search`
- `crate.intel`
- `crate.features`
- `crate.api_diff`
- `crate.api`
- `crate.type_info`
- `crate.trait_impls`
- `crate.re_exports`
- `crate.import_path`
- `crate.error_types`
- `crate.derive_macros`
- `crate.compare`
- `crate.compatibility`
- `crate.compatibility_matrix`
- `crate.migration_path`
- `crate.license_check`
- `crate.alternatives`
- `crate.versions`
- `crate.graph`
- `crate.hotspots`
- `crate.usage_patterns`

Dependency intelligence:

- `dependency.audit`
- `dependency.resolve`
- `dependency.feature_impact`

Source/symbol/docs:

- `source.search`
- `source.read`
- `source.context`
- `symbol.search`
- `docs.search`

## Quick Start

1. Copy environment defaults:

```bash
cp .env.example .env
```

1. Start the container:

```bash
docker compose up --build rust-mcp
```

1. Check health:

```bash
curl -sS http://127.0.0.1:43173/healthz
curl -sS http://127.0.0.1:43173/readyz
```

## MCP Protocol Quick Check

The expected client sequence is:

1. `initialize`
1. `notifications/initialized`
1. `tools/list` / `tools/call`

Example (bash + curl):

```bash
MCP_URL="http://127.0.0.1:43173/mcp"
TMP_HEADERS="$(mktemp)"

# 1) initialize
curl -sS -D "$TMP_HEADERS" \
  -H "content-type: application/json" \
  -H "accept: application/json, text/event-stream" \
  -X POST "$MCP_URL" \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"manual-client","version":"0.1.0"}}}'

SESSION_ID="$(awk -F': ' 'tolower($1)=="mcp-session-id"{gsub("\r","",$2); print $2}' "$TMP_HEADERS")"

# 2) notifications/initialized
curl -sS \
  -H "content-type: application/json" \
  -H "accept: application/json, text/event-stream" \
  -H "mcp-session-id: ${SESSION_ID}" \
  -X POST "$MCP_URL" \
  -d '{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}'

# 3) call ping
curl -sS \
  -H "content-type: application/json" \
  -H "accept: application/json, text/event-stream" \
  -H "mcp-session-id: ${SESSION_ID}" \
  -X POST "$MCP_URL" \
  -d '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"ping","arguments":{"message":"hello"}}}'
```

## Indexing Notes

- `index.sync_crates` bootstraps crates.io metadata into local tables.
- `index.refresh` supports scope-specific refreshes (`crate`, `all`, `security`, `docs`, `local_cache`, `rustdoc_json`).
- `crate.search` and `crate.intel` can trigger freshness checks and enqueue deeper refresh work.

## MCP Protocol Version Policy

- Latest published MCP protocol version: `2025-11-25`.
- Server-supported (negotiated) MCP protocol version: `2025-03-26`.
- Recommended client initialize request: send `protocolVersion: "2025-11-25"` and honor negotiated `result.protocolVersion`.
- Requested protocol versions are treated as compatibility negotiation, not a guarantee that the requested version is accepted as-is.

## Observability

- Metrics exporter: `http://127.0.0.1:9090/metrics` (separate listener).
- HTTP health endpoints:
  - `GET /healthz`
  - `GET /readyz`
  - `GET /schemas` (all tool request/response JSON Schemas)
  - `GET /schemas/{tool_name}` (single tool request/response JSON Schema)
- Tool invocation metrics are recorded for count/latency and exported to Prometheus.

## Configuration

Important environment variables:

- `MCP_HTTP_BIND` (default `127.0.0.1:43173`)
- `MCP_SSE_KEEP_ALIVE_SECS` (default `15`)
- `MCP_SSE_RETRY_MS` (default `3000`)
- `MCP_STRICT_ACCEPT` (default `false`; when `true`, reject POST `/mcp` requests that do not accept both `application/json` and `text/event-stream`)
- `DATABASE_URL`
- `PROMETHEUS_BIND` (default `0.0.0.0:9090` in container)
- `CRATES_IO_BASE_URL`, `DOCS_RS_BASE_URL`
- `CRATES_IO_MIN_INTERVAL_MS`, `DOCS_RS_MIN_INTERVAL_MS`, `OSV_MIN_INTERVAL_MS`
- `MAX_CONCURRENT_REQUESTS`
- `CARGO_REGISTRY_DIR`
- `RUSTSEC_DB_DIR` (optional local advisory-db checkout)
- `RUSTDOC_JSON_DIR` (optional pre-generated rustdoc JSON files)
- `SCHEMA_EXPORT_DIR` (optional startup export of tool schema artifacts)

## Transport

- Supported in this binary: Streamable HTTP at `/mcp`
- Not implemented in this binary: stdio transport
- If a client only supports stdio process launch, run a separate stdio-to-HTTP proxy binary against this server instance.
- In non-strict mode, JSON-only Accept headers are rewritten for compatibility and include an RFC `Warning` response header.

## Development

Useful local commands:

```bash
just fmt
just lint
just test
just e2e-test
just up
just down
```

## Known Limitations

- Rustdoc indexing is currently based on local files from `RUSTDOC_JSON_DIR` when configured (container-side rustdoc generation is roadmap work).

## Roadmap

Future work is tracked in `docs/ROADMAP.md`.
