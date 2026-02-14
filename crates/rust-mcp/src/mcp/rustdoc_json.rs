use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use semver::Version;
use serde_json::Value;
use sha2::{Digest as _, Sha256};

use super::models::CrateVersionSelectionRow;
use super::server::McpServer;
use super::utils::{normalize_required, sync_page, sync_per_page};

#[derive(Debug, Default)]
pub(super) struct RustdocJsonRefreshOutcome {
    pub(super) scanned_files: usize,
    pub(super) synced_versions: usize,
    pub(super) symbols_written: usize,
    pub(super) touched_versions: Vec<String>,
    pub(super) errors: Vec<String>,
}

#[derive(Debug)]
struct RustdocCandidate {
    path: PathBuf,
    crate_name: String,
    crate_version: Option<String>,
}

#[derive(Debug)]
struct RustdocSymbol {
    name: String,
    kind: String,
    visibility: Option<String>,
    signature: Option<String>,
    start_line: i32,
    end_line: i32,
}

fn file_sha256_hex(content: &[u8]) -> String {
    let digest = Sha256::digest(content);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
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

fn extract_symbols(document: &Value) -> Vec<RustdocSymbol> {
    let mut symbols = Vec::<RustdocSymbol>::new();

    let index = document
        .get("index")
        .and_then(Value::as_object);
    let Some(index) = index else {
        return symbols;
    };

    for item in index.values() {
        let Some(name) = item
            .get("name")
            .and_then(Value::as_str)
        else {
            continue;
        };
        let trimmed_name = name.trim();
        if trimmed_name.is_empty() {
            continue;
        }

        let kind = item
            .get("kind")
            .and_then(Value::as_str)
            .map(|value| value.to_ascii_lowercase())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "item".to_string());

        let visibility = if let Some(raw) = item.get("visibility") {
            if let Some(value) = raw.as_str() {
                Some(value.to_string())
            } else if raw
                .as_object()
                .is_some_and(|map| map.contains_key("public"))
            {
                Some("public".to_string())
            } else {
                None
            }
        } else {
            None
        };

        let signature = item
            .get("inner")
            .and_then(|inner| inner.get("decl"))
            .map(Value::to_string)
            .filter(|text| !text.trim().is_empty());

        let start_line = item
            .get("span")
            .and_then(|span| span.get("begin"))
            .and_then(|begin| begin.get(0))
            .and_then(Value::as_i64)
            .unwrap_or(1)
            .max(1) as i32;
        let end_line = item
            .get("span")
            .and_then(|span| span.get("end"))
            .and_then(|end| end.get(0))
            .and_then(Value::as_i64)
            .unwrap_or(start_line as i64)
            .max(start_line as i64) as i32;

        symbols.push(RustdocSymbol {
            name: trimmed_name.to_string(),
            kind,
            visibility,
            signature,
            start_line,
            end_line,
        });
    }

    symbols
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

impl McpServer {
    pub(super) async fn sync_rustdoc_json_cache(
        &self,
        crate_name: Option<String>,
        page: Option<u32>,
        per_page: Option<u32>,
    ) -> Result<RustdocJsonRefreshOutcome, String> {
        let crate_filter = match crate_name {
            Some(value) => Some(normalize_required(value, "crate_name")?),
            None => None,
        };

        let Some(root_dir) = self
            .state
            .config
            .rustdoc_json_dir
            .clone()
        else {
            return Ok(RustdocJsonRefreshOutcome {
                errors: vec![
                    "RUSTDOC_JSON_DIR is not configured; skipping rustdoc JSON refresh".to_string(),
                ],
                ..Default::default()
            });
        };

        if !root_dir.exists() {
            return Ok(RustdocJsonRefreshOutcome {
                errors: vec![format!("rustdoc JSON directory not found: {}", root_dir.display())],
                ..Default::default()
            });
        }

        let files = walk_json_files(&root_dir)?;
        let mut candidates = files
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

        candidates.sort_by(|left, right| left.path.cmp(&right.path));

        let page = sync_page(page);
        let per_page = sync_per_page(per_page) as usize;
        let offset = ((page - 1) as usize).saturating_mul(per_page);
        let selected = candidates
            .into_iter()
            .skip(offset)
            .take(per_page)
            .collect::<Vec<_>>();

        let mut outcome = RustdocJsonRefreshOutcome::default();

        for candidate in selected {
            outcome.scanned_files += 1;

            let bytes = match std::fs::read(&candidate.path) {
                Ok(bytes) => bytes,
                Err(error) => {
                    outcome.errors.push(format!(
                        "failed to read rustdoc JSON file {}: {error}",
                        candidate.path.display()
                    ));
                    continue;
                }
            };

            let content = match String::from_utf8(bytes.clone()) {
                Ok(content) => content,
                Err(error) => {
                    outcome.errors.push(format!(
                        "invalid UTF-8 in rustdoc JSON file {}: {error}",
                        candidate.path.display()
                    ));
                    continue;
                }
            };

            let document = match serde_json::from_str::<Value>(&content) {
                Ok(document) => document,
                Err(error) => {
                    outcome.errors.push(format!(
                        "failed to parse rustdoc JSON file {}: {error}",
                        candidate.path.display()
                    ));
                    continue;
                }
            };

            let resolved_crate_name = document
                .get("crate_name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
                .unwrap_or_else(|| candidate.crate_name.clone());

            if let Some(filter) = crate_filter.as_ref()
                && resolved_crate_name != *filter
            {
                continue;
            }

            let metadata_version = document
                .get("crate_version")
                .and_then(Value::as_str)
                .map(ToString::to_string)
                .or(candidate
                    .crate_version
                    .clone());

            let crate_version = if let Some(version) = metadata_version {
                sqlx::query_as::<_, CrateVersionSelectionRow>(
                    "SELECT
                        cv.id,
                        cv.version,
                        cv.rust_version,
                        cv.published_at::TEXT AS published_at,
                        cv.readme
                     FROM crate_versions cv
                     JOIN crates c ON c.id = cv.crate_id
                     WHERE c.name = $1 AND cv.version = $2
                     LIMIT 1",
                )
                .bind(&resolved_crate_name)
                .bind(&version)
                .fetch_optional(&self.state.db)
                .await
                .map_err(|e| {
                    format!(
                        "rustdoc JSON crate version lookup failed for {}@{}: {e}",
                        resolved_crate_name, version
                    )
                })?
            } else {
                sqlx::query_as::<_, CrateVersionSelectionRow>(
                    "SELECT
                        cv.id,
                        cv.version,
                        cv.rust_version,
                        cv.published_at::TEXT AS published_at,
                        cv.readme
                     FROM crate_versions cv
                     JOIN crates c ON c.id = cv.crate_id
                     WHERE c.name = $1
                     ORDER BY cv.published_at DESC NULLS LAST, cv.id DESC
                     LIMIT 1",
                )
                .bind(&resolved_crate_name)
                .fetch_optional(&self.state.db)
                .await
                .map_err(|e| {
                    format!(
                        "rustdoc JSON latest version lookup failed for {}: {e}",
                        resolved_crate_name
                    )
                })?
            };

            let Some(crate_version) = crate_version else {
                outcome.errors.push(format!(
                    "crate/version not indexed for rustdoc JSON file {} (crate '{}')",
                    candidate.path.display(),
                    resolved_crate_name
                ));
                continue;
            };

            let synthetic_path = synthetic_rustdoc_path(&root_dir, &candidate.path);

            sqlx::query(
                "INSERT INTO source_files (
                    crate_version_id,
                    path,
                    sha256,
                    file_size,
                    language,
                    content,
                    indexed_at
                 ) VALUES (
                    $1, $2, $3, $4, $5, $6, NOW()
                 )
                 ON CONFLICT (crate_version_id, path) DO UPDATE
                 SET sha256 = EXCLUDED.sha256,
                     file_size = EXCLUDED.file_size,
                     language = EXCLUDED.language,
                     content = EXCLUDED.content,
                     indexed_at = NOW()",
            )
            .bind(crate_version.id)
            .bind(&synthetic_path)
            .bind(file_sha256_hex(&bytes))
            .bind(bytes.len() as i64)
            .bind(Some("rustdoc_json"))
            .bind(&content)
            .execute(&self.state.db)
            .await
            .map_err(|e| {
                format!(
                    "failed to upsert rustdoc source file {} for {}@{}: {e}",
                    synthetic_path, resolved_crate_name, crate_version.version
                )
            })?;

            let source_file_id = sqlx::query_scalar::<_, i64>(
                "SELECT id
                 FROM source_files
                 WHERE crate_version_id = $1 AND path = $2
                 LIMIT 1",
            )
            .bind(crate_version.id)
            .bind(&synthetic_path)
            .fetch_one(&self.state.db)
            .await
            .map_err(|e| {
                format!(
                    "failed to lookup rustdoc source file id {} for {}@{}: {e}",
                    synthetic_path, resolved_crate_name, crate_version.version
                )
            })?;

            sqlx::query(
                "DELETE FROM symbols
                 WHERE source_file_id = $1
                   AND index_source = 'rustdoc_json'",
            )
            .bind(source_file_id)
            .execute(&self.state.db)
            .await
            .map_err(|e| {
                format!(
                    "failed to clear rustdoc symbols for {}@{}: {e}",
                    resolved_crate_name, crate_version.version
                )
            })?;

            let symbols = extract_symbols(&document);
            for symbol in symbols {
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
                        $1, $2, $3, $4, $5, $6, $7, $8, 'rustdoc_json', NOW()
                     )",
                )
                .bind(crate_version.id)
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
                        "failed to insert rustdoc symbol for {}@{}: {e}",
                        resolved_crate_name, crate_version.version
                    )
                })?;
                outcome.symbols_written += 1;
            }

            outcome.synced_versions += 1;
            outcome
                .touched_versions
                .push(format!("{}@{}", resolved_crate_name, crate_version.version));
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
