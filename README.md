# rust-mcp

Local-first Rust dependency intelligence [MCP](https://modelcontextprotocol.io/) server for agentic coding assistants.

`rust-mcp` gives LLM agents deep knowledge of the Rust crate ecosystem: API surfaces, version diffs, dependency graphs, security advisories, source code, and more -- all served from a local index backed by embedded PostgreSQL.

## Highlights

- **35 MCP tools** covering crate intelligence, dependency auditing, source/symbol search, and docs
- **Runs as a single Docker container** with embedded PostgreSQL -- no external database needed
- **Automatic indexing** from your local `~/.cargo/registry`; new `cargo` downloads are detected and indexed in near real-time
- **On-demand indexing** of any published crate via `index_crates`
- **Security advisories** from OSV cross-referenced in `crate_intel` and `dependency_audit`
- **Streamable HTTP transport** at `/mcp` with an optional stdio adapter for clients that need it
- **385+ tests** including unit, integration/e2e & live tests

## Quick Start

1. Copy environment defaults:

```bash
cp .env.example .env
```

1. Start the container:

```bash
docker compose up --build rust-mcp
```

1. Verify it's running:

```bash
curl -sS http://127.0.0.1:43173/healthz
# {"status":"ok"}

curl -sS http://127.0.0.1:43173/readyz
# {"status":"ready"}
```

The container mounts `~/.cargo/registry` (read-only) and uses two Docker volumes for database and server state.

## Client Configuration

### Direct HTTP (preferred)

If your client supports remote MCP servers, point it at the HTTP endpoint:

```json
{
  "mcpServers": {
    "rust-mcp": {
      "url": "http://127.0.0.1:43173/mcp"
    }
  }
}
```

### stdio adapter

For clients that only support process-launched MCP servers, use the `rust-mcp-stdio` adapter:

```json
{
  "mcpServers": {
    "rust-mcp": {
      "command": "cargo",
      "args": [
        "run", "-p", "rust-mcp-stdio",
        "--", "--mcp-url", "http://127.0.0.1:43173/mcp"
      ]
    }
  }
}
```

The adapter auto-bootstraps the upstream MCP session and passes all tools through transparently.

For client-specific setup (VS Code Copilot Chat, Claude Code, Cursor, Gemini CLI, Codex, etc.), see [docs/CLIENT_CONFIGURATION.md](docs/CLIENT_CONFIGURATION.md).

## Tools

<details>
<summary><strong>Core & indexing</strong> (5 tools)</summary>

| Tool | Description |
| ---- | ---- |
| `ping` | Check MCP connectivity and DB readiness |
| `schema_get` | Return request/response JSON Schemas for one or all MCP tools |
| `index_crates` | Fetch and index crates from crates.io (call when a crate is not yet indexed) |
| `index_status` | Return index freshness, coverage, and queue state |
| `index_refresh` | Trigger index refresh for a scope and return job status |

</details>

<details>
<summary><strong>Crate intelligence</strong> (22 tools)</summary>

| Tool | Description |
| ---- | ---- |
| `crate_search` | Search indexed crates by name, category, keyword, or description |
| `crate_intel` | Dense intelligence: versions, deps, dependents, advisories (start here) |
| `crate_features` | Feature flags, defaults, and transitive enables for a crate version |
| `crate_api` | Public API symbols with optional kind/path filters |
| `crate_api_diff` | Compare public API symbols between two versions: added, removed, changed |
| `crate_type_info` | Type definition metadata and impl details |
| `crate_trait_impls` | Trait/type implementation relationships with optional filters |
| `crate_re_exports` | Public re-export mappings to canonical import paths |
| `crate_import_path` | Resolve public import paths for a symbol |
| `crate_error_types` | Error-type metadata, conversion impls, and functions returning each error |
| `crate_deprecated` | Deprecated symbols with notes and suggested replacements |
| `crate_derive_macros` | Proc-macro exports (derive, attribute, function-like) |
| `crate_compare` | Compare two crates on adoption, risk, and maintenance signals |
| `crate_compatibility` | Pairwise dependency compatibility check between two crates |
| `crate_compatibility_matrix` | Compatibility across multiple version pairs between two crates |
| `crate_migration_path` | Migration actions for a crate upgrade from API diff breaking changes |
| `crate_license_check` | License metadata with optional allow/deny policy evaluation |
| `crate_alternatives` | Ranked alternative crates by taxonomy overlap and adoption signals |
| `crate_versions` | Version timeline with yanked/security/adoption markers |
| `crate_graph` | Depth-bounded dependency/dependent graph |
| `crate_hotspots` | Unsafe and concurrency hotspots in crate source |
| `crate_usage_patterns` | Source snippets from dependent crates that use a target symbol |

</details>

<details>
<summary><strong>Dependency intelligence</strong> (3 tools)</summary>

| Tool | Description |
| ---- | ---- |
| `dependency_audit` | Audit a Cargo.toml for yanked versions, advisories, outdated deps, MSRV conflicts |
| `dependency_resolve` | Simulate dependency resolution and report resolvable versions or conflicts |
| `dependency_feature_impact` | Estimate additional dependency surface from selected feature flags |

</details>

<details>
<summary><strong>Source, symbol & docs</strong> (5 tools)</summary>

| Tool | Description |
| ---- | ---- |
| `source_search` | Search indexed source files by text/regex with optional filters |
| `source_read` | Read a line range from an indexed crate source file |
| `source_context` | Semantic context around a source location: module path, imports, nearby types |
| `symbol_search` | Search indexed symbols by name with optional crate/version/kind filters |
| `docs_search` | Search indexed docs.rs pages by query with optional filters |

</details>

## HTTP Endpoints

All endpoints are served on a single HTTP listener (default `127.0.0.1:43173`):

| Path | Method | Description |
| ---- | ---- | ---- |
| `/mcp` | POST | MCP Streamable HTTP transport |
| `/healthz` | GET | Liveness check |
| `/readyz` | GET | Readiness check (includes DB connectivity) |
| `/schemas` | GET | All tool request/response JSON Schemas |
| `/schemas/{tool_name}` | GET | Single tool JSON Schema |
| `/metrics` | GET | Prometheus metrics |
| `/.well-known/mcp.json` | GET | MCP server discovery card |

## Configuration

All configuration is via environment variables (also settable as CLI flags). See [.env.example](.env.example) for annotated defaults.

<details>
<summary><strong>Database</strong></summary>

| Variable | Default | Description |
| ---- | ---- | ---- |
| `DATABASE_URL` | `postgres://postgres@%2Frun%2Fpostgresql/rust_mcp` | PostgreSQL connection string |
| `DATABASE_MIN_CONNECTIONS` | `1` | Minimum connection pool size |
| `DATABASE_MAX_CONNECTIONS` | `10` | Maximum connection pool size |
| `AUTO_MIGRATE` | `true` | Run SQL migrations on startup |

</details>

<details>
<summary><strong>HTTP / MCP transport</strong></summary>

| Variable | Default | Description |
| ---- | ---- | ---- |
| `MCP_HTTP_BIND` | `127.0.0.1:43173` | HTTP bind address |
| `MCP_SSE_KEEP_ALIVE_SECS` | `15` | SSE keep-alive interval (seconds) |
| `MCP_SSE_RETRY_MS` | `3000` | SSE retry delay for reconnecting clients (ms) |
| `MCP_STRICT_ACCEPT` | `false` | When `true`, reject requests that don't accept both `application/json` and `text/event-stream` |
| `MAX_CONCURRENT_REQUESTS` | `128` | Maximum concurrent inbound HTTP requests |

</details>

<details>
<summary><strong>Outbound rate limiting</strong></summary>

Each external service has per-request burst gating (`_MIN_INTERVAL_MS`) and a rolling-window rate limit (`_WINDOW_MAX_REQUESTS` / `_WINDOW_DURATION_SECS`). Set `_WINDOW_MAX_REQUESTS=0` to disable window limiting for a service.

**crates.io:**

| Variable | Default | Description |
| ---- | ---- | ---- |
| `CRATES_IO_MIN_INTERVAL_MS` | `1000` | Minimum delay between requests (ms) |
| `CRATES_IO_WINDOW_MAX_REQUESTS` | `60` | Max requests in the rolling window (0 = disabled) |
| `CRATES_IO_WINDOW_DURATION_SECS` | `120` | Rolling window duration (seconds) |

**docs.rs:**

| Variable | Default | Description |
| ---- | ---- | ---- |
| `DOCS_RS_MIN_INTERVAL_MS` | `1000` | Minimum delay between requests (ms) |
| `DOCS_RS_WINDOW_MAX_REQUESTS` | `60` | Max requests in the rolling window (0 = disabled) |
| `DOCS_RS_WINDOW_DURATION_SECS` | `120` | Rolling window duration (seconds) |

**OSV:**

| Variable | Default | Description |
| ---- | ---- | ---- |
| `OSV_MIN_INTERVAL_MS` | `1000` | Minimum delay between requests (ms) |
| `OSV_WINDOW_MAX_REQUESTS` | `60` | Max requests in the rolling window (0 = disabled) |
| `OSV_WINDOW_DURATION_SECS` | `120` | Rolling window duration (seconds) |

**GitHub API:**

| Variable | Default | Description |
| ---- | ---- | ---- |
| `GITHUB_MIN_INTERVAL_MS` | `5000` | Minimum delay between requests (ms) |
| `GITHUB_WINDOW_MAX_REQUESTS` | `59` | Max requests in the rolling window (0 = disabled) |
| `GITHUB_WINDOW_DURATION_SECS` | `3600` | Rolling window duration (seconds) |

</details>

<details>
<summary><strong>External integrations</strong></summary>

| Variable | Default | Description |
| ---- | ---- | ---- |
| `CRATES_IO_BASE_URL` | `https://crates.io` | crates.io API base URL |
| `CRATES_IO_TIMEOUT_SECS` | `20` | HTTP timeout for crates.io/OSV requests (seconds) |
| `DOCS_RS_BASE_URL` | `https://docs.rs` | docs.rs base URL |
| `OSV_BASE_URL` | `https://api.osv.dev` | OSV API base URL |
| `GITHUB_BASE_URL` | `https://api.github.com` | GitHub API base URL |

</details>

<details>
<summary><strong>Git probe (commit liveness)</strong></summary>

| Variable | Default | Description |
| ---- | ---- | ---- |
| `GIT_PROBE_ENABLED` | `true` | Enable git-based repo probing for commit liveness data |
| `GIT_PROBE_CLONE_DEPTH` | `500` | Shallow clone depth for history extraction (0 = disabled) |
| `GIT_PROBE_TIMEOUT_SECS` | `60` | Timeout per clone + extraction (seconds) |

</details>

<details>
<summary><strong>Paths</strong></summary>

| Variable | Default | Description |
| ---- | ---- | ---- |
| `CARGO_REGISTRY_DIR` | `/cargo/registry` | Mounted cargo registry directory |
| `MCP_DATA_DIR` | `/var/lib/rust-mcp` | Server local data directory |
| `CRATE_SOURCE_CACHE_DIR` | `/var/lib/rust-mcp/crate-sources` | Cache for on-demand crate source downloads |
| `RUSTSEC_DB_DIR` | _(unset)_ | Optional local advisory-db checkout |
| `RUSTDOC_JSON_DIR` | _(unset)_ | Optional pre-generated rustdoc JSON files |
| `SCHEMA_EXPORT_DIR` | _(unset)_ | Optional startup export of tool schema artifacts |

</details>

<details>
<summary><strong>Registry discovery & pre-warming</strong></summary>

| Variable | Default | Description |
| ---- | ---- | ---- |
| `REGISTRY_SCAN_INTERVAL_SECS` | `600` | Seconds between registry discovery scans (0 = disabled) |
| `REGISTRY_SCAN_BATCH_LIMIT` | `0` | Max new crate jobs per scan (0 = unlimited) |
| `PRE_WARM_CRATES` | _(empty)_ | Comma-separated crate names to index first at startup |
| `REGISTRY_CACHE_WATCH_INTERVAL_MS` | `1000` | Polling interval for new `.crate` files (ms, 0 = disabled) |

</details>

<details>
<summary><strong>Enrichment, security & sessions</strong></summary>

| Variable | Default | Description |
| ---- | ---- | ---- |
| `ENRICHMENT_MAINTENANCE_INTERVAL_SECS` | `300` | Seconds between enrichment maintenance scans (0 = disabled after startup) |
| `RUSTDOC_RETRY_COOLDOWN_SECS` | `86400` | Minimum seconds before retrying a failed rustdoc enrichment |
| `SECURITY_SYNC_INTERVAL_SECS` | `86400` | Seconds between security advisory syncs (0 = startup only) |
| `SECURITY_SYNC_BATCH_SIZE` | `50` | Max crates to check per security sync pass |
| `SESSION_IDLE_TIMEOUT_SECS` | `259200` | Idle timeout for MCP sessions before pruning (0 = no pruning) |

</details>

<details>
<summary><strong>Logging</strong></summary>

| Variable | Default | Description |
| ---- | ---- | ---- |
| `RUST_LOG` | `info,rust_mcp=debug,sqlx=warn` | Tracing filter ([RUST_LOG style](https://docs.rs/tracing-subscriber)) |
| `LOG_FORMAT` | `pretty` | Log output format (`pretty` or `json`) |

</details>

<details>
<summary><strong>Docker / entrypoint only</strong> (not read by the server binary)</summary>

| Variable | Default | Description |
| ---- | ---- | ---- |
| `MCP_HTTP_PORT` | `43173` | Host-side port mapping in docker-compose |
| `OUTBOUND_FIREWALL` | `true` | Enable iptables-based outbound domain allowlist |
| `OUTBOUND_ALLOWLIST` | `crates.io,static.crates.io,docs.rs,api.osv.dev,api.github.com,github.com` | Comma-separated allowed outbound domains |

</details>

<details>
<summary><strong><code>rust-mcp-stdio</code> adapter</strong></summary>

| Variable | Default | Description |
| ---- | ---- | ---- |
| `MCP_URL` | `http://127.0.0.1:43173/mcp` | Upstream MCP endpoint URL |
| `MCP_CONNECT_TIMEOUT_SECS` | `10` | HTTP connect timeout (seconds) |
| `MCP_REQUEST_TIMEOUT_SECS` | `120` | HTTP request timeout (seconds) |
| `MCP_PREFLIGHT_SCHEMA` | `true` | Validate upstream tool contracts from `/schemas` before proxying |
| `MCP_AUTO_BOOTSTRAP_SESSION` | `true` | Auto-run upstream `initialize` + `notifications/initialized` when no session exists |

</details>

## MCP Protocol

<details>
<summary><strong>Protocol version and session flow</strong></summary>

- Server MCP protocol version: `2025-06-18`
- Session flow: `initialize` -> `notifications/initialized` -> `tools/list` / `tools/call`
- Propagate `mcp-session-id` header on all requests after `initialize`
- Responses may be JSON (`application/json`) or SSE (`text/event-stream`)
- In non-strict mode, JSON-only Accept headers are rewritten for compatibility (with an RFC `Warning` header)

</details>

<details>
<summary><strong>Manual protocol check (curl)</strong></summary>

```bash
MCP_URL="http://127.0.0.1:43173/mcp"
TMP_HEADERS="$(mktemp)"

# 1) initialize
curl -sS -D "$TMP_HEADERS" \
  -H "content-type: application/json" \
  -H "accept: application/json, text/event-stream" \
  -X POST "$MCP_URL" \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"manual-client","version":"0.1.0"}}}'

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

</details>

<details>
<summary><strong>Search pagination contract</strong></summary>

Search-style responses expose consistent pagination metadata:

- `page`, `limit`, `count`, `has_more`, `truncated`, `cursor`, `next_cursor`
- `cursor` is an opaque token; when provided, keep filters unchanged and reuse the same page size
- `next_cursor` is populated only when additional results are available
- `truncated=true` indicates the current page is incomplete and can be continued with `next_cursor`

Tools supporting pagination: `symbol_search`, `crate_search`, `source_search`, `docs_search`, `crate_versions`, `crate_alternatives`, `crate_hotspots`, `crate_usage_patterns`, `crate_api`, `crate_re_exports`, `crate_import_path`, `crate_error_types`, `crate_trait_impls`.

</details>

## Development

```bash
just build       # build (release)
just run         # run server locally (outside Docker)
just fmt         # format code
just lint        # clippy + check + fmt check
just fix         # auto-fix clippy lints + format
just test        # integration tests with coverage
just test-e2e    # Docker-based end-to-end tests
just test-live   # live tests against real upstreams (opt-in)
just up          # docker compose up (build + detach)
just down        # docker compose down
just logs        # follow container logs
```

Targeted test runs:

```bash
cargo --locked nextest run -p rust-mcp --features integration-tests --test integration <filter>
cargo --locked nextest run -p rust-mcp --features e2e-tests --test e2e_http <filter>
```

## Known Limitations

- Rustdoc indexing is based on docs.rs-fetched JSON and optional local files from `RUSTDOC_JSON_DIR`; container-side rustdoc generation is roadmap work.

## Roadmap

Future work is tracked in [docs/ROADMAP.md](docs/ROADMAP.md).
