use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use sha2::{Digest as _, Sha256};
use sqlx::FromRow;

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

                outcome.upserted_files += affected as usize;
            }

            let deleted = if seen_paths.is_empty() {
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
