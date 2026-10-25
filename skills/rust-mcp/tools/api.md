# API surface tools

These read rustdoc data (indexed on demand). Each takes `crate_name` and an optional `version`, except `crate_api_diff`, which takes `from_version` and `to_version`.

## `crate_api`

Public API symbols, filterable by `path_glob` (e.g. `"*connection*"`) and `kinds` (e.g. `["fn", "struct", "trait"]`). Add `include_docs` for summaries.

## `crate_type_info`

One type in detail (`type_name`): definition, fields/variants, inherent methods and trait impls.

## `crate_trait_impls`

Which types implement a trait, or which traits a type implements. **Requires `trait_name` or `type_name`.**

## `crate_import_path`

The public `use` path for a symbol (`symbol_name`), accounting for re-exports.

## `crate_re_exports`

Re-export mappings to canonical paths, filterable by `path_prefix` (e.g. `"tokio::io"`).

## `crate_error_types`

Error types, their conversion impls, and the functions returning each.

## `crate_deprecated`

Deprecated items with notes and suggested replacements. Paginates by `page`/`limit` (no cursor).

## `crate_derive_macros`

Proc-macro exports (derive, attribute, function-like). Query the macro crate itself, e.g. `serde_derive`.

## `crate_api_diff`

Public API changes between `from_version` and `to_version`: added, removed, changed. If a version is reported missing, call `index_crates` with `crates: [{name, version}]` for it and retry.
