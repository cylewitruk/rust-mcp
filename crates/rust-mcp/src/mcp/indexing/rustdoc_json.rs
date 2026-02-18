use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use rustdoc_types::{
    Attribute, Crate as RustdocCrate, Enum, FunctionPointer, FunctionSignature, GenericArg,
    GenericArgs, GenericBound, GenericParamDef, GenericParamDefKind, Id, Impl, Item, ItemEnum,
    Span, Struct, StructKind, TraitBoundModifier, Type as RustdocType, TypeAlias, Union,
    VariantKind, Visibility as RustdocVisibility, WherePredicate,
};
use semver::Version;
use serde_json::json;
use sha2::{Digest as _, Sha256};

use crate::db::indexing::{
    fetch_rustdoc_sync_candidates, fetch_source_file_id_required, replace_crate_version_index_rows,
    upsert_source_file_unconditional,
};
use crate::db::models::{
    IndexedExtractionBatch, IndexedImplInsert, IndexedSymbolInsert, IndexedTraitInsert,
    IndexedTypeInsert, RustdocSyncCandidateRow,
};
use crate::integration::docs_rs::{
    DocsRsClient, decode_docs_rs_rustdoc_payload, docs_rs_rustdoc_synthetic_path,
};
use crate::mcp::server::McpServer;
use crate::mcp::utils::{normalize_required, sync_page, sync_per_page};

// ============================================================
// Outcome tracking
// ============================================================

#[derive(Debug, Default)]
pub(crate) struct RustdocJsonRefreshOutcome {
    pub(crate) scanned_files: usize,
    pub(crate) synced_versions: usize,
    pub(crate) symbols_written: usize,
    pub(crate) types_written: usize,
    pub(crate) impls_written: usize,
    pub(crate) traits_written: usize,
    pub(crate) touched_versions: Vec<String>,
    pub(crate) errors: Vec<String>,
}

// ============================================================
// Intermediate extraction structs
// ============================================================

#[derive(Debug)]
struct RustdocCandidate {
    path: PathBuf,
    crate_name: String,
    crate_version: Option<String>,
}

// ============================================================
// File discovery helpers (unchanged)
// ============================================================

fn file_sha256_hex(content: &[u8]) -> String {
    let digest = Sha256::digest(content);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

fn walk_json_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut pending = vec![root.to_path_buf()];
    let mut files = Vec::<PathBuf>::new();

    while let Some(dir) = pending.pop() {
        let entries = std::fs::read_dir(&dir)
            .map_err(|e| format!("failed to read directory {}: {e}", dir.display()))?;

        for entry in entries {
            let entry = entry.map_err(|e| {
                format!("failed to read directory entry under {}: {e}", dir.display())
            })?;
            let path = entry.path();
            let file_type = entry
                .file_type()
                .map_err(|e| format!("failed to inspect {}: {e}", path.display()))?;

            if file_type.is_dir() {
                pending.push(path);
            } else if file_type.is_file()
                && path
                    .extension()
                    .and_then(OsStr::to_str)
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
            {
                files.push(path);
            }
        }
    }

    files.sort();
    Ok(files)
}

fn crate_from_stem(stem: &str) -> (String, Option<String>) {
    let normalized = stem.trim();
    if normalized.is_empty() {
        return (String::new(), None);
    }

    let segments = normalized
        .split('-')
        .collect::<Vec<_>>();
    if segments.len() >= 2 {
        for split_index in 1..segments.len() {
            let version_candidate = segments[split_index..].join("-");
            let semver_candidate = version_candidate.trim_start_matches('v');
            if Version::parse(semver_candidate).is_ok() {
                let crate_name = segments[..split_index].join("-");
                if !crate_name.is_empty() {
                    return (crate_name, Some(version_candidate));
                }
            }
        }
    }

    (normalized.to_string(), None)
}

fn synthetic_rustdoc_path(root_dir: &Path, file_path: &Path) -> String {
    let relative = file_path
        .strip_prefix(root_dir)
        .ok()
        .map(|value| {
            value
                .to_string_lossy()
                .replace('\\', "/")
        })
        .or_else(|| {
            file_path
                .file_name()
                .and_then(OsStr::to_str)
                .map(ToString::to_string)
        })
        .unwrap_or_else(|| "unknown.json".to_string());

    format!("rustdoc-json/{relative}")
}

fn format_rustdoc_parse_error(
    candidate: &RustdocSyncCandidateRow,
    source_path: &str,
    content: &str,
    parse_error: &serde_json::Error,
) -> String {
    let format_version = serde_json::from_str::<serde_json::Value>(content)
        .ok()
        .and_then(|value| {
            value
                .get("format_version")
                .and_then(serde_json::Value::as_u64)
        });

    match format_version {
        Some(version) => format!(
            "failed to parse rustdoc JSON payload for {}@{} from {}: {} (payload \
             format_version={version}, expected={}; verify rustdoc-types compatibility)",
            candidate.crate_name,
            candidate.version,
            source_path,
            parse_error,
            rustdoc_types::FORMAT_VERSION,
        ),
        None => format!(
            "failed to parse rustdoc JSON payload for {}@{} from {}: {} (payload is missing \
             format_version; ensure this is rustdoc JSON output)",
            candidate.crate_name, candidate.version, source_path, parse_error,
        ),
    }
}

fn diagnostic_hint_for_sync_error(error: &str) -> Option<&'static str> {
    if error.contains("RUSTDOC_JSON_DIR is not configured") {
        Some("set RUSTDOC_JSON_DIR to enable local rustdoc fallback")
    } else if error.contains("rustdoc JSON directory not found") {
        Some("verify RUSTDOC_JSON_DIR points to an existing directory")
    } else if error.contains("payload format_version=") {
        Some("re-generate rustdoc JSON with a compatible nightly / rustdoc-types format")
    } else if error.contains("crate mismatch") {
        Some("ensure rustdoc file name and payload crate match the indexed crate version")
    } else if error.contains("could not be decoded") {
        Some("confirm docs.rs payload is gzip/zstd-compressed rustdoc JSON")
    } else if error.contains("invalid UTF-8") {
        Some("ensure local fallback files are UTF-8 JSON documents")
    } else if error.contains("no local rustdoc JSON file found") {
        Some("add a matching <crate>-<version>.json file in RUSTDOC_JSON_DIR")
    } else {
        None
    }
}

fn format_candidate_sync_failure(
    crate_name: &str,
    version: &str,
    source_errors: &[String],
) -> String {
    let mut message = format!("rustdoc JSON sync failed for {crate_name}@{version}");
    for (index, error) in source_errors
        .iter()
        .enumerate()
    {
        let attempt = index + 1;
        let _ = write!(message, " | attempt #{attempt}: {error}");
        if let Some(hint) = diagnostic_hint_for_sync_error(error) {
            let _ = write!(message, " (hint: {hint})");
        }
    }
    message
}

// ============================================================
// Type rendering
// ============================================================

fn render_type(ty: &RustdocType, krate: &RustdocCrate) -> String {
    match ty {
        RustdocType::ResolvedPath(path) => render_path(path, krate),
        RustdocType::Generic(name) => name.clone(),
        RustdocType::Primitive(name) => name.clone(),
        RustdocType::BorrowedRef { lifetime, is_mutable, type_ } => {
            let mut s = String::from("&");
            if let Some(lt) = lifetime {
                s.push_str(lt);
                s.push(' ');
            }
            if *is_mutable {
                s.push_str("mut ");
            }
            s.push_str(&render_type(type_, krate));
            s
        }
        RustdocType::Tuple(types) => {
            let inner: Vec<_> = types
                .iter()
                .map(|t| render_type(t, krate))
                .collect();
            format!("({})", inner.join(", "))
        }
        RustdocType::Slice(ty) => format!("[{}]", render_type(ty, krate)),
        RustdocType::Array { type_, len } => {
            format!("[{}; {}]", render_type(type_, krate), len)
        }
        RustdocType::RawPointer { is_mutable, type_ } => {
            let qual = if *is_mutable { "mut" } else { "const" };
            format!("*{} {}", qual, render_type(type_, krate))
        }
        RustdocType::ImplTrait(bounds) => {
            let bs: Vec<_> = bounds
                .iter()
                .map(|b| render_bound(b, krate))
                .collect();
            format!("impl {}", bs.join(" + "))
        }
        RustdocType::DynTrait(dyn_trait) => {
            let ts: Vec<_> = dyn_trait
                .traits
                .iter()
                .map(|pt| render_path(&pt.trait_, krate))
                .collect();
            let mut s = format!("dyn {}", ts.join(" + "));
            if let Some(lt) = &dyn_trait.lifetime {
                s.push_str(" + ");
                s.push_str(lt);
            }
            s
        }
        RustdocType::FunctionPointer(fp) => render_fn_pointer(fp, krate),
        RustdocType::QualifiedPath { name, self_type, trait_, .. } => {
            let self_str = render_type(self_type, krate);
            match trait_ {
                Some(t) => format!("<{} as {}>::{}", self_str, render_path(t, krate), name),
                None => format!("{}::{}", self_str, name),
            }
        }
        RustdocType::Infer => "_".to_string(),
        RustdocType::Pat { type_, .. } => render_type(type_, krate),
    }
}

