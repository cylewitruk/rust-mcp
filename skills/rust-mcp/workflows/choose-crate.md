# Choose a crate

Goal: pick the best crate for a need, with evidence. Run only the steps the decision needs.

1. **Candidates:** `crate_search(query, sort: "downloads")` searches crates already in the index; filter by `category` or `keyword`. Starting from a known crate, use `crate_alternatives(crate_name)` instead.
2. **Shortlist:** `crate_intel` on the top 2–4 candidates (adoption, maintenance, advisories).
3. **Head to head:** `crate_compare(left_crate, right_crate)` gives adoption, risk and maintenance signals plus a recommendation.
4. **Constraints:**
   - License policy: `crate_license_check(crate_name, allow_licenses or deny_licenses)`. `crate_alternatives` accepts the same lists.
   - Footprint: `dependency_feature_impact(crate_name, features)`.
   - Fit with the project: `dependency_resolve(cargo_toml, additions)`, an estimate; Cargo confirms.
5. **API fit:** skim `crate_api` / `crate_type_info` for the finalists. See [find-api.md](find-api.md).

Tool details: [../tools/discovery.md](../tools/discovery.md), [../tools/ecosystem.md](../tools/ecosystem.md).
