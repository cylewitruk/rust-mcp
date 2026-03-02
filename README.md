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
- `schema_get`
- `index_sync_crates`
- `index_status`
- `index_refresh`

Crate intelligence:

- `crate_search`
- `crate_intel`
- `crate_features`
- `crate_api_diff`
- `crate_api`
- `crate_type_info`
- `crate_trait_impls`
- `crate_re_exports`
- `crate_import_path`
- `crate_error_types`
- `crate_derive_macros`
- `crate_compare`
- `crate_compatibility`
- `crate_compatibility_matrix`
- `crate_migration_path`
- `crate_license_check`
- `crate_alternatives`
- `crate_versions`
- `crate_graph`
- `crate_hotspots`
- `crate_usage_patterns`

Dependency intelligence:

- `dependency_audit`
- `dependency_resolve`
- `dependency_feature_impact`

Source/symbol/docs:

- `source_search`
- `source_read`
- `source_context`
- `symbol_search`
- `docs_search`

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

- `index_sync_crates` bootstraps crates.io metadata into local tables.
- `index_refresh` supports scope-specific refreshes (`crate`, `all`, `security`, `docs`, `local_cache`, `rustdoc_json`).
- `crate_search` and `crate_intel` can trigger freshness checks and enqueue deeper refresh work.

## Rustdoc Freshness and Confidence

- Rustdoc-backed crate tools continue to report freshness checks against crates.io and local index provenance in `freshness`, `freshness_check_result`, and `refresh_enqueued` fields.
- Rustdoc-backed path/shape resolution tools (`crate_re_exports`, `crate_import_path`) return high confidence when canonical rustdoc symbol metadata is available, and degrade to medium/low confidence when falling back to sparse local metadata.
- Rustdoc-backed type/impl tools (`crate_type_info`, `crate_trait_impls`) prefer richer rustdoc-derived rows when duplicate syn/local and rustdoc rows coexist, and confidence reflects whether definitions/impl metadata were actually resolved.
- API diffing (`crate_api_diff`) now prioritizes rustdoc-derived public symbols when dual-source duplicates exist, reducing false negatives caused by sparse duplicate snapshots.

## Search Pagination Contract

- Search-style responses expose consistent pagination metadata: `page`, `limit`, `count`, `has_more`, `truncated`, `cursor`, and `next_cursor`.
- `cursor` is an opaque token; when provided, clients should keep filters unchanged and either omit `limit` or reuse the same page size.
- `next_cursor` is only populated when additional results are available.
- `truncated=true` indicates the current page is incomplete relative to available matches and can be continued with `next_cursor`.
- This contract is now implemented for `symbol_search`, `crate_search`, `source_search`, `docs_search`, `crate_versions`, `crate_alternatives`, `crate_hotspots`, `crate_usage_patterns`, `crate_api`, `crate_re_exports`, `crate_import_path`, `crate_error_types`, and `crate_trait_impls`.

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

All server environment variables with their defaults:

**Database:**

| Variable | Default | Description |
|---|---|---|
| `DATABASE_URL` | `postgres://postgres@%2Frun%2Fpostgresql/rust_mcp` | PostgreSQL connection string |
| `DATABASE_MIN_CONNECTIONS` | `1` | Minimum database connection pool size |
| `DATABASE_MAX_CONNECTIONS` | `10` | Maximum database connection pool size |
| `AUTO_MIGRATE` | `true` | Run SQL migrations on startup |

**HTTP / MCP transport:**

| Variable | Default | Description |
|---|---|---|
| `MCP_HTTP_BIND` | `127.0.0.1:43173` | HTTP bind address |
| `MCP_SSE_KEEP_ALIVE_SECS` | `15` | SSE keep-alive interval (seconds) |
| `MCP_SSE_RETRY_MS` | `3000` | SSE retry delay for reconnecting clients (ms) |
| `MCP_STRICT_ACCEPT` | `false` | When `true`, reject POST `/mcp` requests that do not accept both `application/json` and `text/event-stream` |
| `MAX_CONCURRENT_REQUESTS` | `128` | Maximum concurrent inbound HTTP requests |

**Outbound rate limiting:**

| Variable | Default | Description |
|---|---|---|
| `CRATES_IO_MIN_INTERVAL_MS` | `1000` | Minimum delay between crates.io requests (ms) |
| `DOCS_RS_MIN_INTERVAL_MS` | `500` | Minimum delay between docs.rs requests (ms) |
| `OSV_MIN_INTERVAL_MS` | `250` | Minimum delay between OSV requests (ms) |

**External integrations:**

| Variable | Default | Description |
|---|---|---|
| `CRATES_IO_BASE_URL` | `https://crates.io` | crates.io API base URL |
| `CRATES_IO_USER_AGENT` | `rust-mcp/0.1.0 (local dev machine)` | User-Agent for outbound crates.io requests |
| `CRATES_IO_TIMEOUT_SECS` | `20` | HTTP timeout for crates.io/OSV requests (seconds) |
| `DOCS_RS_BASE_URL` | `https://docs.rs` | docs.rs base URL |

**Paths:**

| Variable | Default | Description |
|---|---|---|
| `CARGO_REGISTRY_DIR` | `/cargo/registry` | Mounted cargo registry directory |
| `MCP_DATA_DIR` | `/var/lib/rust-mcp` | Server local data directory |
| `RUSTSEC_DB_DIR` | _(unset)_ | Optional local advisory-db checkout |
| `RUSTDOC_JSON_DIR` | _(unset)_ | Optional pre-generated rustdoc JSON files |
| `SCHEMA_EXPORT_DIR` | _(unset)_ | Optional startup export of tool schema artifacts |

