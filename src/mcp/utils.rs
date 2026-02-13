use serde_json::Value;

use super::models::CrateSearchRow;

pub(super) const DEFAULT_SYNC_QUERY: &str = "rust";

pub(super) fn normalize_optional(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

pub(super) fn normalize_required(value: String, field: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{field} must not be empty"));
    }
    Ok(trimmed.to_string())
}

pub(super) fn search_limit(value: Option<u32>) -> u32 {
    value
        .unwrap_or(10)
        .clamp(1, 50)
}

pub(super) fn source_search_limit(value: Option<u32>) -> u32 {
    value
        .unwrap_or(20)
        .clamp(1, 100)
}

pub(super) fn symbol_search_limit(value: Option<u32>) -> u32 {
    value
        .unwrap_or(25)
        .clamp(1, 200)
}

pub(super) fn docs_search_limit(value: Option<u32>) -> u32 {
    value
        .unwrap_or(25)
        .clamp(1, 200)
}

pub(super) fn source_read_end_line(value: Option<u32>) -> u32 {
    value
        .unwrap_or(200)
        .clamp(1, 2_000)
}

pub(super) fn path_glob_to_like(glob: &str) -> String {
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

pub(super) fn version_limit(value: Option<u32>) -> u32 {
    value
        .unwrap_or(100)
        .clamp(1, 500)
}

pub(super) fn dependents_limit(value: Option<u32>) -> u32 {
    value
        .unwrap_or(25)
        .clamp(1, 200)
}

pub(super) fn sync_page(value: Option<u32>) -> u32 {
    value.unwrap_or(1).max(1)
}

pub(super) fn sync_per_page(value: Option<u32>) -> u32 {
    value
        .unwrap_or(25)
        .clamp(1, 100)
}

pub(super) fn graph_depth(value: Option<u32>) -> u32 {
    value.unwrap_or(1).clamp(1, 4)
}

pub(super) fn readme_limit(value: Option<u32>) -> usize {
    value
        .unwrap_or(25_000)
        .clamp(500, 200_000) as usize
}

pub(super) fn dedupe_strings(mut values: Vec<String>) -> Vec<String> {
    values = values
        .into_iter()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .collect::<Vec<_>>();
    values.sort();
    values.dedup();
    values
}

pub(super) fn value_to_string_vec(value: &Value) -> Vec<String> {
    match value {
        Value::Array(items) => items
            .iter()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

pub(super) fn truncate_optional_text(
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

pub(super) fn match_reasons(
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
