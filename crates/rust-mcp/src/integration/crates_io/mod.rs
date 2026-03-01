use std::collections::BTreeMap;

use reqwest::StatusCode;
use reqwest::header::CONTENT_TYPE;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use tracing::debug;

use crate::state::{AppState, OutboundSource};

const README_MAX_CHARS: usize = 1_000_000;

#[derive(Debug, Deserialize)]
pub struct CratesIoSearchResponse {
    pub crates: Vec<CratesIoSearchCrate>,
    pub meta: CratesIoSearchMeta,
}

#[derive(Debug, Deserialize)]
pub struct CratesIoSearchMeta {
    pub total: u64,
}

#[derive(Debug, Deserialize)]
pub struct CratesIoSearchCrate {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct CratesIoCrateDetailResponse {
    #[serde(rename = "crate")]
    pub krate: CratesIoCrateRecord,
    pub versions: Vec<CratesIoVersionRecord>,
    #[serde(default)]
    pub keywords: Vec<CratesIoKeyword>,
    #[serde(default)]
    pub categories: Vec<CratesIoCategory>,
}

#[derive(Debug, Deserialize)]
pub struct CratesIoCrateRecord {
    pub name: String,
    pub description: Option<String>,
    pub repository: Option<String>,
    pub documentation: Option<String>,
    pub homepage: Option<String>,
    pub max_version: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CratesIoVersionRecord {
    pub num: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    #[serde(default)]
    pub yanked: bool,
    pub downloads: Option<i64>,
    pub checksum: Option<String>,
    pub rust_version: Option<String>,
    pub license: Option<String>,
    #[serde(default)]
    pub features: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct CratesIoKeyword {
    pub id: String,
    pub keyword: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CratesIoCategory {
    pub id: String,
    pub slug: Option<String>,
    pub category: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CratesIoDependenciesResponse {
    #[serde(default)]
    dependencies: Vec<CratesIoDependency>,
}

#[derive(Debug, Deserialize)]
pub struct CratesIoDependency {
    pub crate_id: String,
    pub req: String,
    pub kind: Option<String>,
    #[serde(default)]
    pub optional: bool,
    #[serde(default)]
    pub features: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct CratesIoReadmeBody {
    content: Option<String>,
    readme: Option<String>,
    message: Option<String>,
}

pub struct CratesIoClient<'a> {
    state: &'a AppState,
}

impl<'a> CratesIoClient<'a> {
    pub fn new(state: &'a AppState) -> Self {
        Self { state }
    }

    pub fn url(&self, path: &str) -> String {
        let base = self
            .state
            .config
            .crates_io_base_url
            .trim_end_matches('/');
        let suffix = path.trim_start_matches('/');
        format!("{base}/{suffix}")
    }

    pub async fn get_json<T: DeserializeOwned>(
        &self,
        path: &str,
        params: &[(&str, String)],
    ) -> Result<T, String> {
        self.state
            .acquire_outbound_slot(OutboundSource::CratesIo)
            .await;
        let url = self.url(path);
        debug!(url = %url, "crates.io GET JSON request");
        let response = self
            .state
            .http
            .get(&url)
            .query(params)
            .send()
            .await
            .map_err(|e| format!("request to {url} failed: {e}"))?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<body unavailable>".to_string());
            return Err(format!("request to {url} failed with status {}: {}", status, body));
        }

        response
            .json::<T>()
            .await
            .map_err(|e| format!("failed to decode JSON response from {url}: {e}"))
    }

    pub async fn search_crates(
        &self,
        params: &[(&str, String)],
    ) -> Result<CratesIoSearchResponse, String> {
        self.get_json("api/v1/crates", params)
            .await
    }

    pub async fn fetch_crate_detail(
        &self,
        crate_name: &str,
    ) -> Result<CratesIoCrateDetailResponse, String> {
        self.get_json(&format!("api/v1/crates/{crate_name}"), &[])
            .await
    }

    pub async fn fetch_readme(
        &self,
        crate_name: &str,
        version: &str,
    ) -> Result<Option<String>, String> {
        self.state
            .acquire_outbound_slot(OutboundSource::CratesIo)
            .await;
        let url = self.url(&format!("api/v1/crates/{crate_name}/{version}/readme"));
        debug!(%crate_name, %version, url = %url, "crates.io fetch readme request");
        let response = self
            .state
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("readme request failed for {crate_name}@{version}: {e}"))?;

        // 404 = no readme uploaded; 403 = S3 returns AccessDenied for
        // missing objects behind the crates.io redirect.  Both mean "no readme".
        if response.status() == StatusCode::NOT_FOUND || response.status() == StatusCode::FORBIDDEN
        {
            return Ok(None);
        }

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<body unavailable>".to_string());
            return Err(format!(
                "readme request failed for {crate_name}@{version} with status {}: {}",
                status, body
            ));
        }

        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let body = response
            .text()
            .await
            .map_err(|e| format!("failed to read readme body for {crate_name}@{version}: {e}"))?;

        if body.trim().is_empty() {
            return Ok(None);
        }

        let readme = if content_type.contains("application/json") {
            parse_json_readme_body(&body)
        } else {
            Some(body)
        };

        Ok(normalize_readme(readme))
    }

    pub async fn fetch_crate_dependencies(
        &self,
        crate_name: &str,
        version: &str,
    ) -> Result<Vec<CratesIoDependency>, String> {
        debug!(%crate_name, %version, "crates.io fetch dependencies request");
        let path = format!("api/v1/crates/{crate_name}/{version}/dependencies");
        let response: CratesIoDependenciesResponse = self
            .get_json(&path, &[])
            .await?;
        Ok(response.dependencies)
    }
}

fn parse_json_readme_body(body: &str) -> Option<String> {
    let parsed = serde_json::from_str::<CratesIoReadmeBody>(body).ok()?;
    parsed
        .content
        .or(parsed.readme)
        .or(parsed.message)
}

fn normalize_readme(readme: Option<String>) -> Option<String> {
    let trimmed = readme?.trim().to_string();
    if trimmed.is_empty() {
        None
    } else if trimmed.len() > README_MAX_CHARS {
        Some(
            trimmed
                .chars()
                .take(README_MAX_CHARS)
                .collect(),
        )
    } else {
        Some(trimmed)
    }
}
