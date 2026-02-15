# AGENTS.md

Local-first Rust dependency intelligence MCP server. Single Docker container with embedded PostgreSQL.

## Project layout

```text
crates/rust-mcp/
  src/
    main.rs            # Binary entry point (calls app::run())
    lib.rs             # Library root: module declarations
    app.rs             # Startup lifecycle: config -> DB -> migrations -> workers -> serve
    config.rs          # All config via clap (CLI flags + env vars)
    state.rs           # AppState: PgPool + reqwest::Client + Config + OutboundRateLimiters
    error.rs           # ApiError enum with axum IntoResponse
    http.rs            # GET /healthz, GET /readyz, router assembly
    logging.rs         # Tracing/logging initialization
    mcp/
      server.rs        # Tool registration via rmcp #[tool_router] macro
      models.rs        # All request/response/DB row types
      utils.rs         # Input validation: normalize, clamp, glob-to-like
      transport.rs     # MCP transport configuration (StreamableHttpService)
      metrics.rs       # tool_invocations recording
      query_cache.rs   # query_cache table ops
      # --- Indexing infrastructure ---
      index.rs         # index.status, index.refresh, refresh worker, freshness probes, adaptive TTL
      local_cache.rs   # Cargo registry scanner + syn-based symbol extraction
      rustdoc_json.rs  # Typed rustdoc JSON parsing via rustdoc-types + DB ingestion
      security.rs      # RustSec/OSV advisory ingestion
      # --- Tool handlers (one per file, see tool table below) ---
      alternatives.rs  # crate.alternatives
      api_diff.rs      # crate.api_diff
      api_surface.rs   # crate.api
      compare.rs       # crate.compare
      compatibility.rs # crate.compatibility, crate.compatibility_matrix
      dependency_audit.rs    # dependency.audit
      dependency_resolve.rs  # dependency.resolve
      derive_macros.rs # crate.derive_macros
      docs_intel.rs    # docs.search
      error_types.rs   # crate.error_types
      feature_impact.rs # dependency.feature_impact
      features.rs      # crate.features
      graph.rs         # crate.graph
      hotspots.rs      # crate.hotspots
      intel.rs         # crate.intel
      license.rs       # crate.license_check
      migration_path.rs # crate.migration_path
      re_exports.rs    # crate.re_exports
      search.rs        # crate.search
      source.rs        # source.search, source.read
      source_context.rs # source.context
      symbol.rs        # symbol.search
      trait_impls.rs   # crate.trait_impls
      type_info.rs     # crate.type_info
      usage_patterns.rs # crate.usage_patterns
      versions.rs      # crate.versions
    bin/
      benchmark_mcp.rs # MCP benchmark harness
      load_test_mcp.rs # MCP load-test harness
docs/                  # Architectural, design and impl documentation
migrations/            # SQLx Postgres migrations (0001–0007)
vendor/                # Vendored crates/source files (git subtrees)
```

## MCP tools

39 tools registered in `server.rs`. Each follows **validate -> query -> transform -> envelope**.

| Tool | Handler | Summary |
|------|---------|---------|
| `ping` | server.rs | Check MCP connectivity and basic DB readiness |
| `index.sync_crates` | index.rs | Fetch crate metadata from crates.io and upsert |
| `index.status` | index.rs | Return index freshness, coverage, queue state |
| `index.refresh` | index.rs | Trigger index refresh for a scope |
| `crate.search` | search.rs | Search crates by name, category, keyword, description |
| `crate.intel` | intel.rs | Dense crate intelligence: versions, deps, dependents, advisories |
| `crate.features` | features.rs | Feature flags, defaults, transitive enables |
| `crate.api_diff` | api_diff.rs | Compare public symbols between two crate versions |
| `crate.api` | api_surface.rs | Public API symbols with kind/path filters |
| `crate.type_info` | type_info.rs | Type definition metadata + associated impl details |
| `crate.trait_impls` | trait_impls.rs | Trait/type implementation relationships |
| `crate.re_exports` | re_exports.rs | Public re-export → canonical import-path mappings |
| `crate.error_types` | error_types.rs | Error types, From conversions, returning functions |
| `crate.derive_macros` | derive_macros.rs | Proc-macro exports (derive, attribute, function-like) |
| `crate.compare` | compare.rs | Compare two crates on adoption, risk, maintenance |
| `crate.compatibility` | compatibility.rs | Pairwise dependency compatibility check |
| `crate.compatibility_matrix` | compatibility.rs | Multi-version compatibility matrix |
| `crate.migration_path` | migration_path.rs | Migration actions from API diff breaking changes |
| `crate.license_check` | license.rs | License metadata + allow/deny policy evaluation |
| `crate.alternatives` | alternatives.rs | Ranked alternative crate suggestions |
| `crate.versions` | versions.rs | Normalized version timeline with yanked/security markers |
| `crate.graph` | graph.rs | Depth-bounded dependency/dependent graph |
| `crate.hotspots` | hotspots.rs | Unsafe and concurrency hotspots in source |
| `crate.usage_patterns` | usage_patterns.rs | Real source snippets from dependent crates using a symbol |
| `dependency.audit` | dependency_audit.rs | Audit Cargo.toml for yanked, advisories, MSRV conflicts |
| `dependency.resolve` | dependency_resolve.rs | Best-effort compatibility simulation for proposed deps |
| `dependency.feature_impact` | feature_impact.rs | Dependency surface added by feature flags |
| `source.search` | source.rs | Search indexed source by text/regex |
| `source.read` | source.rs | Read line range from indexed source file |
| `source.context` | source_context.rs | Semantic source context around a location |
| `symbol.search` | symbol.rs | Search symbols by name with crate/version/kind filters |
| `docs.search` | docs_intel.rs | Search indexed docs.rs pages |

## Tool implementation pattern

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
# cargo +nightly --locked fmt --all
# RUST_LOG=warn cargo --locked clippy --all-targets -- -D warnings
# cargo check --locked --all-targets
# cargo +nightly --locked fmt --all -- --check
# cargo --locked nextest run --no-fail-fast --all-targets
just fmt && just lint && just test
# docker compose build
just cbuild
```

If `just lint` fails, try using `just fix` before attempting to resolve the remarks individually.

## Running

```sh
just up                   # Build and start
docker compose down # -v  # Stop/remove (-v resets database via volume drop)
```

MCP on `127.0.0.1:43173`. Prometheus metrics on `127.0.0.1:9090`. Postgres via unix socket: `postgres://postgres@%2Frun%2Fpostgresql/rust_mcp`.