fn render_path(path: &rustdoc_types::Path, _krate: &RustdocCrate) -> String {
    let mut s = path.path.clone();
    if let Some(args) = &path.args {
        s.push_str(&render_generic_args(args, _krate));
    }
    s
}

fn render_generic_args(args: &GenericArgs, krate: &RustdocCrate) -> String {
    match args {
        GenericArgs::AngleBracketed { args, constraints } => {
            let mut parts = Vec::new();
            for arg in args {
                match arg {
                    GenericArg::Lifetime(lt) => parts.push(lt.clone()),
                    GenericArg::Type(ty) => parts.push(render_type(ty, krate)),
                    GenericArg::Const(c) => parts.push(c.expr.clone()),
                    GenericArg::Infer => parts.push("_".to_string()),
                }
            }
            for constraint in constraints {
                parts.push(format!("{} = ...", constraint.name));
            }
            if parts.is_empty() { String::new() } else { format!("<{}>", parts.join(", ")) }
        }
        GenericArgs::Parenthesized { inputs, output } => {
            let input_str: Vec<_> = inputs
                .iter()
                .map(|t| render_type(t, krate))
                .collect();
            let mut s = format!("({})", input_str.join(", "));
            if let Some(out) = output {
                s.push_str(" -> ");
                s.push_str(&render_type(out, krate));
            }
            s
        }
        GenericArgs::ReturnTypeNotation => "(..)".to_string(),
    }
}

fn render_bound(bound: &GenericBound, krate: &RustdocCrate) -> String {
    match bound {
        GenericBound::TraitBound { trait_, modifier, .. } => {
            let prefix = match modifier {
                TraitBoundModifier::Maybe => "?",
                TraitBoundModifier::MaybeConst => "~const ",
                TraitBoundModifier::None => "",
            };
            format!("{}{}", prefix, render_path(trait_, krate))
        }
        GenericBound::Outlives(lt) => lt.clone(),
        GenericBound::Use(_) => "use<...>".to_string(),
    }
}

fn render_fn_pointer(fp: &FunctionPointer, krate: &RustdocCrate) -> String {
    let mut s = String::from("fn");
    s.push_str(&render_fn_signature(&fp.sig, krate));
    s
}

fn render_fn_signature(sig: &FunctionSignature, krate: &RustdocCrate) -> String {
    let inputs: Vec<_> = sig
        .inputs
        .iter()
        .map(|(name, ty)| format!("{}: {}", name, render_type(ty, krate)))
        .collect();
    let mut s = format!("({})", inputs.join(", "));
    if let Some(output) = &sig.output {
        s.push_str(" -> ");
        s.push_str(&render_type(output, krate));
    }
    s
}

fn render_where_predicate(pred: &WherePredicate, krate: &RustdocCrate) -> String {
    match pred {
        WherePredicate::BoundPredicate { type_, bounds, .. } => {
            let ty = render_type(type_, krate);
            let bs: Vec<_> = bounds
                .iter()
                .map(|b| render_bound(b, krate))
                .collect();
            format!("{}: {}", ty, bs.join(" + "))
        }
        WherePredicate::LifetimePredicate { lifetime, outlives, .. } => {
            if outlives.is_empty() {
                lifetime.clone()
            } else {
                format!("{}: {}", lifetime, outlives.join(" + "))
            }
        }
        WherePredicate::EqPredicate { lhs, rhs } => {
            let lhs_str = render_type(lhs, krate);
            let rhs_str = match rhs {
                rustdoc_types::Term::Type(ty) => render_type(ty, krate),
                rustdoc_types::Term::Constant(c) => c.expr.clone(),
            };
            format!("{} = {}", lhs_str, rhs_str)
        }
    }
}

/// Renders generic params for display in signatures: `<T: Clone, U>`
fn render_generic_params_display(params: &[GenericParamDef], krate: &RustdocCrate) -> String {
    if params.is_empty() {
        return String::new();
    }
    let parts: Vec<_> = params
        .iter()
        .filter_map(|p| {
            match &p.kind {
                GenericParamDefKind::Lifetime { outlives } => {
                    let mut s = p.name.clone();
                    if !outlives.is_empty() {
                        s.push_str(": ");
                        s.push_str(&outlives.join(" + "));
                    }
                    Some(s)
                }
                GenericParamDefKind::Type { bounds, default, is_synthetic } => {
                    // Skip synthetic params (impl Trait in argument position)
                    if *is_synthetic {
                        return None;
                    }
                    let mut s = p.name.clone();
                    if !bounds.is_empty() {
                        s.push_str(": ");
                        s.push_str(
                            &bounds
                                .iter()
                                .map(|b| render_bound(b, krate))
                                .collect::<Vec<_>>()
                                .join(" + "),
                        );
                    }
                    if let Some(def) = default {
                        s.push_str(" = ");
                        s.push_str(&render_type(def, krate));
                    }
                    Some(s)
                }
                GenericParamDefKind::Const { type_, default } => {
                    let mut s = format!("const {}: {}", p.name, render_type(type_, krate));
                    if let Some(def) = default {
                        s.push_str(" = ");
                        s.push_str(def);
                    }
                    Some(s)
                }
            }
        })
        .collect();
    if parts.is_empty() { String::new() } else { format!("<{}>", parts.join(", ")) }
}

// ============================================================
// Helpers
// ============================================================

fn visibility_string(vis: &RustdocVisibility) -> Option<String> {
    match vis {
        RustdocVisibility::Public => Some("public".to_string()),
        RustdocVisibility::Default => Some("private".to_string()),
        RustdocVisibility::Crate => Some("pub(crate)".to_string()),
        RustdocVisibility::Restricted { path, .. } => Some(format!("pub(in {path})")),
    }
}

fn resolve_definition_path(id: &Id, krate: &RustdocCrate) -> Option<String> {
    krate
        .paths
        .get(id)
        .map(|summary| summary.path.join("::"))
}

fn path_rank(path: &str) -> (usize, usize, &str) {
    let segments = path
        .split("::")
        .filter(|segment| !segment.is_empty())
        .count();
    (segments, path.len(), path)
}

fn resolve_use_target_id(start: Id, krate: &RustdocCrate) -> Id {
    let mut current = start;
    let mut seen = HashSet::<Id>::new();

    loop {
        if !seen.insert(current) {
            // Defensively break cycles in malformed data.
            return current;
        }

        let Some(item) = krate.index.get(&current) else {
            return current;
        };
        let ItemEnum::Use(use_item) = &item.inner else {
            return current;
        };
        let Some(next_id) = use_item.id else {
            return current;
        };
        current = next_id;
    }
}

