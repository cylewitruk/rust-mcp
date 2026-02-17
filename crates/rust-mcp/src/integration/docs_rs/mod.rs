use crate::state::{AppState, OutboundSource};

pub struct DocsRsClient<'a> {
    state: &'a AppState,
}

impl<'a> DocsRsClient<'a> {
    pub fn new(state: &'a AppState) -> Self {
        Self { state }
    }

    pub fn base_url(&self) -> &str {
        self.state
            .config
            .docs_rs_base_url
            .trim_end_matches('/')
    }

    pub fn url(&self, path: &str) -> String {
        let suffix = path.trim_start_matches('/');
        format!("{}/{}", self.base_url(), suffix)
    }

    pub fn rustdoc_json_url(&self, crate_name: &str, version: &str) -> String {
        format!("{}/crate/{crate_name}/{version}/json.gz", self.base_url())
    }

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
