# AGENTS.md

Instructions for LLM/code agents working in this repository.

This file is the agent-facing source of truth, alongside:

- `README.md` for end users
- `docs/ROADMAP.md` for future work

## Project Summary

`rust-mcp` is a local-first Rust dependency intelligence MCP server.

- Runtime: single container with embedded PostgreSQL.
- Primary protocol: MCP Streamable HTTP at `/mcp`.
- HTTP endpoints: `/healthz`, `/readyz`, `/schemas`, `/schemas/{tool_name}`.
- Metrics: Prometheus exporter on a dedicated bind.

## Current Source Layout

```text
crates/rust-mcp/
  src/
    main.rs
    app.rs
    config.rs
    contracts.rs               # tool-schema catalog assembly + export
    http.rs
    state.rs
    mcp/
      server.rs                 # tool registration + instrumentation
      transport.rs              # Streamable HTTP service config
      metrics.rs                # DB-backed tool invocation metrics
      query_cache.rs            # short-lived result memoization
      models.rs                 # shared model types
      indexing/
        handlers.rs             # index_crates / index_status / index_refresh
        worker.rs               # durable refresh worker loop
        freshness.rs
        local_cache.rs
        rustdoc_json.rs
        security.rs
      tools/
        krate/                  # crate_* handlers
        dependency/             # dependency_* handlers
        source/                 # source_* handlers
        docs.rs                 # docs_search
        symbol.rs               # symbol_search
  tests/
    common.rs
    integration/
      mod.rs
      core_tools.rs
      crate_tools/
      dependency_tools/
      docs_tools.rs
      index_tools/
      postgres.rs
      source_tools/
      symbol_tools.rs
    e2e/
      mod.rs
      helpers.rs
      protocol/
      tool_calls/
      crates_io_fixtures.rs
      docs_rs_fixtures.rs
```

## MCP Tool Inventory (Current)

- Core/index: `ping`, `schema_get`, `index_crates`, `index_status`, `index_refresh`
- Crate: `crate_search`, `crate_intel`, `crate_features`, `crate_api_diff`, `crate_api`, `crate_type_info`, `crate_trait_impls`, `crate_re_exports`, `crate_import_path`, `crate_error_types`, `crate_derive_macros`, `crate_compare`, `crate_compatibility`, `crate_compatibility_matrix`, `crate_migration_path`, `crate_license_check`, `crate_alternatives`, `crate_versions`, `crate_graph`, `crate_hotspots`, `crate_usage_patterns`
- Dependency: `dependency_audit`, `dependency_resolve`, `dependency_feature_impact`
- Source/symbol/docs: `source_search`, `source_read`, `source_context`, `symbol_search`, `docs_search`

## Implementation Conventions

- Handler flow: `validate -> query -> transform -> response`.
- Register tools in `src/mcp/server.rs` with `#[tool(...)]`.
- Prefer SQLx `QueryBuilder` + `.push_bind()` for user-provided values.
- Keep module ownership clear:
  - index lifecycle in `mcp/indexing/*`
  - query/tool logic in `mcp/tools/*`
- Preserve response envelope consistency:
  - confidence fields (`confidence`, `confidence_assessment`)
  - `suggested_next_tools`
  - provenance/freshness fields where applicable
- Avoid `pub(crate)`, `pub(mod)`, `pub(super)`, etc. visibility modifiers. Prefer appropriately scoped items together with appropriately scoped exports/imports.

## Protocol Expectations

- Session flow: `initialize` -> `notifications/initialized` -> `tools/list` / `tools/call`.
- Streamable HTTP may return JSON or SSE (`text/event-stream`), so tests/harnesses must parse JSON `data:` events for SSE responses.
- Include/propagate `mcp-session-id` header across requests after initialize.

## Testing

`crates/rust-mcp/Cargo.toml` defines two explicit integration test targets:

- `integration` (`tests/integration/mod.rs`) behind `integration-tests`
- `e2e_http` (`tests/e2e/mod.rs`) behind `e2e-tests`

Common commands:

```sh
just build          # builds the project
just fmt            # formats code using the project-specific rustfmt rules
just lint           # run `cargo clippy` with the correct arguments
just fix            # attempt to fix clippy lints automatically + run `fmt`
just test           # runs lib + integration tests (from `tests/integration`)
just test-e2e       # runs e2e integration tests (from `tests/e2e`)
```

Use targeted runs while iterating:

```sh
cargo --locked nextest run -p rust-mcp --features integration-tests --test integration <filter>
cargo --locked nextest run -p rust-mcp --features e2e-tests --test e2e_http <filter>
```

## Operational Notes

- Core runtime is HTTP-only; stdio support should live in a separate adapter binary.
- Refresh worker and startup rustdoc sync are spawned from `app.rs`.
- Docker entrypoint boots embedded PostgreSQL and can apply outbound host allowlisting.

## Docs Policy

- Keep current-state user guidance in `README.md`.
- Keep agent/developer workflow guidance in `AGENTS.md`.
- Keep only forward-looking backlog in `docs/ROADMAP.md`.
- Remove or avoid reintroducing date-stamped spec/roadmap docs as active references.
