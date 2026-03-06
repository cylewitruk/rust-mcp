use serde::Deserialize;
use tracing::debug;

use crate::state::{AppState, OutboundSource};

/// GitHub REST API client for repo metadata and GHSA advisory lookups.
pub struct GitHubClient<'a> {
    state: &'a AppState,
}

impl<'a> GitHubClient<'a> {
    pub fn new(state: &'a AppState) -> Self {
        Self { state }
    }

    fn base_url(&self) -> &str {
        self.state
            .config
            .github_base_url
            .trim_end_matches('/')
    }

    /// Fetches repository metadata for a given `owner/repo`.
    pub async fn fetch_repo_metadata(
        &self,
        owner: &str,
        repo: &str,
    ) -> Result<GitHubRepoMetadata, String> {
        self.state
            .acquire_outbound_slot(OutboundSource::GitHub)
            .await;
        let url = format!("{}/repos/{}/{}", self.base_url(), owner, repo);
        debug!(%owner, %repo, %url, "GitHub repo metadata request");

        let response = self
            .state
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("GitHub repo fetch failed for {owner}/{repo}: {e}"))?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<body unavailable>".to_string());
            return Err(format!(
                "GitHub repo fetch failed for {owner}/{repo} with status {status}: {body}"
            ));
        }

        response
            .json::<GitHubRepoMetadata>()
            .await
            .map_err(|e| format!("failed to decode GitHub repo response for {owner}/{repo}: {e}"))
    }

    /// Queries GitHub Security Advisories for a Rust crate.
    pub async fn fetch_advisories(&self, crate_name: &str) -> Result<Vec<GitHubAdvisory>, String> {
        self.state
            .acquire_outbound_slot(OutboundSource::GitHub)
            .await;
        let url = format!("{}/advisories?ecosystem=rust&package={}", self.base_url(), crate_name);
        debug!(%crate_name, %url, "GitHub GHSA advisory request");

        let response = self
            .state
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("GHSA fetch failed for {crate_name}: {e}"))?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<body unavailable>".to_string());
            return Err(format!("GHSA fetch failed for {crate_name} with status {status}: {body}"));
        }

        response
            .json::<Vec<GitHubAdvisory>>()
            .await
            .map_err(|e| format!("failed to decode GHSA response for {crate_name}: {e}"))
    }

    /// Fetches the total contributor count for a repository using a single
    /// `per_page=1` request and parsing the `Link` header for the last page.
    pub async fn fetch_contributor_count(
        &self,
        owner: &str,
        repo: &str,
    ) -> Result<Option<u64>, String> {
        self.state
            .acquire_outbound_slot(OutboundSource::GitHub)
            .await;
        let url = format!(
            "{}/repos/{}/{}/contributors?per_page=1&anon=true",
            self.base_url(),
            owner,
            repo
        );
        debug!(%owner, %repo, %url, "GitHub contributor count request");

        let response = self
            .state
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("contributor count fetch failed for {owner}/{repo}: {e}"))?;

        let status = response.status();
        if status == reqwest::StatusCode::NO_CONTENT {
            return Ok(Some(0));
        }
        if !status.is_success() {
            return Ok(None);
        }

        // Parse the Link header for the last page number.
        // Format: <url?page=42>; rel="last"
        if let Some(link) = response
            .headers()
            .get("link")
            .and_then(|v| v.to_str().ok())
            && let Some(count) = parse_last_page_from_link_header(link)
        {
            return Ok(Some(count));
        }

        // If no Link header, the response body itself is the full list.
        let contributors = response
            .json::<Vec<serde_json::Value>>()
            .await
            .unwrap_or_default();
        Ok(Some(contributors.len() as u64))
    }

    /// Fetches the most recent releases for a repository.
    pub async fn fetch_releases(
        &self,
        owner: &str,
        repo: &str,
        per_page: u8,
    ) -> Result<Vec<GitHubRelease>, String> {
        self.state
            .acquire_outbound_slot(OutboundSource::GitHub)
            .await;
        let url =
            format!("{}/repos/{}/{}/releases?per_page={}", self.base_url(), owner, repo, per_page,);
        debug!(%owner, %repo, %url, "GitHub releases request");

        let response = self
            .state
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("releases fetch failed for {owner}/{repo}: {e}"))?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<body unavailable>".to_string());
            return Err(format!(
                "releases fetch failed for {owner}/{repo} with status {status}: {body}"
            ));
        }

        response
            .json::<Vec<GitHubRelease>>()
            .await
            .map_err(|e| format!("failed to decode releases response for {owner}/{repo}: {e}"))
    }
}

/// Extracts the last page number from a GitHub `Link` header.
///
/// The header looks like:
/// `<https://api.github.com/repos/foo/bar/contributors?per_page=1&anon=true&page=42>; rel="last"`
fn parse_last_page_from_link_header(header: &str) -> Option<u64> {
    for part in header.split(',') {
        if part.contains("rel=\"last\"") {
            // Extract URL between < and >
            let url = part
                .trim()
                .strip_prefix('<')?
                .split('>')
                .next()?;
            // Find page= parameter
            let query = url.split('?').nth(1)?;
            for param in query.split('&') {
                if let Some(value) = param.strip_prefix("page=") {
                    return value.parse::<u64>().ok();
                }
            }
        }
    }
    None
}

