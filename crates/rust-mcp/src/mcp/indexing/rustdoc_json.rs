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
    fetch_rustdoc_sync_candidates, mark_version_rustdoc_attempted, mark_version_rustdoc_enriched,
    replace_crate_version_index_rows, upsert_source_file_unconditional,
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

/// Outcome of a rustdoc JSON sync operation.
#[derive(Debug, Default)]
pub struct RustdocJsonRefreshOutcome {
    /// Number of rustdoc JSON files that were processed.
    pub scanned_files: usize,
    /// Number of crate versions whose documentation was synced.
    pub synced_versions: usize,
    /// Number of symbol entries written to the database.
    pub symbols_written: usize,
    /// Number of type entries written to the database.
    pub types_written: usize,
    /// Number of trait implementation entries written to the database.
    pub impls_written: usize,
    /// Number of trait definition entries written to the database.
    pub traits_written: usize,
    /// List of `crate@version` strings that were touched during the sync.
    pub touched_versions: Vec<String>,
    /// Errors encountered during the sync, one per failed operation.
    pub errors: Vec<String>,
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

/// Returns `true` when the filename ends in `.json` or `.json.zst`.
fn is_rustdoc_json_file(path: &Path) -> bool {
    let name = match path
        .file_name()
        .and_then(OsStr::to_str)
    {
        Some(n) => n.to_ascii_lowercase(),
        None => return false,
    };
    name.ends_with(".json") || name.ends_with(".json.zst")
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
            } else if file_type.is_file() && is_rustdoc_json_file(&path) {
                files.push(path);
            }
        }
    }

    files.sort();
    Ok(files)
}

/// Extract the logical stem from a rustdoc JSON filename, stripping `.json`
/// and `.json.zst` suffixes so that the caller always gets the bare name.
fn rustdoc_logical_stem(path: &Path) -> Option<String> {
    let name = path
        .file_name()?
        .to_string_lossy();
    name.strip_suffix(".json.zst")
        .or_else(|| name.strip_suffix(".json"))
        .map(|base| base.to_string())
}

fn crate_from_stem(stem: &str) -> (String, Option<String>) {
    let normalized = stem.trim();
    if normalized.is_empty() {
        return (String::new(), None);
    }

    // Strategy 1: split on `-` and join the tail (handles pre-release versions
    // like `my-crate-1.0.0-alpha.1`).
    let dash_segments = normalized
        .split('-')
        .collect::<Vec<_>>();
    if dash_segments.len() >= 2 {
        for split_index in 1..dash_segments.len() {
            let version_candidate = dash_segments[split_index..].join("-");
            let semver_candidate = version_candidate.trim_start_matches('v');
            if Version::parse(semver_candidate).is_ok() {
                let crate_name = dash_segments[..split_index].join("-");
                if !crate_name.is_empty() {
                    return (crate_name, Some(version_candidate));
                }
            }
        }
    }

    // Strategy 2: split on `_` and take a single segment as the version.
    // Handles nightly toolchain output like
    // `tokio_1.48.0_x86_64-unknown-linux-gnu_latest`. Semver never contains
    // `_`, so each segment is tested individually.
    let underscore_segments = normalized
        .split('_')
        .collect::<Vec<_>>();
    if underscore_segments.len() >= 2 {
        for split_index in 1..underscore_segments.len() {
            let version_candidate = underscore_segments[split_index];
            let semver_candidate = version_candidate.trim_start_matches('v');
            if Version::parse(semver_candidate).is_ok() {
                let crate_name = underscore_segments[..split_index].join("_");
                if !crate_name.is_empty() {
                    return (crate_name, Some(version_candidate.to_string()));
                }
            }
        }
    }

    (normalized.to_string(), None)
}

/// Read a local rustdoc JSON file, transparently decompressing `.json.zst`.
fn read_local_rustdoc_file(path: &Path) -> Result<String, String> {
    let raw = std::fs::read(path)
        .map_err(|e| format!("failed to read local rustdoc JSON file {}: {e}", path.display()))?;

    let bytes = if path
        .file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|n| n.ends_with(".zst"))
    {
        zstd::decode_all(raw.as_slice()).map_err(|e| {
            format!("failed to decompress zstd rustdoc file {}: {e}", path.display())
        })?
    } else {
        raw
    };

    String::from_utf8(bytes).map_err(|e| {
        format!("invalid UTF-8 in rustdoc JSON file {} (local fallback): {e}", path.display())
    })
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

/// Extracts the `format_version` field from raw rustdoc JSON content without
/// fully deserializing the document.
fn extract_format_version(content: &str) -> Option<u64> {
    serde_json::from_str::<serde_json::Value>(content)
        .ok()
        .and_then(|value| {
            value
                .get("format_version")
                .and_then(serde_json::Value::as_u64)
        })
}

