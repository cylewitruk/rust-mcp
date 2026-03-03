use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::models::{CrateSearchRow, ResponseFreshnessSource};

/// Default search query used for crate sync operations.
pub const DEFAULT_SYNC_QUERY: &str = "rust";

/// Trims and returns `None` for empty or whitespace-only strings.
pub fn normalize_optional(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Trims whitespace and returns an error if the result is empty.
pub fn normalize_required(value: String, field: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{field} must not be empty"));
    }
    Ok(trimmed.to_string())
}

/// Clamps a crate search result limit (default 10, range 1..=50).
pub fn search_limit(value: Option<u32>) -> u32 {
    value
        .unwrap_or(10)
        .clamp(1, 50)
}

/// Clamps a source search result limit (default 20, range 1..=100).
pub fn source_search_limit(value: Option<u32>) -> u32 {
    value
        .unwrap_or(20)
        .clamp(1, 100)
}

/// Clamps a symbol search result limit (default 25, range 1..=200).
pub fn symbol_search_limit(value: Option<u32>) -> u32 {
    value
        .unwrap_or(25)
        .clamp(1, 200)
}

/// Clamps a docs search result limit (default 25, range 1..=200).
pub fn docs_search_limit(value: Option<u32>) -> u32 {
    value
        .unwrap_or(25)
        .clamp(1, 200)
}

/// Clamps a source-read end line (default 200, range 1..=2000).
pub fn source_read_end_line(value: Option<u32>) -> u32 {
    value
        .unwrap_or(200)
        .clamp(1, 2_000)
}

/// Converts a file-path glob pattern to a SQL `LIKE` expression.
pub fn path_glob_to_like(glob: &str) -> String {
    let mut escaped = String::with_capacity(glob.len() + 8);
    for ch in glob.chars() {
        match ch {
            '*' => escaped.push('%'),
            '?' => escaped.push('_'),
            '%' | '_' | '\\' => {
                escaped.push('\\');
                escaped.push(ch);
            }
            _ => escaped.push(ch),
        }
    }
    escaped
}

/// Clamps a version listing limit (default 100, range 1..=500).
pub fn version_limit(value: Option<u32>) -> u32 {
    value
        .unwrap_or(100)
        .clamp(1, 500)
}

/// Clamps a dependents listing limit (default 25, range 1..=200).
pub fn dependents_limit(value: Option<u32>) -> u32 {
    value
        .unwrap_or(25)
        .clamp(1, 200)
}

/// Resolves a sync page number (default 1, minimum 1).
pub fn sync_page(value: Option<u32>) -> u32 {
    value.unwrap_or(1).max(1)
}

/// Clamps a sync per-page size (default 25, range 1..=100).
pub fn sync_per_page(value: Option<u32>) -> u32 {
    value
        .unwrap_or(25)
        .clamp(1, 100)
}

/// Clamps a dependency graph traversal depth (default 1, range 1..=4).
pub fn graph_depth(value: Option<u32>) -> u32 {
    value.unwrap_or(1).clamp(1, 4)
}

/// Clamps an API diff entry limit (default 500, range 1..=2000).
pub fn api_diff_limit(value: Option<u32>) -> u32 {
    value
        .unwrap_or(500)
        .clamp(1, 2_000)
}

/// Clamps an alternatives listing limit (default 10, range 1..=50).
pub fn alternatives_limit(value: Option<u32>) -> u32 {
    value
        .unwrap_or(10)
        .clamp(1, 50)
}

/// Clamps a hotspots listing limit (default 100, range 1..=500).
pub fn hotspots_limit(value: Option<u32>) -> u32 {
    value
        .unwrap_or(100)
        .clamp(1, 500)
}

/// Clamps a crate API surface listing limit (default 200, range 1..=1000).
pub fn crate_api_limit(value: Option<u32>) -> u32 {
    value
        .unwrap_or(200)
        .clamp(1, 1_000)
}

/// Clamps a trait implementations listing limit (default 200, range 1..=1000).
pub fn trait_impls_limit(value: Option<u32>) -> u32 {
    value
        .unwrap_or(200)
        .clamp(1, 1_000)
}

