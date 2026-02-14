use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use sqlx::FromRow;
use syn::punctuated::Punctuated;
use syn::{Attribute, Item, Type, Visibility};

use super::server::McpServer;
use super::utils::{normalize_optional, normalize_required, sync_page, sync_per_page};

#[derive(Debug, FromRow)]
struct CrateVersionKeyRow {
    cache_dir_name: String,
    crate_name: String,
    version: String,
    crate_version_id: i64,
}

#[derive(Debug)]
struct IndexedSourceFile {
    relative_path: String,
    sha256: String,
    file_size: i64,
    language: Option<String>,
    content: String,
}

#[derive(Debug)]
struct ExtractedSymbol {
    name: String,
    kind: String,
    signature: Option<String>,
    visibility: Option<String>,
    start_line: i32,
    end_line: i32,
}

#[derive(Debug)]
struct ExtractedType {
    type_name: String,
    kind: String,
    visibility: Option<String>,
    generic_params: Value,
    fields: Value,
    variants: Value,
    start_line: i32,
    end_line: i32,
}

#[derive(Debug)]
struct ExtractedImpl {
    type_name: String,
    type_name_display: Option<String>,
    trait_name: Option<String>,
    trait_name_display: Option<String>,
    impl_kind: String,
    methods: Value,
    start_line: i32,
    end_line: i32,
}

#[derive(Debug)]
struct RustExtraction {
    symbols: Vec<ExtractedSymbol>,
    types: Vec<ExtractedType>,
    impls: Vec<ExtractedImpl>,
}

#[derive(Debug)]
struct IndexedCacheEntry {
    crate_name: String,
    version: String,
    crate_version_id: i64,
    files: Vec<IndexedSourceFile>,
}