fn build_canonical_path_map(krate: &RustdocCrate) -> HashMap<Id, String> {
    let mut canonical = krate
        .paths
        .iter()
        .map(|(id, summary)| (*id, summary.path.join("::")))
        .collect::<HashMap<Id, String>>();

    for (use_id, item) in &krate.index {
        let ItemEnum::Use(use_item) = &item.inner else {
            continue;
        };
        if !matches!(item.visibility, RustdocVisibility::Public) || use_item.is_glob {
            continue;
        }

        let Some(target_id) = use_item.id else {
            continue;
        };
        let Some(alias_path) = canonical.get(use_id).cloned() else {
            continue;
        };

        let resolved_target = resolve_use_target_id(target_id, krate);
        let entry = canonical
            .entry(resolved_target)
            .or_insert_with(|| alias_path.clone());
        if path_rank(&alias_path) < path_rank(entry.as_str()) {
            *entry = alias_path;
        }
    }

    canonical
}

fn extract_span(span: &Option<Span>) -> (i32, i32) {
    match span {
        Some(span) => {
            let start = span.begin.0.max(1) as i32;
            let end = (span.end.0 as i32).max(start);
            (start, end)
        }
        None => (1, 1),
    }
}

fn has_non_exhaustive(item: &Item) -> bool {
    item.attrs
        .iter()
        .any(|attr| matches!(attr, Attribute::NonExhaustive))
}

fn item_enum_to_kind(inner: &ItemEnum) -> &'static str {
    match inner {
        ItemEnum::Module(_) => "module",
        ItemEnum::ExternCrate { .. } => "extern_crate",
        ItemEnum::Use(_) => "use",
        ItemEnum::Struct(_) => "struct",
        ItemEnum::StructField(_) => "struct_field",
        ItemEnum::Enum(_) => "enum",
        ItemEnum::Variant(_) => "variant",
        ItemEnum::Union(_) => "union",
        ItemEnum::Function(_) => "function",
        ItemEnum::Trait(_) => "trait",
        ItemEnum::TraitAlias(_) => "trait_alias",
        ItemEnum::Impl(_) => "impl",
        ItemEnum::TypeAlias(_) => "type_alias",
        ItemEnum::Constant { .. } => "constant",
        ItemEnum::Static(_) => "static",
        ItemEnum::ExternType => "extern_type",
        ItemEnum::Macro(_) => "macro",
        ItemEnum::ProcMacro(_) => "proc_macro",
        ItemEnum::Primitive(_) => "primitive",
        ItemEnum::AssocConst { .. } => "assoc_const",
        ItemEnum::AssocType { .. } => "assoc_type",
    }
}

/// Extract the terminal/simple name from a Type for the type_name column.
fn type_terminal_name(ty: &RustdocType, krate: &RustdocCrate) -> String {
    match ty {
        RustdocType::ResolvedPath(path) => path
            .path
            .rsplit("::")
            .next()
            .unwrap_or(&path.path)
            .to_string(),
        RustdocType::BorrowedRef { type_, .. } => type_terminal_name(type_, krate),
        RustdocType::Generic(name) => name.clone(),
        RustdocType::Primitive(name) => name.clone(),
        RustdocType::Tuple(_) => "(tuple)".to_string(),
        RustdocType::Slice(inner) => format!("[{}]", type_terminal_name(inner, krate)),
        _ => render_type(ty, krate),
    }
}

/// Build a human-readable signature string from an item.
fn build_signature(item: &Item, krate: &RustdocCrate) -> Option<String> {
    let name = item
        .name
        .as_deref()
        .unwrap_or("_");
    match &item.inner {
        ItemEnum::Function(f) => {
            let params = render_generic_params_display(&f.generics.params, krate);
            let sig = render_fn_signature(&f.sig, krate);
            Some(format!("fn {name}{params}{sig}"))
        }
        ItemEnum::Struct(st) => {
            let params = render_generic_params_display(&st.generics.params, krate);
            Some(format!("struct {name}{params}"))
        }
        ItemEnum::Enum(en) => {
            let params = render_generic_params_display(&en.generics.params, krate);
            Some(format!("enum {name}{params}"))
        }
        ItemEnum::Union(u) => {
            let params = render_generic_params_display(&u.generics.params, krate);
            Some(format!("union {name}{params}"))
        }
        ItemEnum::Trait(t) => {
            let params = render_generic_params_display(&t.generics.params, krate);
            Some(format!("trait {name}{params}"))
        }
        ItemEnum::TypeAlias(ta) => {
            let params = render_generic_params_display(&ta.generics.params, krate);
            Some(format!("type {name}{params} = {}", render_type(&ta.type_, krate)))
        }
        ItemEnum::Constant { type_, .. } => {
            Some(format!("const {name}: {}", render_type(type_, krate)))
        }
        ItemEnum::Static(st) => {
            let m = if st.is_mutable { "mut " } else { "" };
            Some(format!("static {m}{name}: {}", render_type(&st.type_, krate)))
        }
        _ => None,
    }
}

// ============================================================
// JSONB serialization helpers
// ============================================================

fn serialize_generic_params(params: &[GenericParamDef], krate: &RustdocCrate) -> serde_json::Value {
    let rendered: Vec<_> = params
        .iter()
        .map(|p| match &p.kind {
            GenericParamDefKind::Lifetime { outlives } => {
                let mut s = p.name.clone();
                if !outlives.is_empty() {
                    s.push_str(": ");
                    s.push_str(&outlives.join(" + "));
                }
                json!({ "name": p.name, "kind": "lifetime", "rendered": s })
            }
            GenericParamDefKind::Type { bounds, default, .. } => {
                let mut s = p.name.clone();
                if !bounds.is_empty() {
                    s.push_str(": ");
                    s.push_str(
                        &bounds
                            .iter()
                            .map(|b| render_bound(b, krate))
                            .collect::<Vec<_>>()
                            .join(" + "),
                    );
                }
                if let Some(def) = default {
                    s.push_str(" = ");
                    s.push_str(&render_type(def, krate));
                }
                json!({ "name": p.name, "kind": "type", "rendered": s })
            }
            GenericParamDefKind::Const { type_, default } => {
                let mut s = format!("const {}: {}", p.name, render_type(type_, krate));
                if let Some(def) = default {
                    s.push_str(" = ");
                    s.push_str(def);
                }
                json!({ "name": p.name, "kind": "const", "rendered": s })
            }
        })
        .collect();
    json!(rendered)
}

fn serialize_where_predicates(preds: &[WherePredicate], krate: &RustdocCrate) -> serde_json::Value {
    let rendered: Vec<String> = preds
        .iter()
        .map(|p| render_where_predicate(p, krate))
        .collect();
    json!(rendered)
}

// ============================================================
// Field / variant resolution
// ============================================================

fn resolve_fields(field_ids: &[Id], krate: &RustdocCrate) -> serde_json::Value {
    let fields: Vec<_> = field_ids
        .iter()
        .filter_map(|field_id| {
            let field_item = krate.index.get(field_id)?;
            let field_name = field_item.name.clone();
            let field_type = match &field_item.inner {
                ItemEnum::StructField(ty) => render_type(ty, krate),
                _ => return None,
            };
            Some(json!({
                "name": field_name,
                "type": field_type,
                "visibility": visibility_string(&field_item.visibility),
            }))
        })
        .collect();
    json!(fields)
}

fn resolve_tuple_fields(field_ids: &[Option<Id>], krate: &RustdocCrate) -> serde_json::Value {
    let fields: Vec<_> = field_ids
        .iter()
        .enumerate()
        .filter_map(|(idx, opt_id)| {
            let field_id = opt_id.as_ref()?;
            let field_item = krate.index.get(field_id)?;
            let field_type = match &field_item.inner {
                ItemEnum::StructField(ty) => render_type(ty, krate),
                _ => return None,
            };
            Some(json!({
                "name": idx.to_string(),
                "type": field_type,
                "visibility": visibility_string(&field_item.visibility),
            }))
        })
        .collect();
    json!(fields)
}

fn variant_kind_str(kind: &VariantKind) -> &'static str {
    match kind {
        VariantKind::Plain => "plain",
        VariantKind::Tuple(_) => "tuple",
        VariantKind::Struct { .. } => "struct",
    }
}

