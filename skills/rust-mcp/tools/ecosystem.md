# Comparison & ecosystem tools

## `crate_compare`

Two different crates (`left_crate`, `right_crate`) side by side on adoption, risk and maintenance, with a recommendation. Scores count confirmed affecting advisories; `advisory_unknown_count` reports unevaluated advisories and lowers confidence. A recommendation is withheld if unknowns could change the winning score.

## `crate_alternatives`

Ranked alternatives to a crate by category/keyword overlap and adoption. Accepts `allow_licenses`/`deny_licenses` (SPDX ids). Unknown advisory applicability is reported separately and lowers confidence; it is not a clean security assessment.

## `crate_compatibility`

Whether two crates' dependency requirements are compatible, from indexed data.

## `crate_compatibility_matrix`

The same check across several version pairs of two crates; pass `left_versions`/`right_versions` to choose them.

## `crate_license_check`

License metadata, optionally evaluated against `allow_licenses`/`deny_licenses`.

## `crate_migration_path`

Suggested migration actions for `from_version` → `to_version`, derived from breaking API changes. Replacements are heuristic; verify each before editing code.

## `crate_usage_patterns`

Source snippets from dependent crates that mention a symbol (`symbol_name`). They are text matches: useful examples, not proof of correct usage.

## `crate_hotspots`

Unsafe and concurrency hotspots in a crate's source, filterable by `path_glob`. Keep at least one of `include_unsafe`/`include_concurrency` enabled.
