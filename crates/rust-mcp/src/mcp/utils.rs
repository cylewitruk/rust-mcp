use serde_json::Value;

use super::models::CrateSearchRow;

pub(crate) const DEFAULT_SYNC_QUERY: &str = "rust";

pub(crate) fn normalize_optional(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

pub(crate) fn normalize_required(value: String, field: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{field} must not be empty"));
    }
    Ok(trimmed.to_string())
}

pub(crate) fn search_limit(value: Option<u32>) -> u32 {
    value
        .unwrap_or(10)
        .clamp(1, 50)
}

pub(crate) fn source_search_limit(value: Option<u32>) -> u32 {
    value
        .unwrap_or(20)
        .clamp(1, 100)
}

pub(crate) fn symbol_search_limit(value: Option<u32>) -> u32 {
    value
        .unwrap_or(25)
        .clamp(1, 200)
}

pub(crate) fn docs_search_limit(value: Option<u32>) -> u32 {
    value
        .unwrap_or(25)
        .clamp(1, 200)
}

pub(crate) fn source_read_end_line(value: Option<u32>) -> u32 {
    value
        .unwrap_or(200)
        .clamp(1, 2_000)
}

pub(crate) fn path_glob_to_like(glob: &str) -> String {
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

pub(crate) fn version_limit(value: Option<u32>) -> u32 {
    value
        .unwrap_or(100)
        .clamp(1, 500)
}

pub(crate) fn dependents_limit(value: Option<u32>) -> u32 {
    value
        .unwrap_or(25)
        .clamp(1, 200)
}

pub(crate) fn sync_page(value: Option<u32>) -> u32 {
    value.unwrap_or(1).max(1)
}

pub(crate) fn sync_per_page(value: Option<u32>) -> u32 {
    value
        .unwrap_or(25)
        .clamp(1, 100)
}

pub(crate) fn graph_depth(value: Option<u32>) -> u32 {
    value.unwrap_or(1).clamp(1, 4)
}

pub(crate) fn api_diff_limit(value: Option<u32>) -> u32 {
    value
        .unwrap_or(500)
        .clamp(1, 2_000)
}

pub(crate) fn alternatives_limit(value: Option<u32>) -> u32 {
    value
        .unwrap_or(10)
        .clamp(1, 50)
}

pub(crate) fn hotspots_limit(value: Option<u32>) -> u32 {
    value
        .unwrap_or(100)
        .clamp(1, 500)
}

pub(crate) fn crate_api_limit(value: Option<u32>) -> u32 {
    value
        .unwrap_or(200)
        .clamp(1, 1_000)
}

pub(crate) fn trait_impls_limit(value: Option<u32>) -> u32 {
    value
        .unwrap_or(200)
        .clamp(1, 1_000)
}

pub(crate) fn dependency_resolve_limit(value: Option<u32>) -> u32 {
    value
        .unwrap_or(100)
        .clamp(1, 1_000)
}

pub(crate) fn compatibility_matrix_version_limit(value: Option<u32>) -> u32 {
    value
        .unwrap_or(5)
        .clamp(1, 20)
}

pub(crate) fn compatibility_matrix_max_pairs(value: Option<u32>) -> u32 {
    value
        .unwrap_or(25)
        .clamp(1, 200)
}

pub(crate) fn usage_patterns_limit(value: Option<u32>) -> u32 {
    value
        .unwrap_or(20)
        .clamp(1, 200)
}

pub(crate) fn re_exports_limit(value: Option<u32>) -> u32 {
    value
        .unwrap_or(200)
        .clamp(1, 1_000)
}

pub(crate) fn error_types_limit(value: Option<u32>) -> u32 {
    value
        .unwrap_or(100)
        .clamp(1, 500)
}

pub(crate) fn feature_impact_heavy_threshold(value: Option<u32>) -> u32 {
    value
        .unwrap_or(5)
        .clamp(1, 100)
}

pub(crate) fn readme_limit(value: Option<u32>) -> usize {
    value
        .unwrap_or(25_000)
        .clamp(500, 200_000) as usize
}

pub(crate) fn dedupe_strings(mut values: Vec<String>) -> Vec<String> {
    values = values
        .into_iter()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .collect::<Vec<_>>();
    values.sort();
    values.dedup();
    values
}

pub(crate) fn value_to_string_vec(value: &Value) -> Vec<String> {
    match value {
        Value::Array(items) => items
            .iter()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

pub(crate) fn truncate_optional_text(
    value: Option<String>,
    max_chars: usize,
) -> (Option<String>, bool) {
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

pub(crate) fn match_reasons(
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
}