**Registry discovery & pre-warming:**

| Variable | Default | Description |
|---|---|---|
| `REGISTRY_SCAN_INTERVAL_SECS` | `600` | Seconds between periodic registry discovery scans (0 = disabled) |
| `REGISTRY_SCAN_BATCH_LIMIT` | `0` | Max new crate jobs per discovery scan (0 = unlimited) |
| `PRE_WARM_CRATES` | _(empty)_ | Comma-separated crate names to index first at startup |

**Logging:**

| Variable | Default | Description |
|---|---|---|
| `RUST_LOG` | `info,rust_mcp=debug,sqlx=warn` | Tracing filter (RUST_LOG style) |
| `LOG_FORMAT` | `pretty` | Log output format (`pretty` or `json`) |

**Prometheus:**

| Variable | Default | Description |
|---|---|---|
| `PROMETHEUS_BIND` | `0.0.0.0:9090` | Prometheus metrics exporter bind address |

**Docker / entrypoint only** (not read by the server binary):

| Variable | Default | Description |
|---|---|---|
| `MCP_HTTP_PORT` | `43173` | Host-side port mapping in docker-compose |
| `PROMETHEUS_PORT` | `9090` | Host-side Prometheus port mapping in docker-compose |
| `OUTBOUND_FIREWALL` | `true` | Enable tinyproxy-based domain allowlist |
| `OUTBOUND_ALLOWLIST` | `crates.io,static.crates.io,docs.rs,api.osv.dev,api.github.com` | Comma-separated allowed outbound domains |

`rust-mcp-stdio` adapter variables:

- `MCP_URL` (default `http://127.0.0.1:43173/mcp`)
- `MCP_CONNECT_TIMEOUT_SECS` (default `10`)
- `MCP_REQUEST_TIMEOUT_SECS` (default `120`)
- `MCP_AUTO_BOOTSTRAP_SESSION` (default `true`; auto-runs upstream `initialize` + `notifications/initialized` for session-required requests when no session exists)

## Transport

- Supported in this binary: Streamable HTTP at `/mcp`
- Not implemented in this binary: stdio transport (handled by separate adapter)
- `rust-mcp-stdio` is a dedicated stdio-to-HTTP MCP adapter binary that proxies MCP JSON-RPC messages to a configured `MCP_URL`.
- Tool support in `rust-mcp-stdio` is pass-through, so all tools exposed by the target `rust-mcp` HTTP instance are available without per-tool adapter code.
- Example stdio adapter launch:

```bash
cargo run -p rust-mcp-stdio -- --mcp-url http://127.0.0.1:43173/mcp
```

- In non-strict mode, JSON-only Accept headers are rewritten for compatibility and include an RFC `Warning` response header.

## Client Configuration Examples

Most MCP clients support one of these two patterns:

### Direct HTTP(S) MCP server

Use this when your client supports remote MCP endpoints directly.

```json
{
  "mcpServers": {
    "rust-mcp-http": {
      "url": "http://127.0.0.1:43173/mcp"
    }
  }
}
```

For TLS deployments, switch to HTTPS:

```json
{
  "mcpServers": {
    "rust-mcp-https": {
      "url": "https://mcp.example.com/mcp"
    }
  }
}
```

### `stdio` adapter (`rust-mcp-stdio`)

Use this when your client only supports launching MCP servers over stdio.

```json
{
  "mcpServers": {
    "rust-mcp-stdio": {
      "command": "cargo",
      "args": [
        "run",
        "-p",
        "rust-mcp-stdio",
        "--",
        "--mcp-url",
        "http://127.0.0.1:43173/mcp"
      ],
      "env": {
        "MCP_PREFLIGHT_SCHEMA": "true"
      }
    }
  }
}
```

HTTPS works the same with stdio by changing `--mcp-url`:

```json
{
  "mcpServers": {
    "rust-mcp-stdio": {
      "command": "cargo",
      "args": [
        "run",
        "-p",
        "rust-mcp-stdio",
        "--",
        "--mcp-url",
        "https://mcp.example.com/mcp"
      ]
    }
  }
}
```

### Popular client setup

For popular clients (Copilot Chat in VS Code, Claude Code CLI/VS Code, Codex CLI/VS Code, Gemini CLI, Cursor), see:

- [docs/CLIENT_CONFIGURATION.md](docs/CLIENT_CONFIGURATION.md)

That guide keeps client-specific setup out of this README and covers both direct HTTP(S) MCP and `stdio` adapter variants.

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

Opt-in live integration tests (real upstreams, no mock crates.io/docs.rs):

```bash
RUST_MCP_RUN_LIVE_E2E=1 \
cargo --locked nextest run -p rust-mcp --features integration-tests --test integration live_unmocked
```

To include local cargo registry indexing coverage, also set:

```bash
export RUST_MCP_LIVE_CARGO_REGISTRY_DIR="$HOME/.cargo/registry/src"
```

## Known Limitations

- Rustdoc indexing is currently based on local files from `RUSTDOC_JSON_DIR` when configured (container-side rustdoc generation is roadmap work).

## Roadmap

Future work is tracked in `docs/ROADMAP.md`.