/// Clamps a dependency resolution listing limit (default 100, range 1..=1000).
pub fn dependency_resolve_limit(value: Option<u32>) -> u32 {
    value
        .unwrap_or(100)
        .clamp(1, 1_000)
}

/// Clamps a compatibility matrix version limit (default 5, range 1..=20).
pub fn compatibility_matrix_version_limit(value: Option<u32>) -> u32 {
    value
        .unwrap_or(5)
        .clamp(1, 20)
}

/// Clamps a compatibility matrix pair limit (default 25, range 1..=200).
pub fn compatibility_matrix_max_pairs(value: Option<u32>) -> u32 {
    value
        .unwrap_or(25)
        .clamp(1, 200)
}

/// Clamps a usage-patterns listing limit (default 20, range 1..=200).
pub fn usage_patterns_limit(value: Option<u32>) -> u32 {
    value
        .unwrap_or(20)
        .clamp(1, 200)
}

/// Clamps a re-exports listing limit (default 200, range 1..=1000).
pub fn re_exports_limit(value: Option<u32>) -> u32 {
    value
        .unwrap_or(200)
        .clamp(1, 1_000)
}

/// Clamps an import-path listing limit (default 10, range 1..=100).
pub fn import_path_limit(value: Option<u32>) -> u32 {
    value
        .unwrap_or(10)
        .clamp(1, 100)
}

/// Clamps an error-types listing limit (default 100, range 1..=500).
pub fn error_types_limit(value: Option<u32>) -> u32 {
    value
        .unwrap_or(100)
        .clamp(1, 500)
}

/// Clamps the heavy-feature threshold for feature-impact analysis (default 5,
/// range 1..=100).
pub fn feature_impact_heavy_threshold(value: Option<u32>) -> u32 {
    value
        .unwrap_or(5)
        .clamp(1, 100)
}

/// Clamps a readme character limit (default 25000, range 500..=200000).
pub fn readme_limit(value: Option<u32>) -> usize {
    value
        .unwrap_or(25_000)
        .clamp(500, 200_000) as usize
}

/// Trims, deduplicates, and sorts a list of strings.
pub fn dedupe_strings(mut values: Vec<String>) -> Vec<String> {
    values = values
        .into_iter()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .collect::<Vec<_>>();
    values.sort();
    values.dedup();
    values
}

