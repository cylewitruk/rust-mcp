use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use semver::Version;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use syn::punctuated::Punctuated;
use syn::{Attribute, Item, Type, Visibility};

use crate::db::indexing::{
    count_source_file_index_rows, fetch_local_cache_version_keys, fetch_source_file_id_optional,
    prune_source_file_index_and_files, replace_source_file_index_rows,
    upsert_source_file_if_sha_changed,
};
use crate::db::models::{
    IndexedExtractionBatch, IndexedImplInsert, IndexedSymbolInsert, IndexedTypeInsert,
};
use crate::mcp::server::McpServer;
use crate::mcp::utils::{normalize_optional, normalize_required, sync_page, sync_per_page};

#[derive(Debug)]
struct IndexedSourceFile {
    relative_path: String,
    sha256: String,
    file_size: i64,
    language: Option<String>,
    content: String,
}

#[derive(Debug)]
struct IndexedCacheEntry {
    crate_name: String,
    version: String,
    crate_version_id: i64,
    files: Vec<IndexedSourceFile>,
}

/// Outcome of a local cargo registry source cache refresh.
#[derive(Debug, Default)]
pub struct LocalCacheRefreshOutcome {
    /// Number of crate versions whose source directories were scanned.
    pub scanned_versions: usize,
    /// Total number of source files encountered during the scan.
    pub scanned_files: usize,
    /// Number of source files inserted or updated in the database.
    pub upserted_files: usize,
    /// Number of stale source files removed from the database.
    pub deleted_files: usize,
    /// List of `crate@version` strings that were touched during the refresh.
    pub touched_versions: Vec<String>,
    /// Errors encountered during the refresh, one per failed operation.
    pub errors: Vec<String>,
}

fn extension_language(path: &Path) -> Option<String> {
    match path
        .extension()
        .and_then(OsStr::to_str)
    {
        Some("rs") => Some("rust".to_string()),
        Some("toml") => Some("toml".to_string()),
        Some("md") => Some("markdown".to_string()),
        Some("json") => Some("json".to_string()),
        Some("yaml") | Some("yml") => Some("yaml".to_string()),
        Some("txt") => Some("text".to_string()),
        _ => None,
    }
}

fn is_text_file(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(OsStr::to_str),
        Some("rs" | "toml" | "md" | "json" | "yaml" | "yml" | "txt")
    )
}

fn file_sha256_hex(content: &[u8]) -> String {
    let digest = Sha256::digest(content);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn walk_text_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut pending = vec![root.to_path_buf()];
    let mut out = Vec::new();

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
            } else if file_type.is_file() && is_text_file(&path) {
                out.push(path);
            }
        }
    }

    out.sort();
    Ok(out)
}

fn scan_cache_version_dir(
    version_dir: &Path,
    path_contains: Option<&str>,
) -> Result<Vec<IndexedSourceFile>, String> {
    const MAX_TEXT_FILE_BYTES: usize = 512 * 1024;

    let mut files = Vec::new();
    for file_path in walk_text_files(version_dir)? {
        let relative = file_path
            .strip_prefix(version_dir)
            .map_err(|e| format!("failed to build relative path for {}: {e}", file_path.display()))?
            .to_string_lossy()
            .replace('\\', "/");

        if let Some(filter) = path_contains
            && !relative
                .to_ascii_lowercase()
                .contains(filter)
        {
            continue;
        }

        let metadata = std::fs::metadata(&file_path)
            .map_err(|e| format!("failed to stat {}: {e}", file_path.display()))?;
        if metadata.len() as usize > MAX_TEXT_FILE_BYTES {
            continue;
        }

        let bytes = std::fs::read(&file_path)
            .map_err(|e| format!("failed to read {}: {e}", file_path.display()))?;
        if bytes.contains(&0) {
            continue;
        }

        let content = match String::from_utf8(bytes.clone()) {
            Ok(content) => content,
            Err(_) => continue,
        };

        files.push(IndexedSourceFile {
            relative_path: relative,
            sha256: file_sha256_hex(&bytes),
            file_size: metadata.len() as i64,
            language: extension_language(&file_path),
            content,
        });
    }

    Ok(files)
}

