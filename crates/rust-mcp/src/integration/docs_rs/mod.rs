use std::collections::HashSet;
use std::io::Read as _;

use flate2::read::GzDecoder;

use crate::state::{AppState, OutboundSource};

const MAX_EXTRACTED_DOCS_CONTENT_CHARS: usize = 1_000_000;
const MAX_TITLE_CHARS: usize = 300;

/// Shared docs.rs HTTP client and URL helpers.
pub struct DocsRsClient<'a> {
    state: &'a AppState,
}

impl<'a> DocsRsClient<'a> {
    /// Creates a docs.rs client bound to shared application state.
    pub fn new(state: &'a AppState) -> Self {
        Self { state }
    }

    /// Returns the normalized docs.rs base URL without a trailing slash.
    pub fn base_url(&self) -> &str {
        self.state
            .config
            .docs_rs_base_url
            .trim_end_matches('/')
    }

    /// Builds an absolute docs.rs URL from a relative path.
    pub fn url(&self, path: &str) -> String {
        let suffix = path.trim_start_matches('/');
        format!("{}/{}", self.base_url(), suffix)
    }

    /// Builds the docs.rs rustdoc JSON endpoint URL for a crate version.
    pub fn rustdoc_json_url(&self, crate_name: &str, version: &str) -> String {
        format!("{}/crate/{crate_name}/{version}/json.gz", self.base_url())
    }

    /// Fetches a docs.rs HTML page body by relative docs path.
    pub async fn fetch_page_html(&self, path: &str) -> Result<String, String> {
        self.state
            .acquire_outbound_slot(OutboundSource::DocsRs)
            .await;
        let url = self.url(path);
        let response = self
            .state
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("docs fetch failed {url}: {e}"))?;

        let status = response.status();
        if !status.is_success() {
            return Err(format!("docs fetch failed {url}: status {status}"));
        }

        response
            .text()
            .await
            .map_err(|e| format!("docs body read failed {url}: {e}"))
    }

    /// Fetches the docs.rs rustdoc JSON payload bytes for a crate version.
    pub async fn fetch_rustdoc_json(
        &self,
        crate_name: &str,
        version: &str,
    ) -> Result<Vec<u8>, String> {
        self.state
            .acquire_outbound_slot(OutboundSource::DocsRs)
            .await;
        let url = self.rustdoc_json_url(crate_name, version);
        let response = self
            .state
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| {
                format!("docs.rs rustdoc JSON request failed for {crate_name}@{version}: {e}")
            })?;

        let status = response.status();
        if !status.is_success() {
            return Err(format!(
                "docs.rs rustdoc JSON request failed for {crate_name}@{version} with status \
                 {status}"
            ));
        }

        response
            .bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| {
                format!("docs.rs rustdoc JSON body read failed for {crate_name}@{version}: {e}")
            })
    }
}

/// Extracts the first `<title>` value from an HTML document.
pub fn extract_title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let start_tag = "<title>";
    let end_tag = "</title>";

    let start = lower.find(start_tag)? + start_tag.len();
    let end = lower[start..].find(end_tag)? + start;
    let title = html[start..end].trim();
    if title.is_empty() {
        None
    } else {
        Some(
            title
                .chars()
                .take(MAX_TITLE_CHARS)
                .collect(),
        )
    }
}

/// Converts HTML into normalized plain text for indexing.
pub fn strip_html(html: &str) -> String {
    let mut output = String::with_capacity(html.len().min(200_000));
    let mut in_tag = false;

    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => output.push(ch),
            _ => {}
        }
    }

    let normalized = output
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    normalized
        .chars()
        .take(MAX_EXTRACTED_DOCS_CONTENT_CHARS)
        .collect()
}

/// Extracts all quoted `href` attribute values from an HTML document.
pub fn extract_href_values(html: &str) -> Vec<String> {
    let lower = html.to_ascii_lowercase();
    let lower_bytes = lower.as_bytes();
    let bytes = html.as_bytes();

    let mut out = Vec::new();
    let mut idx = 0_usize;
    while idx + 6 < lower_bytes.len() {
        let Some(found) = lower[idx..].find("href=") else {
            break;
        };
        idx += found + 5;
        if idx >= bytes.len() {
            break;
        }

        let quote = bytes[idx] as char;
        if quote != '"' && quote != '\'' {
            idx += 1;
            continue;
        }

        idx += 1;
        let start = idx;
        while idx < bytes.len() && (bytes[idx] as char) != quote {
            idx += 1;
        }
        if idx <= bytes.len()
            && let Ok(value) = std::str::from_utf8(&bytes[start..idx])
        {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                out.push(trimmed.to_string());
            }
        }
        idx = idx.saturating_add(1);
    }

    out
}

fn normalize_joined_path(path: &str) -> Option<String> {
    let mut raw = path
        .split('#')
        .next()
        .unwrap_or(path)
        .split('?')
        .next()
        .unwrap_or(path)
        .trim()
        .to_string();

    if raw.is_empty() {
        return None;
    }

    let has_trailing_slash = raw.ends_with('/');
    raw = raw
        .trim_start_matches('/')
        .to_string();

    let mut parts: Vec<&str> = Vec::new();
    for segment in raw.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(segment),
        }
    }

    let mut normalized = parts.join("/");
    if normalized.is_empty() {
        return None;
    }

    if has_trailing_slash {
        normalized.push('/');
    }
    Some(normalized)
}

