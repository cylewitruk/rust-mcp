# AGENTS.md

Local-first Rust dependency intelligence MCP server. Single Docker container with embedded PostgreSQL.

## Project layout

```text
src/
  app.rs             # Startup lifecycle: config -> DB -> migrations -> workers -> serve
  config.rs          # All config via clap (CLI flags + env vars)
  state.rs           # AppState: PgPool + reqwest::Client + Config
  error.rs           # ApiError enum with axum IntoResponse
  http.rs            # GET /healthz, GET /readyz
  mcp/
    server.rs        # Tool registration via rmcp #[tool_router] macro
    models.rs        # All request/response/DB row types
    utils.rs         # Input validation: normalize, clamp, glob-to-like
    index.rs         # index.status, index.refresh, refresh worker, freshness probes, adaptive TTL
    search.rs        # crate.search
    intel.rs         # crate.intel
    versions.rs      # crate.versions
    graph.rs         # crate.graph
    source.rs        # source.search, source.read
    symbol.rs        # symbol.search
    docs_intel.rs    # docs.search
    security.rs      # RustSec/OSV advisory ingestion
    local_cache.rs   # Cargo registry scanner + syn-based symbol extraction
    query_cache.rs   # query_cache table ops
    metrics.rs       # tool_invocations recording
migrations/          # SQLx Postgres migrations
```

## Tool implementation pattern

Each tool follows: **validate -> query -> transform -> envelope**.

- Types go in `models.rs` — requests derive `Deserialize, schemars::JsonSchema`; responses derive `Serialize, schemars::JsonSchema`; DB rows derive `FromRow`
- Handler goes in its own `mcp/<name>.rs` as `pub(super) async fn handle_<name>(&self, request) -> Result<Json<Response>, String>`
- Register in `server.rs` with `#[tool(name = "namespace.action", description = "...")]`, wrapped in `self.instrument_tool()`
- Validate inputs with `utils.rs` helpers before any DB query
- Build SQL with `QueryBuilder::<Postgres>` and `.push_bind()` — never interpolate user input
- Every response includes: `confidence` ("high"/"low"), `next_best_calls` (`Vec<String>`), `provenance` (source string)
- Crate-facing tools also include `freshness_check_performed/result`, `refresh_enqueued`, `refresh_job_id`

## Key conventions

- Error handling: tool errors are `Err(String)`, HTTP errors use `ApiError`, startup uses `anyhow`
- DB rows use `pub(super)` visibility and `#[derive(FromRow)]`
- Enums: `#[serde(rename_all = "snake_case")]`
- Timestamps: `TIMESTAMPTZ` in Postgres, cast to `::TEXT` (ISO 8601) in queries
- FTS: `tsvector` generated columns + GIN indexes; query with `plainto_tsquery()`
- Fuzzy: `pg_trgm` GIN indexes; score with `similarity()`
- Lints: `missing_docs = "warn"`, `unused_trait_names = "deny"`, `unwrap_in_result = "warn"`

## Running tests

Prefer `nextest` which offers process isolation and is generally faster:

```sh
# cargo --locked nextest run --no-fail-fast --all-targets
just test
```

## Post-implementation checks

Verify the following commands succeed after code changes:

```sh
# cargo fmt + cargo check + cargo clippy + cargo nextest
just fmt && just lint && just test
# docker compose build
just cbuild
```

If `just lint` fails, try using `just fix` before attempting to resolve the remarks individually.

## Running

```sh
just up       # Build and start
docker compose down -v             # Reset database
```

MCP on `127.0.0.1:43173`. Postgres via unix socket: `postgres://postgres@%2Frun%2Fpostgresql/rust_mcp`.
