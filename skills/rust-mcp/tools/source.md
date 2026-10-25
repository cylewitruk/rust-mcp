# Source, symbol & docs tools

Use these for implementation details, finding items by name across crates, and prose documentation. For "what exists, how is it typed", the [API surface tools](api.md) are cheaper.

## `symbol_search`

Find symbols by name (`query`) across indexed crates, optionally filtered by `crate_name`, `version` and `kind`.

## `docs_search`

Full-text search over indexed docs.rs pages (guides, module docs, examples). If the pages are missing, `index_refresh` with `scope: "docs"` fetches them.

## `source_search`

Text or regex (`mode`) search in crate source, filterable by `crate_name`, `version` and `path_glob`. Use it to find the file and line before `source_read`.

## `source_read`

A line range from a crate source file. `path` is relative to the crate root, e.g. `"src/de/mod.rs"`. Indexed files are text sources (`.rs`, `.toml`, `.md`, `.json`, `.yaml`/`.yml`, `.txt`); others, such as `Cargo.toml.orig`, are not available.

## `source_context`

Semantic context around a location: module path, imports, containing impl, nearby types. **Requires `line` or `symbol_name`** in addition to `crate_name` and `path`.
