# RA Semantic Worker Design

Status: Draft
Owner: rust-mcp maintainers
Last updated: 2026-02-14
RA submodule pin: `vendor/rust-analyzer` @ `c75729db68`

## Summary

A lightweight `rust-mcp-ra-worker` binary that uses rust-analyzer's `hir` crate directly to extract structured semantic data from dependency crate source. It runs as a managed subprocess of rust-mcp, receives extraction requests over stdin, and emits structured results (types, impls, methods, import paths) directly compatible with rust-mcp's database tables.

The worker **does not use the `ide` crate**. All extraction operates at the `hir` level, which provides the complete semantic model (name resolution, type inference, trait solving) without IDE-specific formatting, completions, or refactoring logic. This cuts the dependency surface significantly.

## Why not `ide`

The `ide` crate provides `Analysis` and `AnalysisHost` — the API that LSP servers use. Its public surface is designed for interactive editor queries: hover tooltips (returns markdown), completions (returns labeled items with insert text), go-to-definition (returns navigation targets), inlay hints, code actions, etc.

For dependency indexing, we don't need any of that. We need:

| What we need | `hir` provides | `ide` would add |
| --- | --- | --- |
| "What methods can I call on this type?" | `Type::iterate_method_candidates()` | `Analysis::completions()` — returns editor-formatted completion items we'd have to parse back |
| "What traits does this type implement?" | `Impl::all_for_type()`, `Type::impls_trait()` | Nothing beyond `hir` |
| "What's the canonical import path?" | `Module::find_use_path()` | Nothing beyond `hir` |
| "What are this struct's fields?" | `Type::fields()`, `Struct::fields()` | `Analysis::hover()` — returns markdown we'd have to parse |
| "List all public items in this crate" | `Module::declarations()`, `Module::children()` | `Analysis::file_structure()` — IDE-oriented |
| "What's the return type of this function?" | `Function::ret_type()` | Accessible via hover, but formatted as markdown |

By depending on `hir` + `load-cargo` + `ide-db` (for `RootDatabase`) and skipping `ide`, `ide-completion`, `ide-assists`, `ide-diagnostics`, and `ide-ssr`, we eliminate ~25 seconds from the build critical path and avoid pulling in code we'll never call.

**Build critical path comparison** (from `cargo build --release --timings`):

| Approach | Critical path | Wall clock |
| --- | --- | --- |
| Full `rust-analyzer` binary | `hir-ty` → `hir` → `ide-db` → `ide` → `rust-analyzer` | ~54s |
| Worker (`hir` + `load-cargo` + `ide-db`) | `hir-ty` → `hir` → `ide-db` | ~35s (estimated) |

> **Benchmark context**: Full RA timings from `cargo build --release --timings` on macOS, Apple M3 Max, 16 cores, warm Cargo cache, pinned commit `c75729db68`. Worker estimate is derived by subtracting `ide` (7.3s) + `rust-analyzer` (10.3s) from the full critical path; actual worker build has not been timed yet. Timing report artifact: `vendor/rust-analyzer/target/cargo-timings/`.

## Dependency surface

```text
rust-mcp-ra-worker
├── hir              # Semantic API: types, impls, traits, modules
├── load-cargo       # Workspace loading: ProjectWorkspace → RootDatabase
├── project-model    # ProjectJson, ProjectWorkspace, CargoConfig
├── ide-db           # RootDatabase (Salsa DB implementing HirDatabase)
├── vfs              # Virtual filesystem (file content management)
├── hir-def          # Definition IDs (needed for some hir operations)
├── hir-ty           # (transitive via hir — type inference engine)
├── base-db          # (transitive via ide-db — source database)
└── serde/serde_json # IPC serialization
```

**Not included**: `ide`, `ide-completion`, `ide-assists`, `ide-diagnostics`, `ide-ssr`, `rust-analyzer` (the binary crate).

## Extraction targets

These map directly to rust-mcp's database tables and tool responses.

### 1. Module tree and public items

**Source**: `Crate::root_module()` → DFS via `Module::children()` + `Module::declarations()`

**Extracts**:

- Complete module hierarchy with visibility
- All public `ModuleDef` items: functions, structs, enums, traits, type aliases, constants
- Re-exports visible through `Module::scope(db, Some(root_module))`