/// Extracts string elements from a JSON array, ignoring non-string values.
pub fn value_to_string_vec(value: &Value) -> Vec<String> {
    match value {
        Value::Array(items) => items
            .iter()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

/// Truncates optional text to `max_chars`, returning whether truncation
/// occurred.
pub fn truncate_optional_text(value: Option<String>, max_chars: usize) -> (Option<String>, bool) {
    let Some(text) = value else {
        return (None, false);
    };

    let mut chars = text.chars();
    let truncated = chars
        .by_ref()
        .take(max_chars)
        .collect::<String>();
    let was_truncated = chars.next().is_some();
    (Some(truncated), was_truncated)
}

/// Determines which fields of a crate search row match the given query/filter.
pub fn match_reasons(
    row: &CrateSearchRow,
    query: Option<&str>,
    category: Option<&str>,
    keyword: Option<&str>,
) -> Vec<String> {
    let mut reasons = Vec::new();

    if let Some(q) = query {
        let q_lower = q.to_ascii_lowercase();
        if row
            .name
            .to_ascii_lowercase()
            .contains(&q_lower)
        {
            reasons.push("name_match".to_string());
        }
        if row
            .description
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .contains(&q_lower)
        {
            reasons.push("description_match".to_string());
        }
        if row.keywords.iter().any(|k| {
            k.eq_ignore_ascii_case(q)
                || k.to_ascii_lowercase()
                    .contains(&q_lower)
        }) {
            reasons.push("keyword_match".to_string());
        }
        if row
            .categories
            .iter()
            .any(|c| {
                c.eq_ignore_ascii_case(q)
                    || c.to_ascii_lowercase()
                        .contains(&q_lower)
            })
        {
            reasons.push("category_match".to_string());
        }
    }

    if let Some(c) = category
        && row
            .categories
            .iter()
            .any(|v| v.eq_ignore_ascii_case(c))
    {
        reasons.push("category_filter_match".to_string());
    }

    if let Some(k) = keyword
        && row
            .keywords
            .iter()
            .any(|v| v.eq_ignore_ascii_case(k))
    {
        reasons.push("keyword_filter_match".to_string());
    }

    reasons
}

// ---------------------------------------------------------------------------
// Generic cursor encoding / decoding
// ---------------------------------------------------------------------------

/// Trait implemented by all cursor token structs.
/// Provides common validation (version check, limit > 0).
pub trait CursorToken: Serialize + DeserializeOwned {
    /// Token schema version (must be 1 for current cursors).
    fn version(&self) -> u8;
    /// Number of items per page.
    fn limit(&self) -> u32;
    /// Zero-based item offset.
    fn offset(&self) -> u32;
}

/// Decode a cursor token from its base64url-encoded string representation.
/// Validates version == 1 and limit > 0.
pub fn decode_cursor<T: CursorToken>(token: &str) -> Result<T, String> {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(token)
        .map_err(|e| format!("cursor is invalid (base64 decode failed: {e})"))?;
    let decoded = serde_json::from_slice::<T>(&bytes)
        .map_err(|e| format!("cursor is invalid (deserialization failed: {e})"))?;

    if decoded.version() != 1 {
        return Err("cursor version is not supported".to_string());
    }
    if decoded.limit() == 0 {
        return Err("cursor is invalid".to_string());
    }

    Ok(decoded)
}

/// Encode a cursor token to its base64url string representation.
pub fn encode_cursor<T: CursorToken>(token: &T) -> Result<String, String> {
    let bytes =
        serde_json::to_vec(token).map_err(|e| format!("cursor serialization failed: {e}"))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

// ---------------------------------------------------------------------------
// Pagination resolution
// ---------------------------------------------------------------------------

/// Resolved pagination parameters for a tool query.
#[derive(Debug)]
pub struct PaginationParams {
    /// Zero-based item offset into the result set.
    pub offset: u32,
    /// Maximum items to return.
    pub limit: u32,
    /// One-based page number corresponding to the offset.
    pub effective_page: u32,
}

/// Resolve pagination from a decoded cursor token or from page + limit params.
///
/// If `cursor_token` is provided, extracts offset/limit from it and validates
/// that the caller's `requested_limit` matches (if the caller provided an
/// explicit limit). Otherwise computes offset from `page` and
/// `requested_limit`.
pub fn resolve_pagination<T: CursorToken>(
    cursor_token: Option<&T>,
    explicit_limit_provided: bool,
    requested_limit: u32,
    page: u32,
) -> Result<PaginationParams, String> {
    if let Some(token) = cursor_token {
        if explicit_limit_provided && requested_limit != token.limit() {
            return Err("limit must match the cursor page size".to_string());
        }
        Ok(PaginationParams {
            offset: token.offset(),
            limit: token.limit(),
            effective_page: (token.offset() / token.limit()).saturating_add(1),
        })
    } else {
        Ok(PaginationParams {
            offset: (page - 1) * requested_limit,
            limit: requested_limit,
            effective_page: page,
        })
    }
}

// ---------------------------------------------------------------------------
// Pagination truncation + next cursor
// ---------------------------------------------------------------------------

/// Result of applying a pagination limit to a result set.
pub struct PaginatedResult<T> {
    /// Items for the current page.
    pub items: Vec<T>,
    /// Whether additional pages exist beyond the current one.
    pub has_more: bool,
    /// Opaque cursor token for fetching the next page, if any.
    pub next_cursor: Option<String>,
}

/// Truncate `items` to `limit`, compute `has_more`, and encode `next_cursor`
/// if there are more results.
///
/// `build_next_token` is called only when `has_more` is true, receiving
/// the next offset. It should return a cursor token struct for encoding.
pub fn apply_pagination_limit<T, C: CursorToken>(
    mut items: Vec<T>,
    limit: u32,
    offset: u32,
    build_next_token: impl FnOnce(u32) -> C,
) -> Result<PaginatedResult<T>, String> {
    let has_more = items.len() > limit as usize;
    if has_more {
        items.truncate(limit as usize);
    }
    let next_cursor = if has_more {
        Some(encode_cursor(&build_next_token(offset.saturating_add(limit)))?)
    } else {
        None
    };
    Ok(PaginatedResult { items, has_more, next_cursor })
}

// ---------------------------------------------------------------------------
// Freshness source array builder
// ---------------------------------------------------------------------------

/// Build the standard two-entry freshness source array used by crate-scoped
/// tools.
pub fn build_crate_freshness_sources(
    local_checked_at: Option<String>,
    crates_io_status: &str,
) -> Vec<ResponseFreshnessSource> {
    vec![
        ResponseFreshnessSource {
            source: "local_postgres_index".to_string(),
            status: "fresh".to_string(),
            checked_at: local_checked_at,
        },
        ResponseFreshnessSource {
            source: "crates.io".to_string(),
            status: crates_io_status.to_string(),
            checked_at: None,
        },
    ]
}

/// Resolves the on-disk source directory for a crate version.
///
/// Checks two locations in order:
/// 1. Host cargo registry: `{cargo_registry_dir}/src/*/{crate_name}-{version}`
/// 2. Downloaded cache: `{crate_source_cache_dir}/{crate_name}-{version}`
///
/// Returns `None` if no matching directory exists in either location.
pub fn resolve_registry_source_dir(
    cargo_registry_dir: &Path,
    crate_name: &str,
    version: &str,
) -> Option<PathBuf> {
    resolve_source_dir(cargo_registry_dir, None, crate_name, version)
}

/// Resolves the on-disk source directory for a crate version, also checking
/// the downloaded crate source cache.
pub fn resolve_source_dir(
    cargo_registry_dir: &Path,
    crate_source_cache_dir: Option<&Path>,
    crate_name: &str,
    version: &str,
) -> Option<PathBuf> {
    let target_dir_name = format!("{crate_name}-{version}");

    // 1. Host cargo registry.
    let src_root = cargo_registry_dir.join("src");
    if let Ok(registries) = std::fs::read_dir(&src_root) {
        for entry in registries.flatten() {
            if !entry
                .file_type()
                .ok()
                .is_some_and(|ft| ft.is_dir())
            {
                continue;
            }
            let candidate = entry
                .path()
                .join(&target_dir_name);
            if candidate.is_dir() {
                return Some(candidate);
            }
        }
    }

    // 2. Downloaded crate source cache.
    if let Some(cache_dir) = crate_source_cache_dir {
        let candidate = cache_dir.join(&target_dir_name);
        if candidate.is_dir() {
            return Some(candidate);
        }
    }

    None
}

/// Reads a single source file from the cargo registry or download cache.
///
/// Combines [`resolve_source_dir`] with a `std::fs::read_to_string`
/// call. Returns `None` if the directory or file does not exist,
/// or if the content is not valid UTF-8.
pub fn read_source_file_from_disk(
    cargo_registry_dir: &Path,
    crate_name: &str,
    version: &str,
    relative_path: &str,
) -> Option<String> {
    let version_dir = resolve_registry_source_dir(cargo_registry_dir, crate_name, version)?;
    let full_path = version_dir.join(relative_path);
    std::fs::read_to_string(full_path).ok()
}

/// Reads a single source file, also checking the download cache.
pub fn read_source_file_from_disk_or_cache(
    cargo_registry_dir: &Path,
    crate_source_cache_dir: Option<&Path>,
    crate_name: &str,
    version: &str,
    relative_path: &str,
) -> Option<String> {
    let version_dir =
        resolve_source_dir(cargo_registry_dir, crate_source_cache_dir, crate_name, version)?;
    let full_path = version_dir.join(relative_path);
    std::fs::read_to_string(full_path).ok()
}

/// Lists all text source file paths under a resolved registry directory.
///
/// Walks the directory tree and returns relative paths for files with
/// recognized text extensions.
pub fn list_source_file_paths_on_disk(version_dir: &Path) -> Vec<String> {
    fn is_text_extension(ext: Option<&OsStr>) -> bool {
        matches!(
            ext.and_then(OsStr::to_str),
            Some("rs" | "toml" | "md" | "json" | "yaml" | "yml" | "txt")
        )
    }

    let mut paths = Vec::new();
    let mut pending = vec![version_dir.to_path_buf()];

    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.is_file()
                && is_text_extension(path.extension())
                && let Ok(relative) = path.strip_prefix(version_dir)
            {
                paths.push(
                    relative
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
    }

    paths.sort();
    paths
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    // --- normalize_optional ---

    #[test]
    fn normalize_optional_trims_whitespace() {
        assert_eq!(normalize_optional(Some("  hello  ".to_string())), Some("hello".to_string()));
    }

    #[test]
    fn normalize_optional_returns_none_for_empty() {
        assert_eq!(normalize_optional(Some("".to_string())), None);
    }

    #[test]
    fn normalize_optional_returns_none_for_whitespace_only() {
        assert_eq!(normalize_optional(Some("   ".to_string())), None);
    }

    #[test]
    fn normalize_optional_passes_through_none() {
        assert_eq!(normalize_optional(None), None);
    }

    // --- normalize_required ---

    #[test]
    fn normalize_required_trims_and_returns_ok() {
        assert_eq!(
            normalize_required("  serde  ".to_string(), "crate_name"),
            Ok("serde".to_string())
        );
    }

    #[test]
    fn normalize_required_errors_on_empty() {
        let result = normalize_required("".to_string(), "crate_name");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .contains("crate_name")
        );
    }

    #[test]
    fn normalize_required_errors_on_whitespace_only() {
        assert!(normalize_required("   ".to_string(), "field").is_err());
    }

    // --- search_limit (representative clamp function) ---

    #[test]
    fn search_limit_defaults_to_10() {
        assert_eq!(search_limit(None), 10);
    }

    #[test]
    fn search_limit_clamps_to_min() {
        assert_eq!(search_limit(Some(0)), 1);
    }

    #[test]
    fn search_limit_clamps_to_max() {
        assert_eq!(search_limit(Some(999)), 50);
    }

    #[test]
    fn search_limit_passes_through_valid() {
        assert_eq!(search_limit(Some(25)), 25);
    }

    // --- sync_page (uses max instead of clamp) ---

    #[test]
    fn sync_page_defaults_to_1() {
        assert_eq!(sync_page(None), 1);
    }

    #[test]
    fn sync_page_clamps_zero_to_1() {
        assert_eq!(sync_page(Some(0)), 1);
    }

    #[test]
    fn sync_page_allows_large_values() {
        assert_eq!(sync_page(Some(500)), 500);
    }

    // --- path_glob_to_like ---

    #[test]
    fn glob_star_becomes_percent() {
        assert_eq!(path_glob_to_like("src/*.rs"), "src/%.rs");
    }

    #[test]
    fn glob_question_becomes_underscore() {
        assert_eq!(path_glob_to_like("lib?.rs"), "lib_.rs");
    }

    #[test]
    fn glob_escapes_sql_percent() {
        assert_eq!(path_glob_to_like("100%"), "100\\%");
    }

    #[test]
    fn glob_escapes_sql_underscore() {
        assert_eq!(path_glob_to_like("my_file"), "my\\_file");
    }

    #[test]
    fn glob_escapes_backslash() {
        assert_eq!(path_glob_to_like("a\\b"), "a\\\\b");
    }

    #[test]
    fn glob_double_star() {
        assert_eq!(path_glob_to_like("src/**/*.rs"), "src/%%/%.rs");
    }

    #[test]
    fn glob_plain_text_unchanged() {
        assert_eq!(path_glob_to_like("src/main.rs"), "src/main.rs");
    }

    #[test]
    fn glob_empty_string() {
        assert_eq!(path_glob_to_like(""), "");
    }

    // --- dedupe_strings ---

    #[test]
    fn dedupe_removes_duplicates() {
        let input = vec!["a".into(), "b".into(), "a".into()];
        assert_eq!(dedupe_strings(input), vec!["a", "b"]);
    }

    #[test]
    fn dedupe_trims_and_filters_empty() {
        let input = vec!["  hello  ".into(), "".into(), "  ".into(), "hello".into()];
        assert_eq!(dedupe_strings(input), vec!["hello"]);
    }

    #[test]
    fn dedupe_sorts_output() {
        let input = vec!["z".into(), "a".into(), "m".into()];
        assert_eq!(dedupe_strings(input), vec!["a", "m", "z"]);
    }

    #[test]
    fn dedupe_empty_input() {
        let input: Vec<String> = vec![];
        let result: Vec<String> = vec![];
        assert_eq!(dedupe_strings(input), result);
    }

    // --- value_to_string_vec ---

    #[test]
    fn value_to_string_vec_extracts_strings() {
        let v = json!(["a", "b", "c"]);
        assert_eq!(value_to_string_vec(&v), vec!["a", "b", "c"]);
    }

    #[test]
    fn value_to_string_vec_skips_non_strings() {
        let v = json!(["a", 1, null, "b"]);
        assert_eq!(value_to_string_vec(&v), vec!["a", "b"]);
    }

    #[test]
    fn value_to_string_vec_returns_empty_for_non_array() {
        assert!(value_to_string_vec(&json!("hello")).is_empty());
        assert!(value_to_string_vec(&json!(42)).is_empty());
        assert!(value_to_string_vec(&json!(null)).is_empty());
        assert!(value_to_string_vec(&json!({})).is_empty());
    }

    #[test]
    fn value_to_string_vec_empty_array() {
        assert!(value_to_string_vec(&json!([])).is_empty());
    }

    // --- truncate_optional_text ---

    #[test]
    fn truncate_none_returns_none_not_truncated() {
        let (text, was_truncated) = truncate_optional_text(None, 100);
        assert!(text.is_none());
        assert!(!was_truncated);
    }

    #[test]
    fn truncate_short_text_unchanged() {
        let (text, was_truncated) = truncate_optional_text(Some("hello".into()), 100);
        assert_eq!(text.unwrap(), "hello");
        assert!(!was_truncated);
    }

    #[test]
    fn truncate_exact_length_not_truncated() {
        let (text, was_truncated) = truncate_optional_text(Some("hello".into()), 5);
        assert_eq!(text.unwrap(), "hello");
        assert!(!was_truncated);
    }

    #[test]
    fn truncate_long_text_truncated() {
        let (text, was_truncated) = truncate_optional_text(Some("hello world".into()), 5);
        assert_eq!(text.unwrap(), "hello");
        assert!(was_truncated);
    }

    #[test]
    fn truncate_handles_multibyte_chars() {
        // "café" is 4 chars but 5 bytes; truncation should work on chars
        let (text, was_truncated) = truncate_optional_text(Some("café!".into()), 4);
        assert_eq!(text.unwrap(), "café");
        assert!(was_truncated);
    }

    // --- match_reasons ---

    fn make_search_row(
        name: &str,
        description: Option<&str>,
        keywords: Vec<&str>,
        categories: Vec<&str>,
    ) -> CrateSearchRow {
        CrateSearchRow {
            crate_id: 1,
            name: name.to_string(),
            description: description.map(ToString::to_string),
            repository_url: None,
            docs_url: None,
            homepage_url: None,
            categories: categories
                .into_iter()
                .map(ToString::to_string)
                .collect(),
            keywords: keywords
                .into_iter()
                .map(ToString::to_string)
                .collect(),
            total_downloads: 0,
            latest_version: None,
            latest_published_at: None,
            dependent_count: 0,
            relevance_score: 0.0,
        }
    }

    #[test]
    fn match_reasons_name_match() {
        let row = make_search_row("serde", None, vec![], vec![]);
        let reasons = match_reasons(&row, Some("serde"), None, None);
        assert!(reasons.contains(&"name_match".to_string()));
    }

    #[test]
    fn match_reasons_name_match_case_insensitive() {
        let row = make_search_row("Serde", None, vec![], vec![]);
        let reasons = match_reasons(&row, Some("serde"), None, None);
        assert!(reasons.contains(&"name_match".to_string()));
    }

    #[test]
    fn match_reasons_description_match() {
        let row = make_search_row("foo", Some("A serialization framework"), vec![], vec![]);
        let reasons = match_reasons(&row, Some("serialization"), None, None);
        assert!(reasons.contains(&"description_match".to_string()));
        assert!(!reasons.contains(&"name_match".to_string()));
    }

    #[test]
    fn match_reasons_keyword_match() {
        let row = make_search_row("foo", None, vec!["json", "parser"], vec![]);
        let reasons = match_reasons(&row, Some("json"), None, None);
        assert!(reasons.contains(&"keyword_match".to_string()));
    }

    #[test]
    fn match_reasons_category_match() {
        let row = make_search_row("foo", None, vec![], vec!["web-programming"]);
        let reasons = match_reasons(&row, Some("web"), None, None);
        assert!(reasons.contains(&"category_match".to_string()));
    }

    #[test]
    fn match_reasons_category_filter() {
        let row = make_search_row("foo", None, vec![], vec!["web-programming"]);
        let reasons = match_reasons(&row, None, Some("web-programming"), None);
        assert!(reasons.contains(&"category_filter_match".to_string()));
    }

    #[test]
    fn match_reasons_keyword_filter() {
        let row = make_search_row("foo", None, vec!["json"], vec![]);
        let reasons = match_reasons(&row, None, None, Some("json"));
        assert!(reasons.contains(&"keyword_filter_match".to_string()));
    }

    #[test]
    fn match_reasons_no_match() {
        let row = make_search_row("foo", Some("bar"), vec!["baz"], vec!["qux"]);
        let reasons = match_reasons(&row, Some("zzz"), None, None);
        assert!(reasons.is_empty());
    }

    #[test]
    fn match_reasons_no_query_no_filters() {
        let row = make_search_row("foo", None, vec![], vec![]);
        let reasons = match_reasons(&row, None, None, None);
        assert!(reasons.is_empty());
    }

    #[test]
    fn match_reasons_multiple_matches() {
        let row =
            make_search_row("serde", Some("serde serialization"), vec!["serde"], vec!["encoding"]);
        let reasons = match_reasons(&row, Some("serde"), None, None);
        assert!(reasons.contains(&"name_match".to_string()));
        assert!(reasons.contains(&"description_match".to_string()));
        assert!(reasons.contains(&"keyword_match".to_string()));
    }

    // --- CursorToken + encode/decode ---

    #[derive(Debug, serde::Serialize, serde::Deserialize, PartialEq)]
    struct TestCursorToken {
        v: u8,
        offset: u32,
        limit: u32,
        name: String,
    }

    impl CursorToken for TestCursorToken {
        fn version(&self) -> u8 {
            self.v
        }
        fn limit(&self) -> u32 {
            self.limit
        }
        fn offset(&self) -> u32 {
            self.offset
        }
    }

    #[test]
    fn cursor_roundtrip() {
        let token = TestCursorToken {
            v: 1,
            offset: 20,
            limit: 10,
            name: "serde".to_string(),
        };
        let encoded = encode_cursor(&token).unwrap();
        let decoded: TestCursorToken = decode_cursor(&encoded).unwrap();
        assert_eq!(decoded, token);
    }

    #[test]
    fn cursor_decode_bad_base64() {
        let result = decode_cursor::<TestCursorToken>("!!!not-base64!!!");
        let err = result.unwrap_err();
        assert!(
            err.starts_with("cursor is invalid (base64 decode failed:"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn cursor_decode_bad_json() {
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"not json");
        let result = decode_cursor::<TestCursorToken>(&encoded);
        let err = result.unwrap_err();
        assert!(
            err.starts_with("cursor is invalid (deserialization failed:"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn cursor_decode_wrong_version() {
        let token = TestCursorToken {
            v: 99,
            offset: 0,
            limit: 10,
            name: "x".to_string(),
        };
        let encoded = encode_cursor(&token).unwrap();
        let result = decode_cursor::<TestCursorToken>(&encoded);
        assert_eq!(result.unwrap_err(), "cursor version is not supported");
    }

    #[test]
    fn cursor_decode_zero_limit() {
        let token = TestCursorToken {
            v: 1,
            offset: 0,
            limit: 0,
            name: "x".to_string(),
        };
        // Encode raw to bypass any construction validation
        let bytes = serde_json::to_vec(&token).unwrap();
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
        let result = decode_cursor::<TestCursorToken>(&encoded);
        assert_eq!(result.unwrap_err(), "cursor is invalid");
    }

    // --- resolve_pagination ---

    #[test]
    fn resolve_pagination_from_cursor() {
        let token = TestCursorToken {
            v: 1,
            offset: 30,
            limit: 10,
            name: "x".to_string(),
        };
        let params = resolve_pagination(Some(&token), false, 10, 1).unwrap();
        assert_eq!(params.offset, 30);
        assert_eq!(params.limit, 10);
        assert_eq!(params.effective_page, 4);
    }

    #[test]
    fn resolve_pagination_from_page() {
        let params = resolve_pagination::<TestCursorToken>(None, false, 25, 3).unwrap();
        assert_eq!(params.offset, 50);
        assert_eq!(params.limit, 25);
        assert_eq!(params.effective_page, 3);
    }

    #[test]
    fn resolve_pagination_limit_mismatch() {
        let token = TestCursorToken {
            v: 1,
            offset: 10,
            limit: 10,
            name: "x".to_string(),
        };
        let result = resolve_pagination(Some(&token), true, 20, 1);
        assert_eq!(result.unwrap_err(), "limit must match the cursor page size");
    }

    #[test]
    fn resolve_pagination_limit_match_ok() {
        let token = TestCursorToken {
            v: 1,
            offset: 10,
            limit: 10,
            name: "x".to_string(),
        };
        let params = resolve_pagination(Some(&token), true, 10, 1).unwrap();
        assert_eq!(params.limit, 10);
    }

    // --- apply_pagination_limit ---

    #[test]
    fn apply_pagination_no_truncation() {
        let items = vec![1, 2, 3];
        let result = apply_pagination_limit(items, 5, 0, |next_offset| TestCursorToken {
            v: 1,
            offset: next_offset,
            limit: 5,
            name: "x".to_string(),
        })
        .unwrap();
        assert_eq!(result.items, vec![1, 2, 3]);
        assert!(!result.has_more);
        assert!(result.next_cursor.is_none());
    }

    #[test]
    fn apply_pagination_with_truncation() {
        let items = vec![1, 2, 3, 4, 5, 6];
        let result = apply_pagination_limit(items, 5, 10, |next_offset| TestCursorToken {
            v: 1,
            offset: next_offset,
            limit: 5,
            name: "x".to_string(),
        })
        .unwrap();
        assert_eq!(result.items, vec![1, 2, 3, 4, 5]);
        assert!(result.has_more);
        assert!(result.next_cursor.is_some());

        let decoded: TestCursorToken = decode_cursor(
            result
                .next_cursor
                .as_ref()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(decoded.offset, 15);
        assert_eq!(decoded.limit, 5);
    }

    #[test]
    fn apply_pagination_exact_limit() {
        let items = vec![1, 2, 3, 4, 5];
        let result = apply_pagination_limit(items, 5, 0, |next_offset| TestCursorToken {
            v: 1,
            offset: next_offset,
            limit: 5,
            name: "x".to_string(),
        })
        .unwrap();
        assert_eq!(result.items.len(), 5);
        assert!(!result.has_more);
        assert!(result.next_cursor.is_none());
    }

    #[test]
    fn apply_pagination_empty() {
        let items: Vec<i32> = vec![];
        let result = apply_pagination_limit(items, 10, 0, |next_offset| TestCursorToken {
            v: 1,
            offset: next_offset,
            limit: 10,
            name: "x".to_string(),
        })
        .unwrap();
        assert!(result.items.is_empty());
        assert!(!result.has_more);
        assert!(result.next_cursor.is_none());
    }

    // --- build_crate_freshness_sources ---

    #[test]
    fn freshness_sources_shape() {
        let sources = build_crate_freshness_sources(Some("2025-01-01".to_string()), "fresh");
        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].source, "local_postgres_index");
        assert_eq!(sources[0].status, "fresh");
        assert_eq!(sources[0].checked_at, Some("2025-01-01".to_string()));
        assert_eq!(sources[1].source, "crates.io");
        assert_eq!(sources[1].status, "fresh");
        assert!(
            sources[1]
                .checked_at
                .is_none()
        );
    }

    #[test]
    fn freshness_sources_changed_status() {
        let sources = build_crate_freshness_sources(None, "changed");
        assert_eq!(sources[1].status, "changed");
        assert!(
            sources[0]
                .checked_at
                .is_none()
        );
    }
}