/// Collect auto-trait names from synthetic impls on a type.
fn collect_auto_traits(impl_ids: &[Id], krate: &RustdocCrate) -> serde_json::Value {
    let mut names = Vec::<String>::new();
    for impl_id in impl_ids {
        let Some(impl_item) = krate.index.get(impl_id) else {
            continue;
        };
        let ItemEnum::Impl(imp) = &impl_item.inner else {
            continue;
        };
        if imp.is_synthetic
            && let Some(trait_path) = &imp.trait_
        {
            let name = trait_path
                .path
                .rsplit("::")
                .next()
                .unwrap_or(&trait_path.path)
                .to_string();
            names.push(name);
        }
    }
    names.sort();
    names.dedup();
    json!(names)
}

// ============================================================
// Symbol extraction
// ============================================================

fn extract_symbols(
    krate: &RustdocCrate,
    canonical_paths: &HashMap<Id, String>,
) -> Vec<IndexedSymbolInsert> {
    let mut symbols = Vec::new();

    for (id, item) in &krate.index {
        let Some(name) = item.name.as_ref() else {
            continue;
        };
        let trimmed = name.trim();
        if trimmed.is_empty() {
            continue;
        }

        let kind = item_enum_to_kind(&item.inner);
        // Skip non-symbol items
        if matches!(kind, "module" | "use" | "extern_crate") {
            continue;
        }

        let (start_line, end_line) = extract_span(&item.span);

        symbols.push(IndexedSymbolInsert {
            name: trimmed.to_string(),
            kind: kind.to_string(),
            visibility: visibility_string(&item.visibility),
            signature: build_signature(item, krate),
            start_line,
            end_line,
            rustdoc_item_id: Some(id.0 as i32),
            canonical_path: canonical_paths
                .get(id)
                .cloned()
                .or_else(|| resolve_definition_path(id, krate)),
            definition_path: resolve_definition_path(id, krate),
            deprecated_since: item
                .deprecation
                .as_ref()
                .and_then(|d| d.since.clone()),
            deprecated_note: item
                .deprecation
                .as_ref()
                .and_then(|d| d.note.clone()),
        });
    }

    symbols
}

// ============================================================
// Type extraction (structs, enums, unions, type aliases)
// ============================================================

fn extract_types(
    krate: &RustdocCrate,
    canonical_paths: &HashMap<Id, String>,
) -> Vec<IndexedTypeInsert> {
    let mut types = Vec::new();

    for (id, item) in &krate.index {
        match &item.inner {
            ItemEnum::Struct(st) => {
                types.push(extract_struct(id, item, st, krate, canonical_paths))
            }
            ItemEnum::Enum(en) => types.push(extract_enum(id, item, en, krate, canonical_paths)),
            ItemEnum::Union(un) => types.push(extract_union(id, item, un, krate, canonical_paths)),
            ItemEnum::TypeAlias(ta) => {
                types.push(extract_type_alias(id, item, ta, krate, canonical_paths))
            }
            _ => {}
        }
    }

    types
}

fn extract_struct(
    id: &Id,
    item: &Item,
    st: &Struct,
    krate: &RustdocCrate,
    canonical_paths: &HashMap<Id, String>,
) -> IndexedTypeInsert {
    let name = item
        .name
        .clone()
        .unwrap_or_default();
    let (start_line, end_line) = extract_span(&item.span);

    let fields = match &st.kind {
        StructKind::Plain { fields, .. } => resolve_fields(fields, krate),
        StructKind::Tuple(field_ids) => resolve_tuple_fields(field_ids, krate),
        StructKind::Unit => json!([]),
    };

    IndexedTypeInsert {
        type_name: name,
        kind: "struct".to_string(),
        visibility: visibility_string(&item.visibility),
        generic_params: serialize_generic_params(&st.generics.params, krate),
        fields,
        variants: json!([]),
        start_line,
        end_line,
        rustdoc_item_id: Some(id.0 as i32),
        canonical_path: canonical_paths
            .get(id)
            .cloned()
            .or_else(|| resolve_definition_path(id, krate)),
        definition_path: resolve_definition_path(id, krate),
        deprecated_since: item
            .deprecation
            .as_ref()
            .and_then(|d| d.since.clone()),
        deprecated_note: item
            .deprecation
            .as_ref()
            .and_then(|d| d.note.clone()),
        is_non_exhaustive: has_non_exhaustive(item),
        auto_traits: collect_auto_traits(&st.impls, krate),
        where_clauses: serialize_where_predicates(&st.generics.where_predicates, krate),
    }
}

fn extract_enum(
    id: &Id,
    item: &Item,
    en: &Enum,
    krate: &RustdocCrate,
    canonical_paths: &HashMap<Id, String>,
) -> IndexedTypeInsert {
    let name = item
        .name
        .clone()
        .unwrap_or_default();
    let (start_line, end_line) = extract_span(&item.span);

    let variants: Vec<_> = en
        .variants
        .iter()
        .filter_map(|variant_id| {
            let variant_item = krate.index.get(variant_id)?;
            let variant_name = variant_item.name.clone()?;
            let ItemEnum::Variant(variant) = &variant_item.inner else {
                return None;
            };

            let variant_fields = match &variant.kind {
                VariantKind::Plain => json!([]),
                VariantKind::Tuple(field_ids) => resolve_tuple_fields(field_ids, krate),
                VariantKind::Struct { fields, .. } => resolve_fields(fields, krate),
            };

            let discriminant = variant
                .discriminant
                .as_ref()
                .map(|d| d.expr.clone());

            Some(json!({
                "name": variant_name,
                "kind": variant_kind_str(&variant.kind),
                "fields": variant_fields,
                "discriminant": discriminant,
            }))
        })
        .collect();

    IndexedTypeInsert {
        type_name: name,
        kind: "enum".to_string(),
        visibility: visibility_string(&item.visibility),
        generic_params: serialize_generic_params(&en.generics.params, krate),
        fields: json!([]),
        variants: json!(variants),
        start_line,
        end_line,
        rustdoc_item_id: Some(id.0 as i32),
        canonical_path: canonical_paths
            .get(id)
            .cloned()
            .or_else(|| resolve_definition_path(id, krate)),
        definition_path: resolve_definition_path(id, krate),
        deprecated_since: item
            .deprecation
            .as_ref()
            .and_then(|d| d.since.clone()),
        deprecated_note: item
            .deprecation
            .as_ref()
            .and_then(|d| d.note.clone()),
        is_non_exhaustive: has_non_exhaustive(item),
        auto_traits: collect_auto_traits(&en.impls, krate),
        where_clauses: serialize_where_predicates(&en.generics.where_predicates, krate),
    }
}

fn extract_union(
    id: &Id,
    item: &Item,
    un: &Union,
    krate: &RustdocCrate,
    canonical_paths: &HashMap<Id, String>,
) -> IndexedTypeInsert {
    let name = item
        .name
        .clone()
        .unwrap_or_default();
    let (start_line, end_line) = extract_span(&item.span);

    IndexedTypeInsert {
        type_name: name,
        kind: "union".to_string(),
        visibility: visibility_string(&item.visibility),
        generic_params: serialize_generic_params(&un.generics.params, krate),
        fields: resolve_fields(&un.fields, krate),
        variants: json!([]),
        start_line,
        end_line,
        rustdoc_item_id: Some(id.0 as i32),
        canonical_path: canonical_paths
            .get(id)
            .cloned()
            .or_else(|| resolve_definition_path(id, krate)),
        definition_path: resolve_definition_path(id, krate),
        deprecated_since: item
            .deprecation
            .as_ref()
            .and_then(|d| d.since.clone()),
        deprecated_note: item
            .deprecation
            .as_ref()
            .and_then(|d| d.note.clone()),
        is_non_exhaustive: has_non_exhaustive(item),
        auto_traits: collect_auto_traits(&un.impls, krate),
        where_clauses: serialize_where_predicates(&un.generics.where_predicates, krate),
    }
}