/// Attempts to deserialize a rustdoc JSON payload whose `format_version` is
/// older than our compiled `rustdoc-types` crate by patching known schema
/// differences into a `serde_json::Value` tree, then converting to `Crate`.
///
/// Known patches:
/// - **56 → 57**: `ExternalCrate::path` (a `PathBuf`) was added as a required
///   field.  We inject `"path": ""` into each entry.
fn try_compat_deserialize(content: &str) -> Option<RustdocCrate> {
    let mut doc: serde_json::Value = serde_json::from_str(content).ok()?;

    let fv = doc
        .get("format_version")
        .and_then(serde_json::Value::as_u64)?;

    // Nothing to patch for current or future versions.
    if fv >= u64::from(rustdoc_types::FORMAT_VERSION) {
        return None;
    }

    // Patch: 56 → 57 — add missing `path` to each `external_crates` entry.
    if fv < 57
        && let Some(external_crates) = doc
            .get_mut("external_crates")
            .and_then(serde_json::Value::as_object_mut)
    {
        for entry in external_crates.values_mut() {
            if let Some(obj) = entry.as_object_mut() {
                obj.entry("path")
                    .or_insert(serde_json::Value::String(String::new()));
            }
        }
    }

    // Future format_version patches go here (if fv < 58 { ... }).

    serde_json::from_value(doc).ok()
}

