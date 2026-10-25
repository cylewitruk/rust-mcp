# Discovery & overview tools

## `crate_search`

Find crates by free text, crates.io category or keyword, sortable by relevance, downloads or recency. Searches crates already in the index; for a specific name, call `crate_intel` directly (it indexes on demand).

## `crate_intel`

Start here for any crate: versions, dependencies, dependents, advisories (for the selected `version`), README excerpt. Set `readme_max_chars: 0` to skip the README.

## `crate_versions`

Version timeline with yanked, security and adoption markers. Use it to pick an upgrade target or check a locked version.

## `crate_features`

Feature flags, default features and what each feature transitively enables.

## `crate_graph`

Depth-bounded dependency graph. `direction`: `dependencies` (what it uses), `dependents` (what uses it) or `both`. Raise `depth` sparingly; graphs grow fast.