fn extract_type_alias(
    id: &Id,
    item: &Item,
    ta: &TypeAlias,
    krate: &RustdocCrate,
    canonical_paths: &HashMap<Id, String>,
) -> IndexedTypeInsert {
    let name = item
        .name
        .clone()
        .unwrap_or_default();
    let (start_line, end_line) = extract_span(&item.span);

    IndexedTypeInsert {
        type_name: name,
        kind: "type_alias".to_string(),
        visibility: visibility_string(&item.visibility),
        generic_params: serialize_generic_params(&ta.generics.params, krate),
        fields: json!([]),
        variants: json!([]),
        start_line,
        end_line,
        rustdoc_item_id: Some(id.0 as i32),
        canonical_path: canonical_paths
            .get(id)
            .cloned()
            .or_else(|| resolve_definition_path(id, krate)),
        definition_path: resolve_definition_path(id, krate),
        deprecated_since: item
            .deprecation
            .as_ref()
            .and_then(|d| d.since.clone()),
        deprecated_note: item
            .deprecation
            .as_ref()
            .and_then(|d| d.note.clone()),
        is_non_exhaustive: false,
        auto_traits: json!([]),
        where_clauses: serialize_where_predicates(&ta.generics.where_predicates, krate),
    }
}

// ============================================================
// Impl extraction
// ============================================================

fn extract_impls(krate: &RustdocCrate) -> Vec<IndexedImplInsert> {
    let mut impls = Vec::new();

    for (id, item) in &krate.index {
        let ItemEnum::Impl(imp) = &item.inner else {
            continue;
        };

        let type_name = type_terminal_name(&imp.for_, krate);
        let type_name_display = Some(render_type(&imp.for_, krate));

        let (trait_name, trait_name_display) = match &imp.trait_ {
            Some(trait_path) => {
                let terminal = trait_path
                    .path
                    .rsplit("::")
                    .next()
                    .unwrap_or(&trait_path.path)
                    .to_string();
                (Some(terminal), Some(render_path(trait_path, krate)))
            }
            None => (None, None),
        };

        let impl_kind = classify_impl_kind(imp, item);
        let methods = extract_impl_methods(&imp.items, krate);
        let (start_line, end_line) = extract_span(&item.span);

        let blanket_type = imp
            .blanket_impl
            .as_ref()
            .map(|ty| render_type(ty, krate));

        impls.push(IndexedImplInsert {
            type_name,
            type_name_display,
            trait_name,
            trait_name_display,
            impl_kind,
            methods,
            start_line,
            end_line,
            rustdoc_item_id: Some(id.0 as i32),
            is_blanket: imp.blanket_impl.is_some(),
            is_synthetic: imp.is_synthetic,
            is_negative: imp.is_negative,
            blanket_type,
            generics: serialize_generic_params(&imp.generics.params, krate),
            where_clauses: serialize_where_predicates(&imp.generics.where_predicates, krate),
        });
    }

    impls
}

fn classify_impl_kind(imp: &Impl, item: &Item) -> String {
    if imp.trait_.is_none() {
        return "inherent".to_string();
    }
    if imp.is_synthetic {
        return "synthetic".to_string();
    }
    if imp.blanket_impl.is_some() {
        return "blanket".to_string();
    }
    if item
        .attrs
        .iter()
        .any(|a| matches!(a, Attribute::AutomaticallyDerived))
    {
        return "derive".to_string();
    }
    "trait".to_string()
}

fn extract_impl_methods(item_ids: &[Id], krate: &RustdocCrate) -> serde_json::Value {
    let methods: Vec<_> = item_ids
        .iter()
        .filter_map(|method_id| {
            let method_item = krate.index.get(method_id)?;
            let ItemEnum::Function(func) = &method_item.inner else {
                return None;
            };
            let method_name = method_item.name.clone()?;

            let has_self = func
                .sig
                .inputs
                .first()
                .is_some_and(|(name, _)| name == "self");

            let return_type = func
                .sig
                .output
                .as_ref()
                .map(|ty| render_type(ty, krate));

            let params: Vec<_> = func
                .sig
                .inputs
                .iter()
                .map(|(name, ty)| json!({ "name": name, "type": render_type(ty, krate) }))
                .collect();

            Some(json!({
                "name": method_name,
                "signature": format!("fn {}{}", method_name, render_fn_signature(&func.sig, krate)),
                "has_self": has_self,
                "is_const": func.header.is_const,
                "is_async": func.header.is_async,
                "is_unsafe": func.header.is_unsafe,
                "params": params,
                "return_type": return_type,
                "visibility": visibility_string(&method_item.visibility),
            }))
        })
        .collect();
    json!(methods)
}

// ============================================================
// Trait extraction
// ============================================================

fn extract_traits(krate: &RustdocCrate) -> Vec<IndexedTraitInsert> {
    let mut traits = Vec::new();

    for (id, item) in &krate.index {
        let ItemEnum::Trait(trait_def) = &item.inner else {
            continue;
        };
        let Some(name) = item.name.as_ref() else {
            continue;
        };

        let supertraits: Vec<String> = trait_def
            .bounds
            .iter()
            .map(|b| render_bound(b, krate))
            .collect();

        let (required, provided, assoc_types) = partition_trait_items(&trait_def.items, krate);

        traits.push(IndexedTraitInsert {
            trait_name: name.clone(),
            is_auto: trait_def.is_auto,
            is_unsafe: trait_def.is_unsafe,
            is_dyn_compatible: trait_def.is_dyn_compatible,
            supertraits: json!(supertraits),
            required_methods: json!(required),
            provided_methods: json!(provided),
            associated_types: json!(assoc_types),
            generics: serialize_generic_params(&trait_def.generics.params, krate),
            rustdoc_item_id: Some(id.0 as i32),
        });
    }

    traits
}

fn partition_trait_items(
    item_ids: &[Id],
    krate: &RustdocCrate,
) -> (Vec<serde_json::Value>, Vec<serde_json::Value>, Vec<serde_json::Value>) {
    let mut required = Vec::new();
    let mut provided = Vec::new();
    let mut assoc_types = Vec::new();

    for item_id in item_ids {
        let Some(item) = krate.index.get(item_id) else {
            continue;
        };
        let Some(name) = item.name.as_ref() else {
            continue;
        };

        match &item.inner {
            ItemEnum::Function(func) => {
                let method = json!({
                    "name": name,
                    "signature": format!("fn {}{}", name, render_fn_signature(&func.sig, krate)),
                    "is_unsafe": func.header.is_unsafe,
                    "is_const": func.header.is_const,
                    "is_async": func.header.is_async,
                });

                if func.has_body {
                    provided.push(method);
                } else {
                    required.push(method);
                }
            }
            ItemEnum::AssocType { bounds, type_, .. } => {
                let bounds_rendered: Vec<String> = bounds
                    .iter()
                    .map(|b| render_bound(b, krate))
                    .collect();
                let default = type_
                    .as_ref()
                    .map(|ty| render_type(ty, krate));

                assoc_types.push(json!({
                    "name": name,
                    "bounds": bounds_rendered,
                    "default": default,
                }));
            }
            ItemEnum::AssocConst { type_, value } => {
                required.push(json!({
                    "name": name,
                    "signature": format!("const {}: {}", name, render_type(type_, krate)),
                    "kind": "assoc_const",
                    "value": value,
                }));
            }
            _ => {}
        }
    }

    (required, provided, assoc_types)
}

// ============================================================
// Top-level extraction orchestrator
// ============================================================

fn extract_all(krate: &RustdocCrate) -> IndexedExtractionBatch {
    let canonical_paths = build_canonical_path_map(krate);
    IndexedExtractionBatch {
        symbols: extract_symbols(krate, &canonical_paths),
        types: extract_types(krate, &canonical_paths),
        impls: extract_impls(krate),
        traits: extract_traits(krate),
    }
}

// ============================================================
// Database sync
// ============================================================

