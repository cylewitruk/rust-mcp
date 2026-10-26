# Dependency tools

## `dependency_audit`

Checks a manifest for yanked versions, advisories, outdated requirements and MSRV conflicts. Pass Cargo.toml **file text** (`cargo_toml`), not a path.

- Each declared requirement is checked against the highest indexed version that satisfies it. Cargo.lock, transitive dependencies, `workspace = true` inheritance and `package` renames are **not** covered.
- Advisories with unknown applicability get a separate marker and lower confidence; they are not counted as confirmed affected.
- Inspect `unresolved` findings: either the crate is not indexed (index it with `index_crates` and re-run) or no indexed version satisfies the requirement (check the requirement itself, and `index_refresh` the crate if the index may be stale).

## `dependency_resolve`

Estimates whether a set of requirements is mutually compatible, using indexed data. Inputs: `cargo_toml` (file text), `dependencies` and/or `additions` as `[{"name": "serde", "version_req": "^1"}]`.

- An `addition` with a requirement **replaces** the manifest's requirement for that crate, so "what if I bump X?" is `cargo_toml` plus `additions: [{name: X, version_req}]`.
- `limit` caps how many input dependencies are evaluated; it is not a depth.
- `check_features` adds counts of referenced features and optional dependencies, not Cargo's feature resolution.
- This is an estimate. Only Cargo can confirm the project resolves and builds.

## `dependency_feature_impact`

Extra dependency surface pulled in by enabling `features` (a non-empty list) on a crate. It takes a crate name, not a manifest.