**Maps to**: `symbols` table (current schema), `crate.re_exports` tool

### 2. Type information (structs, enums, unions)

**Source**: `Struct::fields()`, `Enum::variants()`, `Variant::fields()`, `Adt::ty()`

**Extracts per type**:

- Fields with resolved types (`Field::ty()`)
- Generic parameters
- `#[repr(...)]` via `Struct::repr()`
- Kind: record/tuple/unit via `Struct::kind()`

**Maps to**: `crate_types` table (current schema), `crate.type_info` tool

### 3. Method and associated item enumeration

**Source**: `Type::iterate_method_candidates()`, `Type::iterate_path_candidates()`

**Extracts per type**:

- All callable methods (inherent + trait), including through auto-deref
- For each method: name, self parameter kind, parameter types, return type, containing trait (if any)
- Associated constants and type aliases via `iterate_path_candidates()`

**Requires**: `SemanticsScope` for trait visibility context. Constructed from the crate's root module.

**Maps to**: `crate_impls` table (`methods` JSONB column, current schema), `crate.type_info` tool

### 4. Trait implementations

**Source**: `Impl::all_in_crate()`, `Impl::all_for_type()`, `Impl::all_for_trait()`

**Extracts per impl block**:

- Self type (`Impl::self_ty()`)
- Trait being implemented (`Impl::trait_()`)
- Associated items (`Impl::items()`)
- Whether negative/unsafe (`Impl::is_negative()`, `Impl::is_unsafe()`)
- Auto trait detection via `Type::is_copy()`, `Type::impls_trait()` for Send/Sync/Unpin

**Maps to**: `crate_impls` table (current schema), `crate.trait_impls` tool

### 5. Canonical import paths

**Source**: `Module::find_use_path(db, item, prefix_kind, cfg)`

**Extracts per public item**:

- Shortest public path from crate root
- Whether the item is re-exported

**Maps to**: `symbols` table (planned: add `canonical_path` column in future migration), `crate.re_exports` tool, `ra.import_path` tool

### 6. Function signatures

**Source**: `Function::ret_type()`, `Function::assoc_fn_params()`, `Function::has_self_param()`

**Extracts per function**:

- Full parameter list with resolved types
- Return type (including async unwrapping via `Function::async_ret_type()`)
- Self parameter kind (value, ref, mut ref)
- Qualifiers: const, async, unsafe
- Generic parameters

**Maps to**: `symbols` table (`signature` column, current schema), `crate_impls` table (`methods` JSONB)

### 7. Trait hierarchy

**Source**: `Trait::direct_supertraits()`, `Trait::all_supertraits()`, `Trait::items()`

**Extracts per trait**:

- Supertrait chain
- Required methods and associated items
- Auto trait / unsafe status
- Dyn-compatibility (`Trait::dyn_compatibility()`)

**Maps to**: `crate_impls` table (planned: current schema lacks `supertraits`, `dyn_compatible` columns — requires future migration), `crate.trait_impls` tool

## Architecture

### Binary structure

```text
crates/
  rust-mcp/          # Main MCP server (existing)
  ra-worker/         # New: semantic extraction worker
    Cargo.toml       # Depends on hir, load-cargo, ide-db, project-model, vfs
    src/
      main.rs        # Entry point: stdin/stdout IPC loop
      extract.rs     # Extraction pipeline
      ipc.rs         # Request/response types (serde)
      project.rs     # rust-project.json construction
```

### IPC protocol

The worker communicates via newline-delimited JSON over stdin/stdout. Each message is a single JSON object on one line.

**Request** (rust-mcp → worker):

```json
{
  "id": "req-001",
  "op": "extract",
  "crate_name": "tokio",
  "crate_version": "1.48.0",
  "source_root": "/cargo/registry/src/index.crates.io-xxx/tokio-1.48.0",
  "sysroot_src": "/rustup-toolchains/stable-.../lib/rustlib/src/rust/library",
  "project_json": { ... },
  "targets": ["modules", "types", "impls", "methods", "traits", "import_paths", "signatures"]
}
```

**Response** (worker → rust-mcp):