impl McpServer {
    async fn ingest_rustdoc_json_document(
        &self,
        candidate: &RustdocSyncCandidateRow,
        source_path: &str,
        content: &str,
        outcome: &mut RustdocJsonRefreshOutcome,
    ) -> Result<(), String> {
        let krate = serde_json::from_str::<RustdocCrate>(content)
            .map_err(|error| format_rustdoc_parse_error(candidate, source_path, content, &error))?;

        if krate.format_version != rustdoc_types::FORMAT_VERSION {
            return Err(format!(
                "failed to parse rustdoc JSON payload for {}@{} from {}: payload \
                 format_version={} is not supported (expected={})",
                candidate.crate_name,
                candidate.version,
                source_path,
                krate.format_version,
                rustdoc_types::FORMAT_VERSION,
            ));
        }

        let resolved_crate_name = krate
            .index
            .get(&krate.root)
            .and_then(|item| item.name.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.replace('_', "-"))
            .unwrap_or_else(|| candidate.crate_name.clone());

        if resolved_crate_name != candidate.crate_name {
            return Err(format!(
                "rustdoc JSON crate mismatch for {}@{} from {}: payload resolved to crate '{}'",
                candidate.crate_name, candidate.version, source_path, resolved_crate_name
            ));
        }

        let content_bytes = content.as_bytes();
        upsert_source_file_unconditional(
            &self.state.db,
            candidate.crate_version_id,
            source_path,
            &file_sha256_hex(content_bytes),
            content_bytes.len() as i64,
            Some("rustdoc_json"),
            content,
        )
        .await
        .map_err(|e| {
            format!(
                "failed to upsert rustdoc source file {} for {}@{}: {e}",
                source_path, candidate.crate_name, candidate.version
            )
        })?;

        let source_file_id =
            fetch_source_file_id_required(&self.state.db, candidate.crate_version_id, source_path)
                .await
                .map_err(|e| {
                    format!(
                        "failed to lookup rustdoc source file id {} for {}@{}: {e}",
                        source_path, candidate.crate_name, candidate.version
                    )
                })?;

        let extraction = extract_all(&krate);

        let write_counts = replace_crate_version_index_rows(
            &self.state.db,
            candidate.crate_version_id,
            source_file_id,
            "rustdoc_json",
            &extraction,
        )
        .await
        .map_err(|e| {
            format!(
                "failed to replace rustdoc extraction rows for {}@{}: {e}",
                candidate.crate_name, candidate.version
            )
        })?;

        outcome.symbols_written += write_counts.symbols;
        outcome.types_written += write_counts.types;
        outcome.impls_written += write_counts.impls;
        outcome.traits_written += write_counts.traits;

        outcome.synced_versions += 1;
        outcome
            .touched_versions
            .push(format!("{}@{}", candidate.crate_name, candidate.version));
        Ok(())
    }

