# Index & server tools

Most crate tools index unknown crates on demand. Use these only when a response says required data is missing, then retry once.

## `index_crates`

Syncs crates from crates.io and enriches them with source and rustdoc (`crates: [{"name": "serde"}]`). Add `version` to an entry to enrich that exact release, e.g. an older version for `crate_api_diff`; the entry fails if the version does not exist. Rustdoc for older releases may be unavailable from docs.rs, in which case source-derived data is used. Can take a while; progress is streamed.

## `index_status`

Index freshness, coverage and queue state: whether background indexing is still running.

## `index_refresh`

Re-syncs already-indexed data for a `scope`: `crate`, `all`, `security`, `docs` (docs.rs pages for `docs_search`), `local_cache` (source) or `rustdoc_json`. Optionally limited to one `crate_name`.

## `ping`

Connectivity and database readiness check.

## `schema_get`

Full request/response JSON Schema for one tool (`tool_name`) or all tools.
