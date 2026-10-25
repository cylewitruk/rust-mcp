# Upgrade a dependency

Goal: move a crate from version A to B and fix what breaks. Use the version from the project's Cargo.lock as A.

1. **Pick the target:** `crate_versions(crate_name)`. Avoid yanked releases; note security markers.
2. **What changed:** `crate_api_diff(crate_name, from_version, to_version)` lists public API symbols added, removed and changed. If a version is missing, call `index_crates` with `crates: [{name, version}]` for it and retry.
3. **What to do:** `crate_migration_path(crate_name, from_version, to_version)` derives suggested actions from the breaking changes. Treat replacements as suggestions and verify each one.
4. **Deprecations:** `crate_deprecated(crate_name, version: to_version)` lists deprecated items with suggested replacements.
5. **Features:** `crate_features(crate_name, version: to_version)`. A requested feature that was removed or renamed makes dependency resolution fail.
6. **Compatibility estimate:** `dependency_resolve(cargo_toml, additions: [{name, version_req: "<new requirement>"}])`. The addition replaces the manifest's requirement for that crate, but only in the simulation. For one specific pair of crates, `crate_compatibility(left_crate, right_crate)`.
7. **Apply it:** change the requirement in Cargo.toml, then confirm with Cargo (`cargo update -p <crate>`, then build).
8. **Fix call sites:** for each broken symbol, use [find-api.md](find-api.md) against `to_version`.

Tool details: [../tools/api.md](../tools/api.md), [../tools/ecosystem.md](../tools/ecosystem.md), [../tools/dependencies.md](../tools/dependencies.md).