    pub(crate) async fn sync_rustdoc_json_cache(
        &self,
        crate_name: Option<String>,
        page: Option<u32>,
        per_page: Option<u32>,
    ) -> Result<RustdocJsonRefreshOutcome, String> {
        let crate_filter = match crate_name {
            Some(value) => Some(normalize_required(value, "crate_name")?),
            None => None,
        };
        let page = sync_page(page);
        let per_page = sync_per_page(per_page);
        let offset = page
            .saturating_sub(1)
            .saturating_mul(per_page);

        let candidates = fetch_rustdoc_sync_candidates(
            &self.state.db,
            crate_filter.as_deref(),
            i64::from(per_page),
            i64::from(offset),
        )
        .await
        .map_err(|e| format!("rustdoc JSON sync failed to load crate versions: {e}"))?;

        let (local_fallback, local_fallback_unavailable) = match self
            .state
            .config
            .rustdoc_json_dir
            .clone()
        {
            None => (
                None,
                Some(
                    "local rustdoc fallback unavailable: RUSTDOC_JSON_DIR is not configured"
                        .to_string(),
                ),
            ),
            Some(root_dir) if !root_dir.exists() => (
                None,
                Some(format!(
                    "local rustdoc fallback unavailable: rustdoc JSON directory not found: {}",
                    root_dir.display()
                )),
            ),
            Some(root_dir) => match walk_json_files(&root_dir) {
                Ok(files) => {
                    let mut local_candidates = files
                        .into_iter()
                        .filter_map(|path| {
                            let stem = path
                                .file_stem()?
                                .to_string_lossy()
                                .to_string();
                            let (candidate_crate, candidate_version) = crate_from_stem(&stem);
                            if candidate_crate.is_empty() {
                                return None;
                            }
                            if let Some(filter) = crate_filter.as_ref()
                                && candidate_crate != *filter
                            {
                                return None;
                            }
                            Some(RustdocCandidate {
                                path,
                                crate_name: candidate_crate,
                                crate_version: candidate_version,
                            })
                        })
                        .collect::<Vec<_>>();
                    local_candidates.sort_by(|left, right| left.path.cmp(&right.path));
                    (Some((root_dir, local_candidates)), None)
                }
                Err(error) => (None, Some(format!("local rustdoc fallback unavailable: {error}"))),
            },
        };

        let mut outcome = RustdocJsonRefreshOutcome::default();
        let docs_rs = DocsRsClient::new(&self.state);

        for candidate in candidates {
            outcome.scanned_files += 1;

            let docs_source_path =
                docs_rs_rustdoc_synthetic_path(&candidate.crate_name, &candidate.version);
            let mut source_errors = Vec::new();

            match docs_rs
                .fetch_rustdoc_json(&candidate.crate_name, &candidate.version)
                .await
            {
                Ok(payload_bytes) => match decode_docs_rs_rustdoc_payload(payload_bytes) {
                    Ok(payload) => {
                        match self
                            .ingest_rustdoc_json_document(
                                &candidate,
                                &docs_source_path,
                                &payload,
                                &mut outcome,
                            )
                            .await
                        {
                            Ok(()) => continue,
                            Err(error) => source_errors.push(error),
                        }
                    }
                    Err(error) => source_errors.push(format!(
                        "docs.rs rustdoc JSON payload for {}@{} could not be decoded: {error}",
                        candidate.crate_name, candidate.version
                    )),
                },
                Err(error) => source_errors.push(error),
            }

            let mut local_ingested = false;
            if let Some((root_dir, local_candidates)) = local_fallback.as_ref() {
                let local_candidate = local_candidates
                    .iter()
                    .find(|local| {
                        local.crate_name == candidate.crate_name
                            && local.crate_version.as_deref() == Some(candidate.version.as_str())
                    })
                    .or_else(|| {
                        local_candidates
                            .iter()
                            .find(|local| {
                                local.crate_name == candidate.crate_name
                                    && local.crate_version.is_none()
                            })
                    });

                if let Some(local_candidate) = local_candidate {
                    let local_source_path = synthetic_rustdoc_path(root_dir, &local_candidate.path);

                    match std::fs::read(&local_candidate.path) {
                        Ok(payload_bytes) => match String::from_utf8(payload_bytes) {
                            Ok(payload) => {
                                match self
                                    .ingest_rustdoc_json_document(
                                        &candidate,
                                        &local_source_path,
                                        &payload,
                                        &mut outcome,
                                    )
                                    .await
                                {
                                    Ok(()) => {
                                        local_ingested = true;
                                    }
                                    Err(error) => source_errors.push(error),
                                }
                            }
                            Err(error) => source_errors.push(format!(
                                "invalid UTF-8 in rustdoc JSON file {} (local fallback): {error}",
                                local_candidate.path.display()
                            )),
                        },
                        Err(error) => source_errors.push(format!(
                            "failed to read local rustdoc JSON file {}: {error}",
                            local_candidate.path.display()
                        )),
                    }
                } else {
                    source_errors.push(format!(
                        "no local rustdoc JSON file found for {}@{}",
                        candidate.crate_name, candidate.version
                    ));
                }
            } else if let Some(reason) = local_fallback_unavailable.as_ref() {
                source_errors.push(reason.clone());
            }

            if !local_ingested {
                outcome
                    .errors
                    .push(format_candidate_sync_failure(
                        &candidate.crate_name,
                        &candidate.version,
                        &source_errors,
                    ));
            }
        }

        outcome
            .touched_versions
            .sort();
        outcome
            .touched_versions
            .dedup();
        Ok(outcome)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use rustdoc_types::{
        Abi, Deprecation, Function, FunctionHeader, FunctionSignature, Generics, Id, Impl, Item,
        ItemEnum, ItemKind, ItemSummary, Module, Path, Span, Struct, StructKind, Target, Trait,
        Type as RustdocType, Use, Visibility as RustdocVisibility,
    };

    use super::*;

    // ---- Fixture builder helpers ----

    fn item(id: u32, name: &str, vis: RustdocVisibility, inner: ItemEnum) -> Item {
        Item {
            id: Id(id),
            crate_id: 0,
            name: Some(name.to_string()),
            span: Some(Span {
                filename: PathBuf::from("src/lib.rs"),
                begin: (1, 1),
                end: (10, 1),
            }),
            visibility: vis,
            docs: None,
            links: HashMap::new(),
            attrs: vec![],
            deprecation: None,
            inner,
        }
    }

    fn empty_generics() -> Generics {
        Generics {
            params: vec![],
            where_predicates: vec![],
        }
    }

    fn build_fixture_crate() -> RustdocCrate {
        let mut index = HashMap::new();
        let mut paths = HashMap::new();

        // Root module (id=0) contains all top-level items
        index.insert(
            Id(0),
            item(
                0,
                "my_crate",
                RustdocVisibility::Public,
                ItemEnum::Module(Module {
                    is_crate: true,
                    items: vec![Id(1), Id(2), Id(3), Id(4), Id(5)],
                    is_stripped: false,
                }),
            ),
        );
        paths.insert(
            Id(0),
            ItemSummary {
                crate_id: 0,
                path: vec!["my_crate".into()],
                kind: ItemKind::Module,
            },
        );

        // id=1: pub struct Foo { pub bar: i32 }
        index.insert(
            Id(1),
            item(
                1,
                "Foo",
                RustdocVisibility::Public,
                ItemEnum::Struct(Struct {
                    kind: StructKind::Plain {
                        fields: vec![Id(10)],
                        has_stripped_fields: false,
                    },
                    generics: empty_generics(),
                    impls: vec![Id(3)],
                }),
            ),
        );
        paths.insert(
            Id(1),
            ItemSummary {
                crate_id: 0,
                path: vec!["my_crate".into(), "Foo".into()],
                kind: ItemKind::Struct,
            },
        );

        // id=10: field `bar: i32`
        index.insert(
            Id(10),
            item(
                10,
                "bar",
                RustdocVisibility::Public,
                ItemEnum::StructField(RustdocType::Primitive(String::from("i32"))),
            ),
        );

        // id=2: pub fn helper() -> bool
        index.insert(
            Id(2),
            item(
                2,
                "helper",
                RustdocVisibility::Public,
                ItemEnum::Function(Function {
                    sig: FunctionSignature {
                        inputs: vec![],
                        output: Some(RustdocType::Primitive("bool".into())),
                        is_c_variadic: false,
                    },
                    generics: empty_generics(),
                    header: FunctionHeader {
                        is_const: false,
                        is_unsafe: false,
                        is_async: false,
                        abi: Abi::Rust,
                    },
                    has_body: true,
                }),
            ),
        );
        paths.insert(
            Id(2),
            ItemSummary {
                crate_id: 0,
                path: vec!["my_crate".into(), "helper".into()],
                kind: ItemKind::Function,
            },
        );

        // id=3: impl Foo { pub fn new() -> Foo }
        let mut impl_item = item(
            3,
            "",
            RustdocVisibility::Default,
            ItemEnum::Impl(Impl {
                is_unsafe: false,
                generics: empty_generics(),
                provided_trait_methods: vec![],
                trait_: None,
                for_: RustdocType::ResolvedPath(Path {
                    path: "Foo".into(),
                    id: Id(1),
                    args: None,
                }),
                items: vec![Id(11)],
                is_negative: false,
                is_synthetic: false,
                blanket_impl: None,
            }),
        );
        impl_item.name = None; // impls typically have no name
        index.insert(Id(3), impl_item);

        // id=11: fn new() -> Foo (method inside impl)
        index.insert(
            Id(11),
            item(
                11,
                "new",
                RustdocVisibility::Public,
                ItemEnum::Function(Function {
                    sig: FunctionSignature {
                        inputs: vec![],
                        output: Some(RustdocType::ResolvedPath(Path {
                            path: "Foo".into(),
                            id: Id(1),
                            args: None,
                        })),
                        is_c_variadic: false,
                    },
                    generics: empty_generics(),
                    header: FunctionHeader {
                        is_const: false,
                        is_unsafe: false,
                        is_async: false,
                        abi: Abi::Rust,
                    },
                    has_body: true,
                }),
            ),
        );

        // id=4: pub trait MyTrait { fn required(&self); fn provided(&self) {} }
        index.insert(
            Id(4),
            item(
                4,
                "MyTrait",
                RustdocVisibility::Public,
                ItemEnum::Trait(Trait {
                    is_auto: false,
                    is_unsafe: false,
                    is_dyn_compatible: true,
                    items: vec![Id(12), Id(13)],
                    generics: empty_generics(),
                    bounds: vec![],
                    implementations: vec![Id(5)],
                }),
            ),
        );
        paths.insert(
            Id(4),
            ItemSummary {
                crate_id: 0,
                path: vec!["my_crate".into(), "MyTrait".into()],
                kind: ItemKind::Trait,
            },
        );

        // id=12: fn required(&self) — no body
        index.insert(
            Id(12),
            item(
                12,
                "required",
                RustdocVisibility::Public,
                ItemEnum::Function(Function {
                    sig: FunctionSignature {
                        inputs: vec![("self".into(), RustdocType::Generic("Self".into()))],
                        output: None,
                        is_c_variadic: false,
                    },
                    generics: empty_generics(),
                    header: FunctionHeader {
                        is_const: false,
                        is_unsafe: false,
                        is_async: false,
                        abi: Abi::Rust,
                    },
                    has_body: false,
                }),
            ),
        );

        // id=13: fn provided(&self) — has body
        index.insert(
            Id(13),
            item(
                13,
                "provided",
                RustdocVisibility::Public,
                ItemEnum::Function(Function {
                    sig: FunctionSignature {
                        inputs: vec![("self".into(), RustdocType::Generic("Self".into()))],
                        output: None,
                        is_c_variadic: false,
                    },
                    generics: empty_generics(),
                    header: FunctionHeader {
                        is_const: false,
                        is_unsafe: false,
                        is_async: false,
                        abi: Abi::Rust,
                    },
                    has_body: true,
                }),
            ),
        );

        // id=5: impl MyTrait for Foo
        let mut trait_impl_item = item(
            5,
            "",
            RustdocVisibility::Default,
            ItemEnum::Impl(Impl {
                is_unsafe: false,
                generics: empty_generics(),
                provided_trait_methods: vec!["provided".into()],
                trait_: Some(Path {
                    path: "MyTrait".into(),
                    id: Id(4),
                    args: None,
                }),
                for_: RustdocType::ResolvedPath(Path {
                    path: "Foo".into(),
                    id: Id(1),
                    args: None,
                }),
                items: vec![Id(14)],
                is_negative: false,
                is_synthetic: false,
                blanket_impl: None,
            }),
        );
        trait_impl_item.name = None;
        index.insert(Id(5), trait_impl_item);

        // id=14: fn required(&self) inside the trait impl
        index.insert(
            Id(14),
            item(
                14,
                "required",
                RustdocVisibility::Public,
                ItemEnum::Function(Function {
                    sig: FunctionSignature {
                        inputs: vec![("self".into(), RustdocType::Generic("Self".into()))],
                        output: None,
                        is_c_variadic: false,
                    },
                    generics: empty_generics(),
                    header: FunctionHeader {
                        is_const: false,
                        is_unsafe: false,
                        is_async: false,
                        abi: Abi::Rust,
                    },
                    has_body: true,
                }),
            ),
        );

        RustdocCrate {
            root: Id(0),
            crate_version: Some("1.0.0".into()),
            includes_private: false,
            index,
            paths,
            external_crates: HashMap::new(),
            target: Target {
                triple: "x86_64-unknown-linux-gnu".into(),
                target_features: vec![],
            },
            format_version: 57,
        }
    }

    #[test]
    fn extract_all_produces_symbols_types_impls_traits() {
        let krate = build_fixture_crate();
        let result = extract_all(&krate);

        // Symbols: all named public items from the root module
        assert!(!result.symbols.is_empty(), "should extract at least one symbol");
        let sym_names: Vec<&str> = result
            .symbols
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        assert!(sym_names.contains(&"Foo"), "should contain struct Foo");
        assert!(sym_names.contains(&"helper"), "should contain function helper");
        assert!(sym_names.contains(&"MyTrait"), "should contain trait MyTrait");

        // Types: the struct Foo
        assert!(!result.types.is_empty(), "should extract at least one type");
        let type_names: Vec<&str> = result
            .types
            .iter()
            .map(|t| t.type_name.as_str())
            .collect();
        assert!(type_names.contains(&"Foo"), "should contain type Foo");
        let foo_type = result
            .types
            .iter()
            .find(|t| t.type_name == "Foo")
            .unwrap();
        assert_eq!(foo_type.kind, "struct");
        assert_eq!(foo_type.rustdoc_item_id, Some(1));
        // Check that fields are populated
        let fields = foo_type
            .fields
            .as_array()
            .expect("fields should be array");
        assert_eq!(fields.len(), 1, "Foo should have one field");
        assert_eq!(fields[0]["name"], "bar");

        // Impls: inherent impl for Foo + trait impl MyTrait for Foo
        assert!(
            result.impls.len() >= 2,
            "should have at least 2 impls (inherent + trait), got {}",
            result.impls.len()
        );
        let inherent = result
            .impls
            .iter()
            .find(|i| i.type_name == "Foo" && i.trait_name.is_none());
        assert!(inherent.is_some(), "should have inherent impl for Foo");
        let inherent = inherent.unwrap();
        assert_eq!(inherent.impl_kind, "inherent");
        let methods = inherent
            .methods
            .as_array()
            .expect("methods should be array");
        assert!(
            methods
                .iter()
                .any(|m| m["name"] == "new"),
            "inherent impl should contain method 'new'"
        );

        let trait_impl = result
            .impls
            .iter()
            .find(|i| i.trait_name.as_deref() == Some("MyTrait"));
        assert!(trait_impl.is_some(), "should have trait impl MyTrait for Foo");
        let trait_impl = trait_impl.unwrap();
        assert_eq!(trait_impl.type_name, "Foo");
        assert_eq!(trait_impl.impl_kind, "trait");

        // Traits: MyTrait
        assert!(!result.traits.is_empty(), "should extract at least one trait");
        let my_trait = result
            .traits
            .iter()
            .find(|t| t.trait_name == "MyTrait")
            .expect("should contain MyTrait");
        assert!(!my_trait.is_auto);
        assert!(!my_trait.is_unsafe);
        assert!(my_trait.is_dyn_compatible);
        assert_eq!(my_trait.rustdoc_item_id, Some(4));
        // Check required vs provided partitioning
        let required = my_trait
            .required_methods
            .as_array()
            .expect("required_methods should be array");
        let provided = my_trait
            .provided_methods
            .as_array()
            .expect("provided_methods should be array");
        assert!(
            required
                .iter()
                .any(|m| m["name"] == "required"),
            "required_methods should contain 'required'"
        );
        assert!(
            provided
                .iter()
                .any(|m| m["name"] == "provided"),
            "provided_methods should contain 'provided'"
        );
    }

    #[test]
    fn extract_all_prefers_shortest_public_reexport_path_as_canonical() {
        let mut index = HashMap::new();
        let mut paths = HashMap::new();

        index.insert(
            Id(0),
            item(
                0,
                "my_crate",
                RustdocVisibility::Public,
                ItemEnum::Module(Module {
                    is_crate: true,
                    items: vec![Id(10), Id(2)],
                    is_stripped: false,
                }),
            ),
        );
        paths.insert(
            Id(0),
            ItemSummary {
                crate_id: 0,
                path: vec!["my_crate".into()],
                kind: ItemKind::Module,
            },
        );

        index.insert(
            Id(10),
            item(
                10,
                "internal",
                RustdocVisibility::Public,
                ItemEnum::Module(Module {
                    is_crate: false,
                    items: vec![Id(1)],
                    is_stripped: false,
                }),
            ),
        );
        paths.insert(
            Id(10),
            ItemSummary {
                crate_id: 0,
                path: vec!["my_crate".into(), "internal".into()],
                kind: ItemKind::Module,
            },
        );

        index.insert(
            Id(1),
            item(
                1,
                "InnerError",
                RustdocVisibility::Public,
                ItemEnum::Struct(Struct {
                    kind: StructKind::Unit,
                    generics: empty_generics(),
                    impls: vec![],
                }),
            ),
        );
        paths.insert(
            Id(1),
            ItemSummary {
                crate_id: 0,
                path: vec!["my_crate".into(), "internal".into(), "InnerError".into()],
                kind: ItemKind::Struct,
            },
        );

        index.insert(
            Id(2),
            item(
                2,
                "Error",
                RustdocVisibility::Public,
                ItemEnum::Use(Use {
                    source: "crate::internal::InnerError".into(),
                    name: "Error".into(),
                    id: Some(Id(1)),
                    is_glob: false,
                }),
            ),
        );
        paths.insert(
            Id(2),
            ItemSummary {
                crate_id: 0,
                path: vec!["my_crate".into(), "Error".into()],
                kind: ItemKind::Use,
            },
        );

        let krate = RustdocCrate {
            root: Id(0),
            crate_version: Some("1.0.0".into()),
            includes_private: false,
            index,
            paths,
            external_crates: HashMap::new(),
            target: Target {
                triple: "x86_64-unknown-linux-gnu".into(),
                target_features: vec![],
            },
            format_version: 57,
        };

        let result = extract_all(&krate);
        let symbol = result
            .symbols
            .iter()
            .find(|symbol| symbol.name == "InnerError")
            .expect("expected rustdoc symbol for re-exported type");
        assert_eq!(
            symbol
                .definition_path
                .as_deref(),
            Some("my_crate::internal::InnerError")
        );
        assert_eq!(
            symbol
                .canonical_path
                .as_deref(),
            Some("my_crate::Error")
        );
    }

    #[test]
    fn extract_all_handles_deprecated_items() {
        let mut krate = build_fixture_crate();
        // Add deprecation to struct Foo
        if let Some(foo) = krate.index.get_mut(&Id(1)) {
            foo.deprecation = Some(Deprecation {
                since: Some("2.0.0".into()),
                note: Some("use Bar instead".into()),
            });
        }

        let result = extract_all(&krate);
        let foo_sym = result
            .symbols
            .iter()
            .find(|s| s.name == "Foo")
            .expect("Foo symbol");
        assert_eq!(
            foo_sym
                .deprecated_since
                .as_deref(),
            Some("2.0.0")
        );
        assert_eq!(
            foo_sym
                .deprecated_note
                .as_deref(),
            Some("use Bar instead")
        );

        let foo_type = result
            .types
            .iter()
            .find(|t| t.type_name == "Foo")
            .expect("Foo type");
        assert_eq!(
            foo_type
                .deprecated_since
                .as_deref(),
            Some("2.0.0")
        );
    }

    #[test]
    fn extract_all_idempotent() {
        let krate = build_fixture_crate();
        let first = extract_all(&krate);
        let second = extract_all(&krate);
        assert_eq!(first.symbols.len(), second.symbols.len());
        assert_eq!(first.types.len(), second.types.len());
        assert_eq!(first.impls.len(), second.impls.len());
        assert_eq!(first.traits.len(), second.traits.len());
    }

    #[test]
    fn render_type_handles_primitive() {
        let krate = build_fixture_crate();
        let rendered = render_type(&RustdocType::Primitive("u64".into()), &krate);
        assert_eq!(rendered, "u64");
    }

    #[test]
    fn render_type_handles_resolved_path() {
        let krate = build_fixture_crate();
        let rendered = render_type(
            &RustdocType::ResolvedPath(Path {
                path: "Vec".into(),
                id: Id(999),
                args: None,
            }),
            &krate,
        );
        assert_eq!(rendered, "Vec");
    }

    #[test]
    fn visibility_string_maps_correctly() {
        assert_eq!(visibility_string(&RustdocVisibility::Public), Some("public".to_string()));
        assert_eq!(visibility_string(&RustdocVisibility::Default), Some("private".to_string()));
        assert_eq!(visibility_string(&RustdocVisibility::Crate), Some("pub(crate)".to_string()));
    }
}
