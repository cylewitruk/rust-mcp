# Check dependencies

Goal: find risky, outdated or conflicting dependencies in a project.

**Know the limits first.** `dependency_audit` checks each *declared requirement* in a Cargo.toml against the highest indexed version that satisfies it. It does **not** read Cargo.lock, inspect transitive dependencies, resolve `workspace = true` inheritance, or follow `package = "..."` renames. A vulnerable *locked* version can therefore be missed. Use `cargo audit` (or similar) when lockfile-level coverage matters, and say which coverage you provided.

1. **Audit the manifest:** pass the Cargo.toml **text** to `dependency_audit(cargo_toml)`. It reports yanked versions, advisories, outdated requirements and MSRV conflicts.
   - In a workspace, audit each member's manifest. Replace `workspace = true` entries with the workspace's requirement first, or note them as unchecked.
   - For `unresolved` findings, index crates that are missing (`index_crates`) and re-run. If no version satisfies a requirement, check the requirement and whether the index is stale (`index_refresh`).
2. **Check locked versions** that matter: look them up directly with `crate_versions(crate_name)` (yanked/security markers) or `crate_intel(crate_name, version)` (advisories).
3. **Explain findings:** for an advisory, yanked or outdated dependency, find a fixed release with `crate_versions` and follow [upgrade-dependency.md](upgrade-dependency.md).
4. **Optional:**
   - Requirement conflicts: `dependency_resolve(cargo_toml)`, an estimate from indexed data.
   - Weight: `dependency_feature_impact(crate_name, features)`.
   - Licenses: `crate_license_check(crate_name, allow_licenses or deny_licenses)`.

Tool details: [../tools/dependencies.md](../tools/dependencies.md), [../tools/discovery.md](../tools/discovery.md).