fn normalize_symbol_signature(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(
            trimmed
                .chars()
                .take(240)
                .collect(),
        )
    }
}

fn tokens_to_string<T: quote::ToTokens>(value: &T) -> String {
    value
        .to_token_stream()
        .to_string()
}

fn extract_generic_params(generics: &syn::Generics) -> Value {
    let params = generics
        .params
        .iter()
        .map(tokens_to_string)
        .collect::<Vec<_>>();
    json!(params)
}

fn extract_type_terminal_name(ty: &Type) -> Option<String> {
    match ty {
        Type::Path(type_path) => type_path
            .path
            .segments
            .last()
            .map(|segment| segment.ident.to_string()),
        Type::Reference(reference) => extract_type_terminal_name(&reference.elem),
        Type::Paren(paren) => extract_type_terminal_name(&paren.elem),
        Type::Group(group) => extract_type_terminal_name(&group.elem),
        _ => None,
    }
}

fn derive_traits(attrs: &[Attribute]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for attr in attrs {
        if !attr.path().is_ident("derive") {
            continue;
        }

        let parsed =
            attr.parse_args_with(Punctuated::<syn::Path, syn::Token![,]>::parse_terminated);
        let Ok(paths) = parsed else {
            continue;
        };

        for path in paths {
            let display = tokens_to_string(&path);
            let Some(terminal) = path
                .segments
                .last()
                .map(|segment| segment.ident.to_string())
            else {
                continue;
            };
            out.push((terminal, display));
        }
    }
    out
}

fn line_for_symbol_name(content: &str, name: &str) -> i32 {
    for (idx, line) in content.lines().enumerate() {
        if line.contains(name) {
            return (idx + 1) as i32;
        }
    }
    1
}

fn line_for_method_name(content: &str, method_name: &str) -> i32 {
    let needle = format!("fn {method_name}");
    line_for_symbol_name(content, &needle)
}

