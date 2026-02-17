use serde::Deserialize;
use serde_json::json;

use crate::state::{AppState, OutboundSource};

const OSV_QUERY_URL: &str = "https://api.osv.dev/v1/query";

#[derive(Debug, Deserialize)]
pub struct OsvQueryResponse {
    #[serde(default)]
    pub vulns: Vec<OsvVulnerability>,
}

#[derive(Debug, Deserialize)]
pub struct OsvVulnerability {
    pub id: String,
    pub summary: Option<String>,
    pub details: Option<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub affected: Vec<OsvAffected>,
    #[serde(default)]
    pub references: Vec<OsvReference>,
    #[serde(default)]
    pub severity: Vec<OsvSeverity>,
}

#[derive(Debug, Deserialize)]
pub struct OsvAffected {
    #[serde(default)]
    pub versions: Vec<String>,
    #[serde(default)]
    pub ranges: Vec<OsvRange>,
}

#[derive(Debug, Deserialize)]
pub struct OsvRange {
    #[serde(rename = "type")]
    pub range_type: String,
    #[serde(default)]
    pub events: Vec<OsvRangeEvent>,
}

#[derive(Debug, Deserialize)]
pub struct OsvRangeEvent {
    pub introduced: Option<String>,
    pub fixed: Option<String>,
    pub last_affected: Option<String>,
    pub limit: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct OsvReference {
    pub url: String,
}

#[derive(Debug, Deserialize)]
pub struct OsvSeverity {
    #[serde(rename = "type")]
    pub severity_type: Option<String>,
    pub score: Option<String>,
}

pub struct OsvDevClient<'a> {
    state: &'a AppState,
}

impl<'a> OsvDevClient<'a> {
    pub fn new(state: &'a AppState) -> Self {
        Self { state }
    }

    pub async fn query_crate(&self, crate_name: &str) -> Result<OsvQueryResponse, String> {
        self.state
            .acquire_outbound_slot(OutboundSource::Osv)
            .await;
        let response = self
            .state
            .http
            .post(OSV_QUERY_URL)
            .json(&json!({
                "package": {
                    "name": crate_name,
                    "ecosystem": "crates.io"
                }
            }))
            .send()
            .await
            .map_err(|e| format!("OSV query failed for {crate_name}: {e}"))?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<body unavailable>".to_string());
            return Err(format!(
                "OSV query failed for {crate_name} with status {}: {}",
                status, body
            ));
        }

        response
            .json::<OsvQueryResponse>()
            .await
            .map_err(|e| format!("failed to decode OSV response for {crate_name}: {e}"))
    }
}
