# Discovery & overview tools

## `crate_search`

Find crates by free text, crates.io category or keyword, sortable by relevance, downloads or recency. Searches crates already in the index; for a specific name, call `crate_intel` directly (it indexes on demand).

## `crate_intel`

Start here for any crate: versions, dependencies, dependents, advisories and a README excerpt. Set `readme_max_chars: 0` to skip the README.

- `advisories` lists only advisories that affect the selected `version` (`applicability: "affected"`) or could not be evaluated (`"unknown"`); `advisories_not_affecting` counts the rest. Full write-ups need `include_advisory_details: true`. Counts deduplicate stored advisory IDs, not all cross-database aliases.
- `dependency_coverage: "not_synced"` means the dependency list is missing, not empty; `index_crates` with that `version` fetches it.
- `version_history[].advisory_unknown_count` distinguishes unevaluated advisories from confirmed affected versions (`has_advisory`).
- `dependents` covers crates in the local index only, not all of crates.io.

## `crate_versions`

Version timeline with yanked, security and adoption markers. `security_advisory_unknown` lowers confidence and means applicability is unresolved. Use it to pick an upgrade target or check a locked version.

## `crate_features`

Feature flags, default features and what each feature transitively enables.

## `crate_graph`

Depth-bounded dependency graph. `direction`: `dependencies` (what it uses), `dependents` (what uses it) or `both`. Raise `depth` sparingly; graphs grow fast.
