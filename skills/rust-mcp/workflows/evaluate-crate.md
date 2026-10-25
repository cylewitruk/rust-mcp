# Evaluate a crate

Goal: understand what a crate is, whether it is healthy, and how to use it. Run only the steps the question needs.

1. **Overview:** `crate_intel(crate_name)` covers versions, dependencies, dependents, advisories and a README excerpt. This often answers the question alone.
2. **Release history and risk:** `crate_versions(crate_name)` gives the timeline with yanked, security and adoption markers.
3. **Configuration:** `crate_features(crate_name, version)` lists feature flags, defaults and what each enables.
4. **Shape of the API:** `crate_api(crate_name, kinds)` lists the main entry points; `crate_type_info` covers the central types. See [find-api.md](find-api.md).
5. **Deeper checks, when relevant:**
   - Dependency weight: `crate_graph(crate_name)` or `dependency_feature_impact(crate_name, features)`.
   - Unsafe/concurrency review: `crate_hotspots(crate_name)`.
   - Licensing: `crate_license_check(crate_name, allow_licenses)`.

Comparing against other crates? See [choose-crate.md](choose-crate.md).

Tool details: [../tools/discovery.md](../tools/discovery.md), [../tools/ecosystem.md](../tools/ecosystem.md).