```json
{
  "id": "req-001",
  "status": "ok",
  "crate_name": "tokio",
  "crate_version": "1.48.0",
  "extraction_time_ms": 4200,
  "peak_rss_kb": 1048576,
  "modules": [ ... ],
  "types": [ ... ],
  "impls": [ ... ],
  "methods": [ ... ],
  "traits": [ ... ],
  "import_paths": [ ... ],
  "signatures": [ ... ]
}
```

**Lifecycle messages**:

```json
{"op": "ping"}                    → {"op": "pong"}
{"op": "shutdown"}                → process exits
```

### Extraction pipeline

```text
1. Receive request
   │
2. Construct ProjectJsonData from request fields
   │  (crate entries, dependency edges, sysroot_src, editions, features)
   │
3. ProjectJson::new(None, &source_root, data)
   │
4. ProjectWorkspace::load_inline(project_json, &cargo_config, &progress)
   │
5. load_workspace(workspace, &extra_env, &load_config)
   │  Returns (RootDatabase, Vfs, None)
   │  load_config: no proc macros, no build scripts, no cache prefill
   │
6. hir::attach_db(&db, || {
   │    let krate = find_target_crate(&db, "tokio");
   │    let root = krate.root_module(&db);
   │
   │    // DFS module tree
   │    walk_modules(&db, root, |module| {
   │        extract_declarations(&db, module);
   │        extract_impl_blocks(&db, module);
   │    });
   │
   │    // Batch extractions
   │    extract_all_impls(&db, krate);
   │    extract_method_candidates(&db, &public_types);
   │    extract_import_paths(&db, root, &public_items);
   │ })
   │
7. Serialize results → stdout
   │
8. Cleanup
   │  db.trigger_lru_eviction()
   │  hir::clear_tls_solver_cache()
```

### Memory management

Each extraction request creates a fresh `RootDatabase`. After extraction completes and results are serialized:

1. `db.trigger_lru_eviction()` — evict Salsa query caches
2. `hir::clear_tls_solver_cache()` — clear thread-local trait solver state
3. Drop the `RootDatabase` — releases all Salsa storage

The worker process itself stays alive between requests (amortizing process startup), but does not retain analysis state across crates. Each crate gets a clean database.

For crates with large dependency trees, peak RSS may reach 1-4 GB during extraction. The parent rust-mcp process monitors RSS via `/proc/<pid>/status` and sends SIGKILL if `RA_MEMORY_LIMIT_MB` is exceeded.

### Thread-local context

All `hir` queries must execute inside `hir::attach_db(db, || { ... })`. This establishes the thread-local database context required by the trait solver and type inference engine. The extraction pipeline runs single-threaded within this block.

Parallel extraction (e.g., processing multiple types concurrently via rayon) is possible using database snapshots (`db.snapshot()`), but adds complexity. The initial implementation should be single-threaded; parallelism can be added if extraction latency is a bottleneck.

## Concrete extraction code

### Module tree walk

```rust
fn walk_modules(
    db: &RootDatabase,
    root: hir::Module,
    f: &mut impl FnMut(&RootDatabase, hir::Module),
) {
    let mut queue = vec![root];
    let mut visited = FxHashSet::default();

    while let Some(module) = queue.pop() {
        if visited.insert(module) {
            f(db, module);
            queue.extend(module.children(db));
        }
    }
}
```

### Type extraction

```rust
fn extract_type_info(db: &RootDatabase, adt: hir::Adt) -> TypeInfo {
    let name = adt.name(db).to_string();
    let module = adt.module(db);
    let ty = adt.ty(db);
    let display_target = /* ... */;

    let fields = match adt {
        hir::Adt::Struct(s) => s.fields(db).into_iter().map(|f| FieldInfo {
            name: f.name(db).to_string(),
            ty: f.ty(db).display(db, display_target).to_string(),
        }).collect(),
        hir::Adt::Enum(e) => vec![], // variants handled separately
        hir::Adt::Union(u) => u.fields(db).into_iter().map(|f| FieldInfo {
            name: f.name(db).to_string(),
            ty: f.ty(db).display(db, display_target).to_string(),
        }).collect(),
    };

    // Auto traits (checked via Type::impls_trait)
    let send = check_auto_trait(db, &ty, "Send");
    let sync = check_auto_trait(db, &ty, "Sync");
    let is_copy = ty.is_copy(db);

    TypeInfo { name, fields, send, sync, is_copy, /* ... */ }
}
```