/// Parses a GitHub `owner/repo` pair from a repository URL.
///
/// Handles common formats:
/// - `https://github.com/owner/repo`
/// - `https://github.com/owner/repo.git`
/// - `https://github.com/owner/repo/tree/main/subcrate`
///
/// Returns `None` for non-GitHub URLs or unparseable paths.
pub fn parse_github_owner_repo(url: &str) -> Option<(String, String)> {
    let url = url
        .trim()
        .trim_end_matches('/');

    // Must contain github.com
    let path = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("http://github.com/"))?;

    let mut segments = path.splitn(3, '/');
    let owner = segments.next()?.to_string();
    let repo = segments
        .next()?
        .trim_end_matches(".git")
        .to_string();

    if owner.is_empty() || repo.is_empty() {
        return None;
    }

    Some((owner, repo))
}

// ── Response models ─────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct GitHubRepoMetadata {
    pub stargazers_count: u64,
    pub forks_count: u64,
    pub open_issues_count: u64,
    pub archived: bool,
    pub pushed_at: Option<String>,
    pub license: Option<GitHubLicense>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct GitHubLicense {
    pub spdx_id: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct GitHubAdvisory {
    pub ghsa_id: String,
    pub cve_id: Option<String>,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub severity: Option<String>,
    pub html_url: Option<String>,
    #[serde(default)]
    pub identifiers: Vec<GitHubAdvisoryIdentifier>,
    #[serde(default)]
    pub vulnerabilities: Vec<GitHubAdvisoryVulnerability>,
    #[serde(default)]
    pub cwes: Vec<GitHubAdvisoryCwe>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct GitHubAdvisoryIdentifier {
    #[serde(rename = "type")]
    pub id_type: String,
    pub value: String,
}

#[derive(Debug, Deserialize)]
pub struct GitHubAdvisoryVulnerability {
    pub package: Option<GitHubAdvisoryPackage>,
    pub vulnerable_version_range: Option<String>,
    pub first_patched_version: Option<GitHubAdvisoryVersion>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct GitHubAdvisoryPackage {
    pub ecosystem: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct GitHubAdvisoryVersion {
    pub identifier: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct GitHubAdvisoryCwe {
    pub cwe_id: Option<String>,
}

/// A GitHub release entry.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct GitHubRelease {
    pub tag_name: String,
    pub name: Option<String>,
    pub body: Option<String>,
    pub published_at: Option<String>,
    pub prerelease: bool,
    pub draft: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_standard_github_url() {
        let (owner, repo) = parse_github_owner_repo("https://github.com/serde-rs/serde").unwrap();
        assert_eq!(owner, "serde-rs");
        assert_eq!(repo, "serde");
    }

    #[test]
    fn parse_github_url_with_git_suffix() {
        let (owner, repo) =
            parse_github_owner_repo("https://github.com/tokio-rs/tokio.git").unwrap();
        assert_eq!(owner, "tokio-rs");
        assert_eq!(repo, "tokio");
    }

    #[test]
    fn parse_github_url_with_subpath() {
        let (owner, repo) =
            parse_github_owner_repo("https://github.com/dtolnay/syn/tree/master/derive").unwrap();
        assert_eq!(owner, "dtolnay");
        assert_eq!(repo, "syn");
    }

    #[test]
    fn parse_github_url_with_trailing_slash() {
        let (owner, repo) = parse_github_owner_repo("https://github.com/rust-lang/rust/").unwrap();
        assert_eq!(owner, "rust-lang");
        assert_eq!(repo, "rust");
    }

    #[test]
    fn parse_non_github_url_returns_none() {
        assert!(parse_github_owner_repo("https://gitlab.com/foo/bar").is_none());
    }

    #[test]
    fn parse_incomplete_github_url_returns_none() {
        assert!(parse_github_owner_repo("https://github.com/owner").is_none());
    }

    #[test]
    fn parse_link_header_extracts_last_page() {
        let header = r#"<https://api.github.com/repos/foo/bar/contributors?per_page=1&anon=true&page=2>; rel="next", <https://api.github.com/repos/foo/bar/contributors?per_page=1&anon=true&page=142>; rel="last""#;
        assert_eq!(parse_last_page_from_link_header(header), Some(142));
    }

    #[test]
    fn parse_link_header_returns_none_without_last() {
        let header = r#"<https://api.github.com/repos/foo/bar/contributors?page=2>; rel="next""#;
        assert_eq!(parse_last_page_from_link_header(header), None);
    }

    #[test]
    fn parse_link_header_returns_none_for_empty() {
        assert_eq!(parse_last_page_from_link_header(""), None);
    }
}