/// Resolves a docs.rs `href` into a normalized in-crate docs path.
pub fn resolve_docs_href(
    docs_base_url: &str,
    current_path: &str,
    href: &str,
    crate_prefix: &str,
) -> Option<String> {
    let raw = href.trim();
    if raw.is_empty()
        || raw.starts_with('#')
        || raw.starts_with("javascript:")
        || raw.starts_with("mailto:")
        || raw.starts_with("tel:")
        || raw.starts_with("//")
    {
        return None;
    }

    let candidate = if raw.starts_with("http://") || raw.starts_with("https://") {
        let base = docs_base_url.trim_end_matches('/');
        let suffix = raw.strip_prefix(base)?;
        suffix
            .trim_start_matches('/')
            .to_string()
    } else if raw.starts_with('/') {
        raw.trim_start_matches('/')
            .to_string()
    } else {
        let base_dir = if current_path.ends_with('/') {
            current_path.to_string()
        } else {
            current_path
                .rsplit_once('/')
                .map(|(dir, _)| format!("{dir}/"))
                .unwrap_or_default()
        };
        format!("{base_dir}{raw}")
    };

    let normalized = normalize_joined_path(&candidate)?;
    if !normalized.starts_with(crate_prefix) {
        return None;
    }
    if !(normalized.ends_with('/') || normalized.ends_with(".html")) {
        return None;
    }
    Some(normalized)
}

/// Discovers unique in-crate docs paths from a docs.rs HTML page.
pub fn discover_docs_paths(
    docs_base_url: &str,
    current_path: &str,
    html: &str,
    crate_prefix: &str,
) -> Vec<String> {
    let mut unique = HashSet::new();
    for href in extract_href_values(html) {
        if let Some(path) = resolve_docs_href(docs_base_url, current_path, &href, crate_prefix) {
            unique.insert(path);
        }
    }

    let mut out = unique
        .into_iter()
        .collect::<Vec<_>>();
    out.sort();
    out
}

/// Returns the synthetic source-files path used for docs.rs rustdoc payloads.
pub fn docs_rs_rustdoc_synthetic_path(crate_name: &str, version: &str) -> String {
    format!("rustdoc-json/docs.rs/{crate_name}-{version}.json")
}

/// Decodes a docs.rs rustdoc payload that may be plain UTF-8 or gzip.
pub fn decode_docs_rs_rustdoc_payload(payload_bytes: Vec<u8>) -> Result<String, String> {
    if let Ok(content) = String::from_utf8(payload_bytes.clone()) {
        return Ok(content);
    }

    let mut decoder = GzDecoder::new(payload_bytes.as_slice());
    let mut decoded_bytes = Vec::new();
    if decoder
        .read_to_end(&mut decoded_bytes)
        .is_ok()
    {
        return String::from_utf8(decoded_bytes)
            .map_err(|e| format!("decoded docs.rs rustdoc JSON payload was invalid UTF-8: {e}"));
    }

    Err("docs.rs rustdoc JSON payload was not valid UTF-8 or gzip".to_string())
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use flate2::Compression;
    use flate2::write::GzEncoder;

    use super::*;

    #[test]
    fn resolve_docs_href_keeps_in_crate_prefix() {
        let base = "https://docs.rs";
        let prefix = "serde/1.0.0/";

        let relative = resolve_docs_href(
            base,
            "serde/1.0.0/serde/index.html",
            "../serde/trait.Serializer.html",
            prefix,
        );
        assert_eq!(relative.as_deref(), Some("serde/1.0.0/serde/trait.Serializer.html"));

        let external = resolve_docs_href(
            base,
            "serde/1.0.0/serde/index.html",
            "https://example.com/not-docs",
            prefix,
        );
        assert!(external.is_none());

        let outside_prefix = resolve_docs_href(
            base,
            "serde/1.0.0/serde/index.html",
            "/tokio/1.0.0/tokio/index.html",
            prefix,
        );
        assert!(outside_prefix.is_none());
    }

    #[test]
    fn extract_href_values_reads_single_and_double_quotes() {
        let html = r#"<a href="a/b.html">A</a><a href='c/d/'>B</a>"#;
        let values = extract_href_values(html);
        assert_eq!(values, vec!["a/b.html".to_string(), "c/d/".to_string()]);
    }

    #[test]
    fn decode_docs_rs_payload_accepts_plain_utf8() {
        let expected = "{\"hello\":\"world\"}".to_string();
        let decoded =
            decode_docs_rs_rustdoc_payload(expected.clone().into_bytes()).expect("decode failed");
        assert_eq!(decoded, expected);
    }

    #[test]
    fn decode_docs_rs_payload_accepts_gzip_utf8() {
        let expected = "{\"hello\":\"world\"}".to_string();
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder
            .write_all(expected.as_bytes())
            .expect("failed to write gzip payload");
        let payload = encoder
            .finish()
            .expect("failed to finish gzip payload");

        let decoded = decode_docs_rs_rustdoc_payload(payload).expect("decode failed");
        assert_eq!(decoded, expected);
    }

    #[test]
    fn decode_docs_rs_payload_rejects_invalid_bytes() {
        let error =
            decode_docs_rs_rustdoc_payload(vec![0, 159, 146, 150]).expect_err("expected error");
        assert!(error.contains("not valid UTF-8 or gzip"));
    }
}