### Method candidate extraction

```rust
fn extract_methods(
    db: &RootDatabase,
    ty: &hir::Type<'_>,
    scope: &hir::SemanticsScope<'_>,
) -> Vec<MethodInfo> {
    let mut methods = Vec::new();

    ty.iterate_method_candidates(db, scope, None, |func| {
        let name = func.name(db).to_string();
        let ret_ty = func.ret_type(db).display(db, display_target).to_string();
        let has_self = func.has_self_param(db);
        let params = func.params_without_self(db)
            .into_iter()
            .map(|p| ParamInfo {
                ty: p.ty().display(db, display_target).to_string(),
                name: p.name(db).map(|n| n.to_string()),
            })
            .collect();

        // Which trait provides this method (if any)
        let trait_name = func.module(db)
            .parent(db) // trait's module
            .and_then(|_| /* resolve trait from impl */);

        methods.push(MethodInfo { name, ret_ty, has_self, params, trait_name });
        None::<()> // continue iterating
    });

    methods
}
```

### Import path extraction

```rust
fn extract_import_paths(
    db: &RootDatabase,
    root_module: hir::Module,
    items: &[hir::ModuleDef],
) -> Vec<ImportPathInfo> {
    let prefix_kind = hir::PrefixKind::ByCrate;
    let cfg = hir::FindPathConfig {
        prefer_no_std: false,
        prefer_prelude: true,
        prefer_absolute: false,
        allow_unstable: false,
    };

    items.iter().filter_map(|item| {
        let path = root_module.find_use_path(db, (*item).into(), prefix_kind, cfg)?;
        Some(ImportPathInfo {
            path: path.to_string(),
            item_name: /* ... */,
            item_kind: /* ... */,
        })
    }).collect()
}
```

### Trait impl extraction

```rust
fn extract_trait_impls(db: &RootDatabase, krate: hir::Crate) -> Vec<TraitImplInfo> {
    let all_impls = hir::Impl::all_in_crate(db, krate);
    let display_target = krate.to_display_target(db);

    all_impls.into_iter().filter_map(|impl_| {
        let self_ty = impl_.self_ty(db).display(db, display_target).to_string();
        let trait_ = impl_.trait_(db)?; // skip inherent impls
        let trait_name = trait_.name(db).to_string();
        let items = impl_.items(db).into_iter().map(|item| match item {
            hir::AssocItem::Function(f) => AssocItemInfo::Method {
                name: f.name(db).to_string(),
                has_body: f.has_body(db),
            },
            hir::AssocItem::Const(c) => AssocItemInfo::Const {
                name: c.name(db).map(|n| n.to_string()),
            },
            hir::AssocItem::TypeAlias(ta) => AssocItemInfo::TypeAlias {
                name: ta.name(db).to_string(),
            },
        }).collect();

        Some(TraitImplInfo {
            self_ty,
            trait_name,
            trait_module_path: /* ... */,
            is_negative: impl_.is_negative(db),
            is_unsafe: impl_.is_unsafe(db),
            items,
        })
    }).collect()
}
```

## SemanticsScope construction

Several extraction targets (notably `iterate_method_candidates`) require a `SemanticsScope`. This provides the trait visibility context — which traits are in scope affects which methods are callable.

For dependency analysis, we want the broadest possible scope: all traits in the crate and its dependencies should be visible. The construction:

```rust
let sema = hir::Semantics::new(db);

// Get the root module's source file.
// Crate::root_file() returns FileId, but Semantics::parse() requires
// EditionedFileId. Use parse_guess_edition() which internally calls
// attach_first_edition() to resolve the edition from the crate graph.
// This matches the pattern used in analysis_stats.rs:502.
let root_file = krate.root_file(db);
let source_file = sema.parse_guess_edition(root_file);

// Get the scope at the top of the root file
let scope = sema.scope(&source_file.syntax())?;
```

This gives us a `SemanticsScope` rooted at the crate's lib.rs/main.rs, which has visibility to all `use` imports in that file. For comprehensive method discovery, we may need to construct scopes that include all trait imports — see "Spike acceptance criteria" below.

## rust-project.json construction

The worker receives `project_json` in the request, but rust-mcp constructs it from indexed `dependency_edges` data.

