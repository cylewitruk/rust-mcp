# Find an API

Goal: locate the right item, how to import it, and how to use it. Start at the step that matches what you already know, and stop once the question is answered.

- **Which crate has it?** `symbol_search(query)` finds items by name across indexed crates. For concepts rather than names, use `docs_search(query)`.
- **What does the crate offer?** `crate_api(crate_name, path_glob, kinds)` lists public items. Add `include_docs` for summaries.
- **What does a type look like?** `crate_type_info(crate_name, type_name)` gives its definition, methods and trait impls.
- **Who implements a trait / what does a type implement?** `crate_trait_impls(crate_name, trait_name or type_name)`.
- **How do I import it?** `crate_import_path(crate_name, symbol_name)` resolves the public `use` path through re-exports.
- **What errors can it return?** `crate_error_types(crate_name)` lists error types, their `From` conversions, and the functions returning them.
- **How do others use it?** `crate_usage_patterns(crate_name, symbol_name)` returns text-matched snippets from dependent crates. Treat them as examples, not guarantees.
- **How is it implemented?** Go to the source tools directly: `source_search` → `source_read`, or `source_context` for the surrounding module, imports and impl.

Tool details: [../tools/api.md](../tools/api.md), [../tools/source.md](../tools/source.md).