#[derive(Debug, Default)]
pub(super) struct LocalCacheRefreshOutcome {
    pub(super) scanned_versions: usize,
    pub(super) scanned_files: usize,
    pub(super) upserted_files: usize,
    pub(super) deleted_files: usize,
    pub(super) touched_versions: Vec<String>,
    pub(super) errors: Vec<String>,
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

fn extract_rust_symbols(content: &str) -> Result<RustExtraction, String> {
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

                symbols.push(ExtractedSymbol {
                    name,
                    kind: "function".to_string(),
                    signature,
                    visibility: match function.vis {
                        Visibility::Public(_) => Some("public".to_string()),
                        _ => Some("private".to_string()),
                    },
                    start_line,
                    end_line: start_line,
                });
            }
            Item::Struct(structure) => {
                let type_name = structure.ident.to_string();
                let start_line = line_for_symbol_name(content, &type_name);
                let signature = content
                    .lines()
                    .nth((start_line.saturating_sub(1)) as usize)
                    .and_then(normalize_symbol_signature);

                symbols.push(ExtractedSymbol {
                    name: type_name.clone(),
                    kind: "struct".to_string(),
                    signature,
                    visibility: match structure.vis {
                        Visibility::Public(_) => Some("public".to_string()),
                        _ => Some("private".to_string()),
                    },
                    start_line,
                    end_line: start_line,
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

                types.push(ExtractedType {
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
                });

                for (trait_name, trait_name_display) in derive_traits(&structure.attrs) {
                    impls.push(ExtractedImpl {
                        type_name: type_name.clone(),
                        type_name_display: Some(type_name.clone()),
                        trait_name: Some(trait_name),
                        trait_name_display: Some(trait_name_display),
                        impl_kind: "derive".to_string(),
                        methods: json!([]),
                        start_line,
                        end_line: start_line,
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

                symbols.push(ExtractedSymbol {
                    name: type_name.clone(),
                    kind: "enum".to_string(),
                    signature,
                    visibility: match enumeration.vis {
                        Visibility::Public(_) => Some("public".to_string()),
                        _ => Some("private".to_string()),
                    },
                    start_line,
                    end_line: start_line,
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

                types.push(ExtractedType {
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
                });

                for (trait_name, trait_name_display) in derive_traits(&enumeration.attrs) {
                    impls.push(ExtractedImpl {
                        type_name: type_name.clone(),
                        type_name_display: Some(type_name.clone()),
                        trait_name: Some(trait_name),
                        trait_name_display: Some(trait_name_display),
                        impl_kind: "derive".to_string(),
                        methods: json!([]),
                        start_line,
                        end_line: start_line,
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

                symbols.push(ExtractedSymbol {
                    name: type_name.clone(),
                    kind: "union".to_string(),
                    signature,
                    visibility: match union_item.vis {
                        Visibility::Public(_) => Some("public".to_string()),
                        _ => Some("private".to_string()),
                    },
                    start_line,
                    end_line: start_line,
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

                types.push(ExtractedType {
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
                });

                for (trait_name, trait_name_display) in derive_traits(&union_item.attrs) {
                    impls.push(ExtractedImpl {
                        type_name: type_name.clone(),
                        type_name_display: Some(type_name.clone()),
                        trait_name: Some(trait_name),
                        trait_name_display: Some(trait_name_display),
                        impl_kind: "derive".to_string(),
                        methods: json!([]),
                        start_line,
                        end_line: start_line,
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

                symbols.push(ExtractedSymbol {
                    name,
                    kind: "trait".to_string(),
                    signature,
                    visibility: match trait_item.vis {
                        Visibility::Public(_) => Some("public".to_string()),
                        _ => Some("private".to_string()),
                    },
                    start_line,
                    end_line: start_line,
                });
            }
            Item::Type(type_alias) => {
                let name = type_alias.ident.to_string();
                let start_line = line_for_symbol_name(content, &name);
                let signature = content
                    .lines()
                    .nth((start_line.saturating_sub(1)) as usize)
                    .and_then(normalize_symbol_signature);

                symbols.push(ExtractedSymbol {
                    name,
                    kind: "type_alias".to_string(),
                    signature,
                    visibility: match type_alias.vis {
                        Visibility::Public(_) => Some("public".to_string()),
                        _ => Some("private".to_string()),
                    },
                    start_line,
                    end_line: start_line,
                });
            }
            Item::Const(const_item) => {
                let name = const_item.ident.to_string();
                let start_line = line_for_symbol_name(content, &name);
                let signature = content
                    .lines()
                    .nth((start_line.saturating_sub(1)) as usize)
                    .and_then(normalize_symbol_signature);

                symbols.push(ExtractedSymbol {
                    name,
                    kind: "const".to_string(),
                    signature,
                    visibility: match const_item.vis {
                        Visibility::Public(_) => Some("public".to_string()),
                        _ => Some("private".to_string()),
                    },
                    start_line,
                    end_line: start_line,
                });
            }
            Item::Static(static_item) => {
                let name = static_item.ident.to_string();
                let start_line = line_for_symbol_name(content, &name);
                let signature = content
                    .lines()
                    .nth((start_line.saturating_sub(1)) as usize)
                    .and_then(normalize_symbol_signature);

                symbols.push(ExtractedSymbol {
                    name,
                    kind: "static".to_string(),
                    signature,
                    visibility: match static_item.vis {
                        Visibility::Public(_) => Some("public".to_string()),
                        _ => Some("private".to_string()),
                    },
                    start_line,
                    end_line: start_line,
                });
            }
            Item::Mod(module_item) => {
                let name = module_item.ident.to_string();
                let start_line = line_for_symbol_name(content, &name);
                let signature = content
                    .lines()
                    .nth((start_line.saturating_sub(1)) as usize)
                    .and_then(normalize_symbol_signature);

                symbols.push(ExtractedSymbol {
                    name,
                    kind: "module".to_string(),
                    signature,
                    visibility: match module_item.vis {
                        Visibility::Public(_) => Some("public".to_string()),
                        _ => Some("private".to_string()),
                    },
                    start_line,
                    end_line: start_line,
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

                impls.push(ExtractedImpl {
                    type_name,
                    type_name_display,
                    trait_name,
                    trait_name_display,
                    impl_kind,
                    methods: json!(method_entries),
                    start_line,
                    end_line: start_line,
                });
            }
            _ => {}
        }
    }

    Ok(RustExtraction { symbols, types, impls })
}

impl McpServer {
    pub(super) async fn sync_local_source_cache(
        &self,
        crate_name: Option<String>,
        query: Option<String>,
        page: Option<u32>,
        per_page: Option<u32>,
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

        let version_rows = sqlx::query_as::<_, CrateVersionKeyRow>(
            "SELECT
                (c.name || '-' || cv.version) AS cache_dir_name,
                c.name AS crate_name,
                cv.version,
                cv.id AS crate_version_id
             FROM crate_versions cv
             JOIN crates c ON c.id = cv.crate_id
             WHERE ($1::TEXT IS NULL OR c.name = $1)",
        )
        .bind(requested_crate_name.as_deref())
        .fetch_all(&self.state.db)
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

                candidates.push((dir_name, path, mapped.crate_version_id));
            }
        }

        candidates.sort_by(|left, right| left.0.cmp(&right.0));
        let selected = candidates
            .into_iter()
            .skip(offset)
            .take(per_page)
            .collect::<Vec<_>>();

        let mut outcome = LocalCacheRefreshOutcome::default();

        for (dir_name, version_dir, crate_version_id) in selected {
            let Some(mapped) = version_map.get(&dir_name) else {
                continue;
            };

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

                let affected = sqlx::query(
                    "INSERT INTO source_files (
                        crate_version_id, path, sha256, file_size, language, content, indexed_at
                     ) VALUES (
                        $1, $2, $3, $4, $5, $6, NOW()
                     )
                     ON CONFLICT (crate_version_id, path) DO UPDATE
                     SET sha256 = EXCLUDED.sha256,
                         file_size = EXCLUDED.file_size,
                         language = EXCLUDED.language,
                         content = EXCLUDED.content,
                         indexed_at = NOW()
                     WHERE source_files.sha256 IS DISTINCT FROM EXCLUDED.sha256",
                )
                .bind(entry.crate_version_id)
                .bind(&file.relative_path)
                .bind(&file.sha256)
                .bind(file.file_size)
                .bind(file.language.as_deref())
                .bind(&file.content)
                .execute(&self.state.db)
                .await
                .map_err(|e| {
                    format!(
                        "failed to upsert source file {} for {}@{}: {e}",
                        file.relative_path, entry.crate_name, entry.version
                    )
                })?
                .rows_affected();

                let source_file_id = sqlx::query_scalar::<_, i64>(
                    "SELECT id
                     FROM source_files
                     WHERE crate_version_id = $1 AND path = $2
                     LIMIT 1",
                )
                .bind(entry.crate_version_id)
                .bind(&file.relative_path)
                .fetch_optional(&self.state.db)
                .await
                .map_err(|e| {
                    format!(
                        "failed to lookup source file id {} for {}@{}: {e}",
                        file.relative_path, entry.crate_name, entry.version
                    )
                })?;

                if file.language.as_deref() == Some("rust") {
                    let Some(source_file_id) = source_file_id else {
                        continue;
                    };

                    let existing_symbol_count = sqlx::query_scalar::<_, i64>(
                        "SELECT (
                            (SELECT COUNT(*)::BIGINT FROM symbols WHERE source_file_id = $1)
                            + (SELECT COUNT(*)::BIGINT FROM crate_types WHERE source_file_id = $1)
                            + (SELECT COUNT(*)::BIGINT FROM crate_impls WHERE source_file_id = $1)
                         )::BIGINT",
                    )
                    .bind(source_file_id)
                    .fetch_one(&self.state.db)
                    .await
                    .map_err(|e| {
                        format!(
                            "failed to count symbols for {} in {}@{}: {e}",
                            file.relative_path, entry.crate_name, entry.version
                        )
                    })?;

                    if affected > 0 || existing_symbol_count == 0 {
                        sqlx::query("DELETE FROM symbols WHERE source_file_id = $1")
                            .bind(source_file_id)
                            .execute(&self.state.db)
                            .await
                            .map_err(|e| {
                                format!(
                                    "failed to clear symbols for {} in {}@{}: {e}",
                                    file.relative_path, entry.crate_name, entry.version
                                )
                            })?;

                        sqlx::query("DELETE FROM crate_types WHERE source_file_id = $1")
                            .bind(source_file_id)
                            .execute(&self.state.db)
                            .await
                            .map_err(|e| {
                                format!(
                                    "failed to clear crate types for {} in {}@{}: {e}",
                                    file.relative_path, entry.crate_name, entry.version
                                )
                            })?;

                        sqlx::query("DELETE FROM crate_impls WHERE source_file_id = $1")
                            .bind(source_file_id)
                            .execute(&self.state.db)
                            .await
                            .map_err(|e| {
                                format!(
                                    "failed to clear crate impls for {} in {}@{}: {e}",
                                    file.relative_path, entry.crate_name, entry.version
                                )
                            })?;

                        let extracted = extract_rust_symbols(&file.content).map_err(|e| {
                            format!(
                                "failed to parse rust symbols in {} for {}@{}: {e}",
                                file.relative_path, entry.crate_name, entry.version
                            )
                        })?;

                        for symbol in extracted.symbols {
                            sqlx::query(
                                "INSERT INTO symbols (
                                    crate_version_id,
                                    source_file_id,
                                    name,
                                    kind,
                                    signature,
                                    visibility,
                                    start_line,
                                    end_line,
                                    index_source,
                                    indexed_at
                                 ) VALUES (
                                    $1, $2, $3, $4, $5, $6, $7, $8, 'syn', NOW()
                                 )",
                            )
                            .bind(entry.crate_version_id)
                            .bind(source_file_id)
                            .bind(symbol.name)
                            .bind(symbol.kind)
                            .bind(symbol.signature)
                            .bind(symbol.visibility)
                            .bind(symbol.start_line)
                            .bind(symbol.end_line)
                            .execute(&self.state.db)
                            .await
                            .map_err(|e| {
                                format!(
                                    "failed to insert symbol for {} in {}@{}: {e}",
                                    file.relative_path, entry.crate_name, entry.version
                                )
                            })?;
                        }

                        for extracted_type in extracted.types {
                            sqlx::query(
                                "INSERT INTO crate_types (
                                    crate_version_id,
                                    source_file_id,
                                    type_name,
                                    kind,
                                    visibility,
                                    generic_params,
                                    fields,
                                    variants,
                                    start_line,
                                    end_line,
                                    index_source,
                                    indexed_at
                                 ) VALUES (
                                    $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, 'syn', NOW()
                                 )",
                            )
                            .bind(entry.crate_version_id)
                            .bind(source_file_id)
                            .bind(extracted_type.type_name)
                            .bind(extracted_type.kind)
                            .bind(extracted_type.visibility)
                            .bind(extracted_type.generic_params)
                            .bind(extracted_type.fields)
                            .bind(extracted_type.variants)
                            .bind(extracted_type.start_line)
                            .bind(extracted_type.end_line)
                            .execute(&self.state.db)
                            .await
                            .map_err(|e| {
                                format!(
                                    "failed to insert crate type for {} in {}@{}: {e}",
                                    file.relative_path, entry.crate_name, entry.version
                                )
                            })?;
                        }

                        for extracted_impl in extracted.impls {
                            sqlx::query(
                                "INSERT INTO crate_impls (
                                    crate_version_id,
                                    source_file_id,
                                    type_name,
                                    type_name_display,
                                    trait_name,
                                    trait_name_display,
                                    impl_kind,
                                    methods,
                                    start_line,
                                    end_line,
                                    index_source,
                                    indexed_at
                                 ) VALUES (
                                    $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, 'syn', NOW()
                                 )",
                            )
                            .bind(entry.crate_version_id)
                            .bind(source_file_id)
                            .bind(extracted_impl.type_name)
                            .bind(extracted_impl.type_name_display)
                            .bind(extracted_impl.trait_name)
                            .bind(extracted_impl.trait_name_display)
                            .bind(extracted_impl.impl_kind)
                            .bind(extracted_impl.methods)
                            .bind(extracted_impl.start_line)
                            .bind(extracted_impl.end_line)
                            .execute(&self.state.db)
                            .await
                            .map_err(|e| {
                                format!(
                                    "failed to insert crate impl for {} in {}@{}: {e}",
                                    file.relative_path, entry.crate_name, entry.version
                                )
                            })?;
                        }
                    }
                }

                outcome.upserted_files += affected as usize;
            }

            let deleted = if seen_paths.is_empty() {
                sqlx::query(
                    "DELETE FROM symbols
                     WHERE crate_version_id = $1
                       AND source_file_id IN (
                         SELECT id FROM source_files WHERE crate_version_id = $1
                       )",
                )
                .bind(entry.crate_version_id)
                .execute(&self.state.db)
                .await
                .map_err(|e| {
                    format!(
                        "failed to prune symbols for {}@{}: {e}",
                        entry.crate_name, entry.version
                    )
                })?;

                sqlx::query(
                    "DELETE FROM crate_types
                                         WHERE crate_version_id = $1
                                             AND source_file_id IN (
                                                 SELECT id FROM source_files WHERE \
                     crate_version_id = $1
                                             )",
                )
                .bind(entry.crate_version_id)
                .execute(&self.state.db)
                .await
                .map_err(|e| {
                    format!(
                        "failed to prune crate types for {}@{}: {e}",
                        entry.crate_name, entry.version
                    )
                })?;

                sqlx::query(
                    "DELETE FROM crate_impls
                                         WHERE crate_version_id = $1
                                             AND source_file_id IN (
                                                 SELECT id FROM source_files WHERE \
                     crate_version_id = $1
                                             )",
                )
                .bind(entry.crate_version_id)
                .execute(&self.state.db)
                .await
                .map_err(|e| {
                    format!(
                        "failed to prune crate impls for {}@{}: {e}",
                        entry.crate_name, entry.version
                    )
                })?;

                sqlx::query("DELETE FROM source_files WHERE crate_version_id = $1")
                    .bind(entry.crate_version_id)
                    .execute(&self.state.db)
                    .await
                    .map_err(|e| {
                        format!(
                            "failed to prune source files for {}@{}: {e}",
                            entry.crate_name, entry.version
                        )
                    })?
                    .rows_affected()
            } else {
                sqlx::query(
                    "DELETE FROM symbols
                     WHERE crate_version_id = $1
                       AND source_file_id IN (
                         SELECT id
                         FROM source_files
                         WHERE crate_version_id = $1
                           AND NOT (path = ANY($2::TEXT[]))
                       )",
                )
                .bind(entry.crate_version_id)
                .bind(&seen_paths)
                .execute(&self.state.db)
                .await
                .map_err(|e| {
                    format!(
                        "failed to prune stale symbols for {}@{}: {e}",
                        entry.crate_name, entry.version
                    )
                })?;

                sqlx::query(
                    "DELETE FROM crate_types
                                         WHERE crate_version_id = $1
                                             AND source_file_id IN (
                                                 SELECT id
                                                 FROM source_files
                                                 WHERE crate_version_id = $1
                                                     AND NOT (path = ANY($2::TEXT[]))
                                             )",
                )
                .bind(entry.crate_version_id)
                .bind(&seen_paths)
                .execute(&self.state.db)
                .await
                .map_err(|e| {
                    format!(
                        "failed to prune stale crate types for {}@{}: {e}",
                        entry.crate_name, entry.version
                    )
                })?;

                sqlx::query(
                    "DELETE FROM crate_impls
                                         WHERE crate_version_id = $1
                                             AND source_file_id IN (
                                                 SELECT id
                                                 FROM source_files
                                                 WHERE crate_version_id = $1
                                                     AND NOT (path = ANY($2::TEXT[]))
                                             )",
                )
                .bind(entry.crate_version_id)
                .bind(&seen_paths)
                .execute(&self.state.db)
                .await
                .map_err(|e| {
                    format!(
                        "failed to prune stale crate impls for {}@{}: {e}",
                        entry.crate_name, entry.version
                    )
                })?;

                sqlx::query(
                    "DELETE FROM source_files
                     WHERE crate_version_id = $1
                       AND NOT (path = ANY($2::TEXT[]))",
                )
                .bind(entry.crate_version_id)
                .bind(&seen_paths)
                .execute(&self.state.db)
                .await
                .map_err(|e| {
                    format!(
                        "failed to prune stale source files for {}@{}: {e}",
                        entry.crate_name, entry.version
                    )
                })?
                .rows_affected()
            };

            outcome.deleted_files += deleted as usize;
            outcome.scanned_versions += 1;
            outcome
                .touched_versions
                .push(format!("{}@{}", entry.crate_name, entry.version));
        }

        Ok(outcome)
    }
}