**Important**: `ProjectJsonData`, `CrateData`, and `Dep` are not constructible from outside `project-model` — their fields are private or `pub(crate)`. The correct approach is to build a JSON payload matching the `rust-project.json` schema and deserialize it via `serde_json::from_value::<ProjectJsonData>()`, then pass the deserialized value to `ProjectJson::new(manifest, base, data)`.

```rust
// Build the JSON payload matching rust-project.json schema
let project_data = serde_json::json!({
    "sysroot_src": request.sysroot_src,
    "crates": dependency_edges_to_crate_entries(&edges, &source_root),
    "runnables": []
});

// Deserialize into ProjectJsonData (fields are private, so we go through serde)
let data: ProjectJsonData = serde_json::from_value(project_data)?;
let project_json = ProjectJson::new(None, &source_root, data);
let workspace = ProjectWorkspace::load_inline(project_json, &cargo_config, &|_| {});

let load_config = LoadCargoConfig {
    load_out_dirs_from_check: false,  // Safe mode: no build scripts
    with_proc_macro_server: ProcMacroServerChoice::None,
    prefill_caches: false,
    proc_macro_processes: 0,
};

let (db, _vfs, _) = load_workspace(workspace, &FxHashMap::default(), &load_config)?;
```

Each crate in the dependency tree becomes a JSON object matching `CrateData`'s serde schema:

```rust
fn crate_entry(name: &str, root_module: &str, edition: &str,
               version: &str, deps: &[(usize, &str)]) -> serde_json::Value {
    serde_json::json!({
        "display_name": name,
        "root_module": root_module,
        "edition": edition,
        "version": version,
        "deps": deps.iter().map(|(idx, dep_name)| serde_json::json!({
            "crate": idx,
            "name": dep_name,
        })).collect::<Vec<_>>(),
        "is_workspace_member": true,
    })
}
```

## Spike acceptance criteria

These must be validated empirically before the design is considered implementation-ready.

1. **Trait visibility scope coverage** (gating): Does a root-file `SemanticsScope` see all traits in the crate's dependency graph, or only those explicitly `use`-imported in the root file? The resolver scope (`semantics.rs:2553`) determines which traits are visible to `iterate_method_candidates()`. If the root-file scope is too narrow, methods from un-imported traits will be silently missing. **Acceptance test**: compare method candidate output for a well-known type (e.g. `Vec<u8>`) against rustdoc JSON output for the same type; coverage delta must be < 5%.

2. **Blanket impl coverage** (gating): `Impl::all_for_type()` documents it's an "approximation" excluding blanket impls. Measure the coverage gap against `Impl::all_in_crate()` + filtering + `Type::impls_trait()` validation for a crate with significant blanket impls (e.g. `serde`, `tokio`). If the gap is > 10%, the extraction pipeline must use the fallback approach.

## Open design questions

1. **Display target**: `Type::display()` requires a `DisplayTarget`. For dependency analysis, should we use `DisplayTarget::SourceCode` (shortest path) or `DisplayTarget::Diagnostics` (fully qualified)? Probably source code for user-facing output.

2. **Process lifecycle**: Should the worker stay alive and handle multiple sequential requests (amortizing startup), or spawn fresh per crate? Staying alive is better for throughput but means we need to ensure clean DB teardown between requests.

3. **Error reporting granularity**: When extraction partially fails (e.g., some types have unresolved inference), should we report per-item errors or just reduce confidence globally? Per-item is more useful for debugging but more verbose.

4. **Feature flag handling**: How do we tell RA which cargo features to enable for the target crate? The `CrateData.cfg` field can set `feature = "xxx"` atoms, but we need to decide: default features only, all features, or user-configurable?

## Relationship to ra-integration.md

This document is the detailed design for the **primary RA integration path** — the semantic extraction worker. It supersedes the LSP subprocess approach and prototype call chain in [ra-integration.md](ra-integration.md) with a complete extraction pipeline. The spike acceptance criteria defined in both documents apply to this design.

Interactive positional tools (`ra.type_at`, `ra.completions`, `ra.definition`, etc.) are **out of scope** for this phase. They require an LSP subprocess, which may be revisited in a future phase if user-workspace positional intelligence becomes a product goal. The current focus is batch semantic extraction for dependency indexing during `index.refresh`.
