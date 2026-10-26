# Index & server tools

Most crate tools index unknown crates on demand. Use these only when a response says required data is missing, then retry once.

## `index_crates`

Syncs crates from crates.io and enriches them with source and rustdoc (`crates: [{"name": "serde"}]`). Add `version` to an entry to enrich that exact release (dependencies, source, rustdoc), e.g. an older version for `crate_api_diff` or a `not_synced` dependency list; the entry fails if the version does not exist or requested dependency synchronization fails. `synced_dependencies` counts edges for the returned version; inspect each entry’s `error`. Rustdoc for older releases may be unavailable from docs.rs, in which case source-derived data is used. Can take a while; progress is streamed.

## `index_status`

Index freshness, coverage and queue state: whether background indexing is still running.

## `index_refresh`

Re-syncs already-indexed data for a `scope`: `crate`, `all`, `security`, `docs` (docs.rs pages for `docs_search`), `local_cache` (source) or `rustdoc_json`. Some scopes can be limited to one `crate_name`; `security` processes a page of indexed crates across OSV, GHSA and configured RustSec sources. Legacy advisory matches stay unknown until their source is refreshed successfully. Queued security jobs retry source failures; inline refreshes report `completed_with_errors`.

## `ping`

Connectivity and database readiness check.

## `schema_get`

Full request/response JSON Schema for one tool (`tool_name`) or all tools.