fn extract_rust_symbols(content: &str) -> Result<IndexedExtractionBatch, String> {
    let file = syn::parse_file(content).map_err(|e| format!("rust parser error: {e}"))?;
    let mut symbols = Vec::new();
    let mut types = Vec::new();
    let mut impls = Vec::new();

    for item in file.items {
        match item {
            Item::Fn(function) => {
                let name = function.sig.ident.to_string();
                let start_line = line_for_symbol_name(content, &name);
                let signature = content
                    .lines()
                    .nth((start_line.saturating_sub(1)) as usize)
                    .and_then(normalize_symbol_signature);

                symbols.push(IndexedSymbolInsert {
                    name,
                    kind: "function".to_string(),
                    signature,
                    visibility: match function.vis {
                        Visibility::Public(_) => Some("public".to_string()),
                        _ => Some("private".to_string()),
                    },
                    start_line,
                    end_line: start_line,
                    ..Default::default()
                });
            }
            Item::Struct(structure) => {
                let type_name = structure.ident.to_string();
                let start_line = line_for_symbol_name(content, &type_name);
                let signature = content
                    .lines()
                    .nth((start_line.saturating_sub(1)) as usize)
                    .and_then(normalize_symbol_signature);

                symbols.push(IndexedSymbolInsert {
                    name: type_name.clone(),
                    kind: "struct".to_string(),
                    signature,
                    visibility: match structure.vis {
                        Visibility::Public(_) => Some("public".to_string()),
                        _ => Some("private".to_string()),
                    },
                    start_line,
                    end_line: start_line,
                    ..Default::default()
                });

                let fields = structure
                    .fields
                    .iter()
                    .map(|field| {
                        json!({
                            "name": field.ident.as_ref().map(|value| value.to_string()),
                            "type": tokens_to_string(&field.ty),
                        })
                    })
                    .collect::<Vec<_>>();

                types.push(IndexedTypeInsert {
                    type_name: type_name.clone(),
                    kind: "struct".to_string(),
                    visibility: match structure.vis {
                        Visibility::Public(_) => Some("public".to_string()),
                        _ => Some("private".to_string()),
                    },
                    generic_params: extract_generic_params(&structure.generics),
                    fields: json!(fields),
                    variants: json!([]),
                    start_line,
                    end_line: start_line,
                    ..Default::default()
                });

                for (trait_name, trait_name_display) in derive_traits(&structure.attrs) {
                    impls.push(IndexedImplInsert {
                        type_name: type_name.clone(),
                        type_name_display: Some(type_name.clone()),
                        trait_name: Some(trait_name),
                        trait_name_display: Some(trait_name_display),
                        impl_kind: "derive".to_string(),
                        methods: json!([]),
                        start_line,
                        end_line: start_line,
                        ..Default::default()
                    });
                }
            }
            Item::Enum(enumeration) => {
                let type_name = enumeration.ident.to_string();
                let start_line = line_for_symbol_name(content, &type_name);
                let signature = content
                    .lines()
                    .nth((start_line.saturating_sub(1)) as usize)
                    .and_then(normalize_symbol_signature);

                symbols.push(IndexedSymbolInsert {
                    name: type_name.clone(),
                    kind: "enum".to_string(),
                    signature,
                    visibility: match enumeration.vis {
                        Visibility::Public(_) => Some("public".to_string()),
                        _ => Some("private".to_string()),
                    },
                    start_line,
                    end_line: start_line,
                    ..Default::default()
                });

                let variants = enumeration
                    .variants
                    .iter()
                    .map(|variant| {
                        let fields = variant
                            .fields
                            .iter()
                            .map(|field| {
                                json!({
                                    "name": field.ident.as_ref().map(|value| value.to_string()),
                                    "type": tokens_to_string(&field.ty),
                                })
                            })
                            .collect::<Vec<_>>();
                        json!({
                            "name": variant.ident.to_string(),
                            "fields": fields,
                            "discriminant": variant.discriminant.as_ref().map(|(_, expr)| tokens_to_string(expr)),
                        })
                    })
                    .collect::<Vec<_>>();

                types.push(IndexedTypeInsert {
                    type_name: type_name.clone(),
                    kind: "enum".to_string(),
                    visibility: match enumeration.vis {
                        Visibility::Public(_) => Some("public".to_string()),
                        _ => Some("private".to_string()),
                    },
                    generic_params: extract_generic_params(&enumeration.generics),
                    fields: json!([]),
                    variants: json!(variants),
                    start_line,
                    end_line: start_line,
                    ..Default::default()
                });

                for (trait_name, trait_name_display) in derive_traits(&enumeration.attrs) {
                    impls.push(IndexedImplInsert {
                        type_name: type_name.clone(),
                        type_name_display: Some(type_name.clone()),
                        trait_name: Some(trait_name),
                        trait_name_display: Some(trait_name_display),
                        impl_kind: "derive".to_string(),
                        methods: json!([]),
                        start_line,
                        end_line: start_line,
                        ..Default::default()
                    });
                }
            }
            Item::Union(union_item) => {
                let type_name = union_item.ident.to_string();
                let start_line = line_for_symbol_name(content, &type_name);
                let signature = content
                    .lines()
                    .nth((start_line.saturating_sub(1)) as usize)
                    .and_then(normalize_symbol_signature);

                symbols.push(IndexedSymbolInsert {
                    name: type_name.clone(),
                    kind: "union".to_string(),
                    signature,
                    visibility: match union_item.vis {
                        Visibility::Public(_) => Some("public".to_string()),
                        _ => Some("private".to_string()),
                    },
                    start_line,
                    end_line: start_line,
                    ..Default::default()
                });

                let fields = union_item
                    .fields
                    .named
                    .iter()
                    .map(|field| {
                        json!({
                            "name": field.ident.as_ref().map(|value| value.to_string()),
                            "type": tokens_to_string(&field.ty),
                        })
                    })
                    .collect::<Vec<_>>();

                types.push(IndexedTypeInsert {
                    type_name: type_name.clone(),
                    kind: "union".to_string(),
                    visibility: match union_item.vis {
                        Visibility::Public(_) => Some("public".to_string()),
                        _ => Some("private".to_string()),
                    },
                    generic_params: extract_generic_params(&union_item.generics),
                    fields: json!(fields),
                    variants: json!([]),
                    start_line,
                    end_line: start_line,
                    ..Default::default()
                });

                for (trait_name, trait_name_display) in derive_traits(&union_item.attrs) {
                    impls.push(IndexedImplInsert {
                        type_name: type_name.clone(),
                        type_name_display: Some(type_name.clone()),
                        trait_name: Some(trait_name),
                        trait_name_display: Some(trait_name_display),
                        impl_kind: "derive".to_string(),
                        methods: json!([]),
                        start_line,
                        end_line: start_line,
                        ..Default::default()
                    });
                }
            }
            Item::Trait(trait_item) => {
                let name = trait_item.ident.to_string();
                let start_line = line_for_symbol_name(content, &name);
                let signature = content
                    .lines()
                    .nth((start_line.saturating_sub(1)) as usize)
                    .and_then(normalize_symbol_signature);

                symbols.push(IndexedSymbolInsert {
                    name,
                    kind: "trait".to_string(),
                    signature,
                    visibility: match trait_item.vis {
                        Visibility::Public(_) => Some("public".to_string()),
                        _ => Some("private".to_string()),
                    },
                    start_line,
                    end_line: start_line,
                    ..Default::default()
                });
            }
            Item::Type(type_alias) => {
                let name = type_alias.ident.to_string();
                let start_line = line_for_symbol_name(content, &name);
                let signature = content
                    .lines()
                    .nth((start_line.saturating_sub(1)) as usize)
                    .and_then(normalize_symbol_signature);

                symbols.push(IndexedSymbolInsert {
                    name,
                    kind: "type_alias".to_string(),
                    signature,
                    visibility: match type_alias.vis {
                        Visibility::Public(_) => Some("public".to_string()),
                        _ => Some("private".to_string()),
                    },
                    start_line,
                    end_line: start_line,
                    ..Default::default()
                });
            }
            Item::Const(const_item) => {
                let name = const_item.ident.to_string();
                let start_line = line_for_symbol_name(content, &name);
                let signature = content
                    .lines()
                    .nth((start_line.saturating_sub(1)) as usize)
                    .and_then(normalize_symbol_signature);

                symbols.push(IndexedSymbolInsert {
                    name,
                    kind: "const".to_string(),
                    signature,
                    visibility: match const_item.vis {
                        Visibility::Public(_) => Some("public".to_string()),
                        _ => Some("private".to_string()),
                    },
                    start_line,
                    end_line: start_line,
                    ..Default::default()
                });
            }
            Item::Static(static_item) => {
                let name = static_item.ident.to_string();
                let start_line = line_for_symbol_name(content, &name);
                let signature = content
                    .lines()
                    .nth((start_line.saturating_sub(1)) as usize)
                    .and_then(normalize_symbol_signature);

                symbols.push(IndexedSymbolInsert {
                    name,
                    kind: "static".to_string(),
                    signature,
                    visibility: match static_item.vis {
                        Visibility::Public(_) => Some("public".to_string()),
                        _ => Some("private".to_string()),
                    },
                    start_line,
                    end_line: start_line,
                    ..Default::default()
                });
            }
            Item::Mod(module_item) => {
                let name = module_item.ident.to_string();
                let start_line = line_for_symbol_name(content, &name);
                let signature = content
                    .lines()
                    .nth((start_line.saturating_sub(1)) as usize)
                    .and_then(normalize_symbol_signature);

                symbols.push(IndexedSymbolInsert {
                    name,
                    kind: "module".to_string(),
                    signature,
                    visibility: match module_item.vis {
                        Visibility::Public(_) => Some("public".to_string()),
                        _ => Some("private".to_string()),
                    },
                    start_line,
                    end_line: start_line,
                    ..Default::default()
                });
            }
            Item::Impl(impl_item) => {
                let Some(type_name) = extract_type_terminal_name(&impl_item.self_ty) else {
                    continue;
                };
                let type_name_display = Some(tokens_to_string(&impl_item.self_ty));
                let (trait_name, trait_name_display, impl_kind) =
                    if let Some((_, trait_path, _)) = impl_item.trait_ {
                        (
                            trait_path
                                .segments
                                .last()
                                .map(|segment| segment.ident.to_string()),
                            Some(tokens_to_string(&trait_path)),
                            "trait".to_string(),
                        )
                    } else {
                        (None, None, "inherent".to_string())
                    };

                let method_entries = impl_item
                    .items
                    .iter()
                    .filter_map(|impl_member| match impl_member {
                        syn::ImplItem::Fn(method) => {
                            let method_name = method.sig.ident.to_string();
                            let line = line_for_method_name(content, &method_name);
                            let signature = content
                                .lines()
                                .nth((line.saturating_sub(1)) as usize)
                                .and_then(normalize_symbol_signature)
                                .or_else(|| Some(tokens_to_string(&method.sig)));

                            Some(json!({
                                "name": method_name,
                                "signature": signature,
                            }))
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>();

                let start_line = method_entries
                    .iter()
                    .filter_map(|method| {
                        method
                            .get("name")
                            .and_then(Value::as_str)
                            .map(|name| line_for_method_name(content, name))
                    })
                    .min()
                    .unwrap_or_else(|| line_for_symbol_name(content, &type_name));

                impls.push(IndexedImplInsert {
                    type_name,
                    type_name_display,
                    trait_name,
                    trait_name_display,
                    impl_kind,
                    methods: json!(method_entries),
                    start_line,
                    end_line: start_line,
                    ..Default::default()
                });
            }
            _ => {}
        }
    }

    Ok(IndexedExtractionBatch {
        symbols,
        types,
        impls,
        traits: Vec::new(),
    })
}

impl McpServer {
    /// Refreshes the local source file cache from the cargo registry directory.
    ///
    /// When `locally_present_only` is `true`, only crate versions flagged as
    /// present in the user's cargo registry are considered.
    pub async fn sync_local_source_cache(
        &self,
        crate_name: Option<String>,
        query: Option<String>,
        page: Option<u32>,
        per_page: Option<u32>,
        locally_present_only: bool,
    ) -> Result<LocalCacheRefreshOutcome, String> {
        let requested_crate_name = match crate_name {
            Some(value) => Some(normalize_required(value, "crate_name")?),
            None => None,
        };
        let path_contains = normalize_optional(query).map(|v| v.to_ascii_lowercase());
        let page = sync_page(page);
        let per_page = sync_per_page(per_page) as usize;
        let offset = ((page - 1) as usize).saturating_mul(per_page);

        let src_root = self
            .state
            .config
            .cargo_registry_dir
            .join("src");
        if !src_root.exists() {
            return Ok(LocalCacheRefreshOutcome {
                errors: vec![format!(
                    "cargo registry source directory not found: {}",
                    src_root.display()
                )],
                ..Default::default()
            });
        }

        let version_rows = fetch_local_cache_version_keys(
            &self.state.db,
            requested_crate_name.as_deref(),
            locally_present_only,
        )
        .await
        .map_err(|e| format!("local cache refresh failed to load crate versions: {e}"))?;

        let version_map = version_rows
            .into_iter()
            .map(|row| (row.cache_dir_name.clone(), row))
            .collect::<HashMap<_, _>>();

        let mut candidates = Vec::new();
        let registries = std::fs::read_dir(&src_root).map_err(|e| {
            format!("failed to read cargo registry source dir {}: {e}", src_root.display())
        })?;
        for registry_dir in registries {
            let registry_dir = registry_dir.map_err(|e| {
                format!("failed to read registry directory entry under {}: {e}", src_root.display())
            })?;
            let registry_path = registry_dir.path();
            if !registry_path.is_dir() {
                continue;
            }

            let version_dirs = std::fs::read_dir(&registry_path)
                .map_err(|e| format!("failed to read {}: {e}", registry_path.display()))?;
            for version_dir in version_dirs {
                let version_dir = version_dir.map_err(|e| {
                    format!(
                        "failed to read package directory entry under {}: {e}",
                        registry_path.display()
                    )
                })?;
                let path = version_dir.path();
                if !path.is_dir() {
                    continue;
                }

                let Some(dir_name) = path
                    .file_name()
                    .and_then(OsStr::to_str)
                    .map(ToString::to_string)
                else {
                    continue;
                };

                let Some(mapped) = version_map.get(&dir_name) else {
                    continue;
                };

                candidates.push((dir_name, mapped.version.clone(), path, mapped.crate_version_id));
            }
        }

        // Sort by reverse semver so newest versions are indexed first.
        // Versions that fail to parse as semver are sorted last (alphabetically).
        candidates.sort_by(|left, right| {
            let lv = Version::parse(&left.1).ok();
            let rv = Version::parse(&right.1).ok();
            match (rv, lv) {
                (Some(r), Some(l)) => r.cmp(&l),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => left.0.cmp(&right.0),
            }
        });
        let selected = candidates
            .into_iter()
            .skip(offset)
            .take(per_page)
            .collect::<Vec<_>>();

        let mut outcome = LocalCacheRefreshOutcome::default();

        for (dir_name, _version, version_dir, crate_version_id) in selected {
            let Some(mapped) = version_map.get(&dir_name) else {
                continue;
            };

            tracing::info!(
                crate_name = %mapped.crate_name,
                version = %mapped.version,
                "indexing source files from local cargo registry"
            );

            let files = match scan_cache_version_dir(&version_dir, path_contains.as_deref()) {
                Ok(files) => files,
                Err(error) => {
                    outcome.errors.push(error);
                    continue;
                }
            };

            let entry = IndexedCacheEntry {
                crate_name: mapped.crate_name.clone(),
                version: mapped.version.clone(),
                crate_version_id,
                files,
            };

            let mut seen_paths = Vec::new();
            for file in &entry.files {
                seen_paths.push(file.relative_path.clone());
                outcome.scanned_files += 1;

                let upserted_id = upsert_source_file_if_sha_changed(
                    &self.state.db,
                    entry.crate_version_id,
                    &file.relative_path,
                    &file.sha256,
                    file.file_size,
                    file.language.as_deref(),
                )
                .await
                .map_err(|e| {
                    format!(
                        "failed to upsert source file {} for {}@{}: {e}",
                        file.relative_path, entry.crate_name, entry.version
                    )
                })?;

                // Use the returned id when a write occurred, otherwise look it up.
                let source_file_id = match upserted_id {
                    Some(id) => Some(id),
                    None => fetch_source_file_id_optional(
                        &self.state.db,
                        entry.crate_version_id,
                        &file.relative_path,
                    )
                    .await
                    .map_err(|e| {
                        format!(
                            "failed to lookup source file id {} for {}@{}: {e}",
                            file.relative_path, entry.crate_name, entry.version
                        )
                    })?,
                };

                if file.language.as_deref() == Some("rust") {
                    let Some(source_file_id) = source_file_id else {
                        continue;
                    };

                    let existing_symbol_count =
                        count_source_file_index_rows(&self.state.db, source_file_id)
                            .await
                            .map_err(|e| {
                                format!(
                                    "failed to count symbols for {} in {}@{}: {e}",
                                    file.relative_path, entry.crate_name, entry.version
                                )
                            })?;

                    if upserted_id.is_some() || existing_symbol_count == 0 {
                        let extracted = match extract_rust_symbols(&file.content) {
                            Ok(batch) => batch,
                            Err(e) => {
                                tracing::debug!(
                                    crate_name = %entry.crate_name,
                                    version = %entry.version,
                                    path = %file.relative_path,
                                    error = %e,
                                    "skipping unparseable rust file"
                                );
                                outcome.errors.push(format!(
                                    "skipped unparseable file {} in {}@{}: {e}",
                                    file.relative_path, entry.crate_name, entry.version
                                ));
                                continue;
                            }
                        };

                        replace_source_file_index_rows(
                            &self.state.db,
                            entry.crate_version_id,
                            source_file_id,
                            "syn",
                            &extracted,
                        )
                        .await
                        .map_err(|e| {
                            format!(
                                "failed to replace indexed rows for {} in {}@{}: {e}",
                                file.relative_path, entry.crate_name, entry.version
                            )
                        })?;
                    }
                }

                if upserted_id.is_some() {
                    outcome.upserted_files += 1;
                }
            }

            let deleted = prune_source_file_index_and_files(
                &self.state.db,
                entry.crate_version_id,
                &seen_paths,
            )
            .await
            .map_err(|e| {
                format!(
                    "failed to prune stale source/index rows for {}@{}: {e}",
                    entry.crate_name, entry.version
                )
            })?;

            outcome.deleted_files += deleted as usize;
            outcome.scanned_versions += 1;
            outcome
                .touched_versions
                .push(format!("{}@{}", entry.crate_name, entry.version));

            tracing::debug!(
                crate_name = %entry.crate_name,
                version = %entry.version,
                files = seen_paths.len(),
                deleted,
                "source file indexing complete for crate version"
            );
        }

        Ok(outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::extract_rust_symbols;

    #[test]
    fn extract_rust_symbols_succeeds_on_valid_rust() {
        let code = r#"
            pub fn hello() -> String { String::new() }
            struct Foo { x: i32 }
        "#;
        let result = extract_rust_symbols(code);
        assert!(result.is_ok());
        let batch = result.unwrap();
        assert_eq!(batch.symbols.len(), 2); // hello + Foo
    }

    #[test]
    fn extract_rust_symbols_returns_error_on_invalid_syntax() {
        // Intentionally invalid Rust — mirrors the compile-fail test fixtures
        // that caused permanent job failures (e.g. pin-project UI tests).
        let invalid_code = "fn foo(, bad) { }";
        let result = extract_rust_symbols(invalid_code);
        assert!(result.is_err(), "expected parse error for invalid syntax");
    }

    #[test]
    fn extract_rust_symbols_returns_error_on_non_module_content() {
        // Non-module content like CSV processing scripts that have a shebang
        // or other non-Rust top-level constructs.
        let non_module = "#!/usr/bin/env python3\nprint('hello')";
        let result = extract_rust_symbols(non_module);
        assert!(result.is_err(), "expected parse error for non-Rust content");
    }

    #[test]
    fn extract_rust_symbols_returns_error_on_deliberately_malformed_test_fixture() {
        // Mirrors compile-fail test fixtures (e.g. syn's tests/test_item.rs)
        // that contain intentionally malformed Rust to test diagnostics.
        let code = "struct Foo { x: i32, , }";
        let result = extract_rust_symbols(code);
        assert!(result.is_err(), "expected parse error for malformed struct");
    }
}