fn format_rustdoc_parse_error(
    candidate: &RustdocSyncCandidateRow,
    source_path: &str,
    content: &str,
    parse_error: &serde_json::Error,
) -> String {
    let format_version = extract_format_version(content);

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
        RustdocVisibility::Crate => Some("pub".to_string()),
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

/// Converts the `item.attrs` list into a JSONB-friendly representation.
///
/// Returns `None` when the list is empty so we don't store `[]` for every item.
fn serialize_attrs(attrs: &[Attribute]) -> Option<serde_json::Value> {
    if attrs.is_empty() {
        return None;
    }

    let arr: Vec<serde_json::Value> = attrs
        .iter()
        .map(|attr| match attr {
            Attribute::NonExhaustive => json!({"kind": "non_exhaustive"}),
            Attribute::MustUse { reason } => json!({"kind": "must_use", "reason": reason}),
            Attribute::MacroExport => json!({"kind": "macro_export"}),
            Attribute::ExportName(name) => json!({"kind": "export_name", "name": name}),
            Attribute::LinkSection(name) => json!({"kind": "link_section", "name": name}),
            Attribute::AutomaticallyDerived => json!({"kind": "automatically_derived"}),
            Attribute::Repr(repr) => {
                let kind_str = match &repr.kind {
                    rustdoc_types::ReprKind::Rust => "Rust",
                    rustdoc_types::ReprKind::C => "C",
                    rustdoc_types::ReprKind::Transparent => "transparent",
                    rustdoc_types::ReprKind::Simd => "simd",
                };
                json!({
                    "kind": "repr",
                    "repr_kind": kind_str,
                    "align": repr.align,
                    "packed": repr.packed,
                    "int": repr.int,
                })
            }
            Attribute::NoMangle => json!({"kind": "no_mangle"}),
            Attribute::TargetFeature { enable } => {
                json!({"kind": "target_feature", "enable": enable})
            }
            Attribute::Other(raw) => json!({"kind": "other", "raw": raw}),
        })
        .collect();
    Some(json!(arr))
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
            docs: item.docs.clone(),
            attrs: serialize_attrs(&item.attrs),
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
        docs: item.docs.clone(),
        attrs: serialize_attrs(&item.attrs),
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
        docs: item.docs.clone(),
        attrs: serialize_attrs(&item.attrs),
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
        docs: item.docs.clone(),
        attrs: serialize_attrs(&item.attrs),
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
        docs: item.docs.clone(),
        attrs: serialize_attrs(&item.attrs),
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
            docs: item.docs.clone(),
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
            docs: item.docs.clone(),
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
        let krate = match serde_json::from_str::<RustdocCrate>(content) {
            Ok(krate) => krate,
            Err(error) => {
                // If the payload has an older format_version, try to patch
                // known schema differences and re-deserialize before giving up.
                if let Some(patched) = try_compat_deserialize(content) {
                    tracing::debug!(
                        crate_name = %candidate.crate_name,
                        version = %candidate.version,
                        source = %source_path,
                        expected_format_version = rustdoc_types::FORMAT_VERSION,
                        "recovered older-format rustdoc JSON via compat patching",
                    );
                    patched
                } else {
                    return Err(format_rustdoc_parse_error(
                        candidate,
                        source_path,
                        content,
                        &error,
                    ));
                }
            }
        };

        if krate.format_version != rustdoc_types::FORMAT_VERSION {
            tracing::debug!(
                crate_name = %candidate.crate_name,
                version = %candidate.version,
                source = %source_path,
                payload_format_version = krate.format_version,
                expected_format_version = rustdoc_types::FORMAT_VERSION,
                "rustdoc JSON format_version mismatch; ingesting anyway since \
                 deserialization succeeded",
            );
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
        let source_file_id = upsert_source_file_unconditional(
            &self.state.db,
            candidate.crate_version_id,
            source_path,
            &file_sha256_hex(content_bytes),
            content_bytes.len() as i64,
            Some("rustdoc_json"),
        )
        .await
        .map_err(|e| {
            format!(
                "failed to upsert rustdoc source file {} for {}@{}: {e}",
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

    /// Fetches and indexes rustdoc JSON from docs.rs for synced crate versions.
    /// Syncs rustdoc JSON symbols for indexed crate versions.
    ///
    /// When `locally_present_only` is `true`, only crate versions flagged as
    /// present in the user's cargo registry are processed.
    pub async fn sync_rustdoc_json_cache(
        &self,
        crate_name: Option<String>,
        page: Option<u32>,
        per_page: Option<u32>,
        locally_present_only: bool,
        skip_enriched: bool,
        retry_cooldown_seconds: Option<i64>,
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

        let mut candidates = fetch_rustdoc_sync_candidates(
            &self.state.db,
            crate_filter.as_deref(),
            i64::from(per_page),
            i64::from(offset),
            locally_present_only,
            skip_enriched,
            retry_cooldown_seconds,
        )
        .await
        .map_err(|e| format!("rustdoc JSON sync failed to load crate versions: {e}"))?;

        // Sort by reverse semver so newest versions are processed first.
        candidates.sort_by(|a, b| {
            let av = semver::Version::parse(&a.version).ok();
            let bv = semver::Version::parse(&b.version).ok();
            match (bv, av) {
                (Some(bv), Some(av)) => bv.cmp(&av),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => a
                    .crate_name
                    .cmp(&b.crate_name),
            }
        });

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
                            let stem = rustdoc_logical_stem(&path)?;
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

            tracing::info!(
                crate_name = %candidate.crate_name,
                version = %candidate.version,
                "enriching crate with rustdoc JSON"
            );

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
                            Ok(()) => {
                                let _ = mark_version_rustdoc_enriched(
                                    &self.state.db,
                                    candidate.crate_version_id,
                                )
                                .await;
                                continue;
                            }
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

                    match read_local_rustdoc_file(&local_candidate.path) {
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
                        Err(error) => source_errors.push(error),
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

            if local_ingested {
                let _ =
                    mark_version_rustdoc_enriched(&self.state.db, candidate.crate_version_id).await;
            } else {
                let _ = mark_version_rustdoc_attempted(&self.state.db, candidate.crate_version_id)
                    .await;
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
        assert_eq!(visibility_string(&RustdocVisibility::Crate), Some("pub".to_string()));
    }

    // ---- compat deserialization tests ----

    /// Builds a v57-shaped JSON Value with external_crates populated, then
    /// strips the fields that were added in v57 and downgrades
    /// `format_version` to simulate a v56 payload.
    fn build_v56_json() -> String {
        let mut krate = build_fixture_crate();
        // Add external crate entries so there's something to patch.
        krate.external_crates.insert(
            1,
            rustdoc_types::ExternalCrate {
                name: "std".into(),
                html_root_url: Some("https://doc.rust-lang.org/stable/".into()),
                path: PathBuf::from("/fake/libstd.rlib"),
            },
        );
        krate.external_crates.insert(
            2,
            rustdoc_types::ExternalCrate {
                name: "serde".into(),
                html_root_url: None,
                path: PathBuf::from("/fake/libserde.rlib"),
            },
        );

        let mut doc: serde_json::Value =
            serde_json::to_value(&krate).expect("fixture should serialize");

        // Downgrade to v56: remove `path` from every external_crates entry.
        doc["format_version"] = serde_json::json!(56);
        if let Some(ext) = doc
            .get_mut("external_crates")
            .and_then(serde_json::Value::as_object_mut)
        {
            for entry in ext.values_mut() {
                if let Some(obj) = entry.as_object_mut() {
                    obj.remove("path");
                }
            }
        }

        serde_json::to_string(&doc).expect("fixture should re-serialize")
    }

    #[test]
    fn try_compat_deserialize_recovers_v56_payload() {
        let v56_json = build_v56_json();

        // Strict deserialization must fail (missing `path` field).
        assert!(
            serde_json::from_str::<RustdocCrate>(&v56_json).is_err(),
            "v56 payload should not deserialize with strict v57 types"
        );

        // Compat patching should succeed.
        let krate = try_compat_deserialize(&v56_json)
            .expect("try_compat_deserialize should recover v56 payload");

        assert_eq!(krate.format_version, 56);
        assert_eq!(krate.external_crates.len(), 2);
        assert_eq!(krate.external_crates[&1].name, "std");
        assert_eq!(krate.external_crates[&2].name, "serde");
        // Patched `path` should be the default empty PathBuf.
        assert_eq!(krate.external_crates[&1].path, PathBuf::new());
    }

    #[test]
    fn try_compat_deserialize_returns_none_for_current_version() {
        let krate = build_fixture_crate();
        let json = serde_json::to_string(&krate).expect("fixture should serialize");

        // Current format_version should not trigger compat patching.
        assert!(
            try_compat_deserialize(&json).is_none(),
            "should return None for current format_version"
        );
    }

    #[test]
    fn try_compat_deserialize_returns_none_for_garbage() {
        assert!(try_compat_deserialize("not json at all").is_none());
        assert!(try_compat_deserialize("{}").is_none());
        assert!(try_compat_deserialize(r#"{"format_version": 57}"#).is_none());
    }
}
