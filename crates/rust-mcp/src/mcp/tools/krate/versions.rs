use rmcp::Json;
pub use rust_mcp_types::types::krate::{
    CrateVersionTimelineItem, CrateVersionsRequest, CrateVersionsResponse,
};
use serde::{Deserialize, Serialize};

use crate::db::tools;
use crate::mcp::models::{ConfidenceAssessment, ConfidenceLevel};
use crate::mcp::progress::ToolCallContext;
use crate::mcp::server::McpServer;
use crate::mcp::utils::{
    CursorToken, apply_pagination_limit, build_crate_freshness_sources, decode_cursor,
    normalize_optional, normalize_required, resolve_pagination, sync_page, version_limit,
};

#[derive(Debug, Serialize, Deserialize)]
struct CrateVersionsCursorToken {
    v: u8,
    offset: u32,
    limit: u32,
    crate_name: String,
}

impl CursorToken for CrateVersionsCursorToken {
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

fn adoption_signal(downloads: i64, yanked: bool) -> String {
    if yanked {
        return "deprecated".to_string();
    }
    if downloads >= 1_000_000 {
        return "high".to_string();
    }
    if downloads >= 100_000 {
        return "medium".to_string();
    }
    "low".to_string()
}

impl McpServer {
    /// Handles the `crate.versions` tool call.
    pub async fn handle_crate_versions(
        &self,
        request: CrateVersionsRequest,
        tcx: ToolCallContext,
    ) -> Result<Json<CrateVersionsResponse>, String> {
        let crate_name = normalize_required(request.crate_name, "crate_name")?;
        let cursor = normalize_optional(request.cursor);
        let page = sync_page(request.page);
        let requested_limit = version_limit(request.limit);

        let decoded = cursor
            .as_deref()
            .map(decode_cursor::<CrateVersionsCursorToken>)
            .transpose()?;

        if let Some(ref token) = decoded
            && token.crate_name != crate_name
        {
            return Err("cursor does not match current crate.versions filters".to_string());
        }

        let pag =
            resolve_pagination(decoded.as_ref(), request.limit.is_some(), requested_limit, page)?;

        let ctx = self
            .fetch_crate_context(&crate_name, &tcx)
            .await?;

        let rows = tools::list_crate_version_timeline(
            &self.state.db,
            ctx.crate_row.id,
            i64::from(pag.limit.saturating_add(1)),
            i64::from(pag.offset),
        )
        .await
        .map_err(|e| format!("version timeline query failed for {}: {e}", ctx.crate_row.name))?;

        let versions = rows
            .into_iter()
            .map(|row| {
                let mut markers = Vec::new();
                let is_latest = row.version == ctx.latest_version.version;

                if is_latest {
                    markers.push("latest".to_string());
                }
                if row.yanked {
                    markers.push("yanked".to_string());
                }
                if row.advisory_count > 0 {
                    markers.push("security_advisory".to_string());
                }
                if let Some(days) = row.release_age_days
                    && days >= 365
                {
                    markers.push("legacy".to_string());
                }

                CrateVersionTimelineItem {
                    version: row.version,
                    rust_version: row.rust_version,
                    published_at: row.published_at,
                    yanked: row.yanked,
                    downloads: row.total_downloads,
                    advisory_count: row.advisory_count,
                    release_age_days: row.release_age_days,
                    is_latest,
                    adoption_signal: adoption_signal(row.total_downloads, row.yanked),
                    markers,
                }
            })
            .collect::<Vec<_>>();

        let crate_name_clone = crate_name.clone();
        let paginated = apply_pagination_limit(versions, pag.limit, pag.offset, |next_offset| {
            CrateVersionsCursorToken {
                v: 1,
                offset: next_offset,
                limit: pag.limit,
                crate_name: crate_name_clone,
            }
        })?;

        let freshness_check_result = ctx
            .freshness_outcome
            .freshness_check_result
            .clone();

        let confidence_assessment = if paginated.items.is_empty() {
            ConfidenceAssessment {
                level: ConfidenceLevel::Low,
                reason: "no versions were returned from local index for selected crate".to_string(),
            }
        } else {
            ConfidenceAssessment {
                level: ConfidenceLevel::High,
                reason: "version timeline resolved from indexed crate versions and advisories"
                    .to_string(),
            }
        };

        Ok(Json(CrateVersionsResponse {
            crate_name: ctx.crate_row.name,
            cursor,
            next_cursor: paginated.next_cursor,
            page: pag.effective_page,
            limit: pag.limit,
            has_more: paginated.has_more,
            truncated: paginated.has_more,
            latest_rust_version: ctx
                .latest_version
                .rust_version,
            count: paginated.items.len(),
            versions: paginated.items,
            freshness_check_performed: ctx
                .freshness_outcome
                .freshness_check_performed,
            freshness_check_result: freshness_check_result.clone(),
            refresh_enqueued: ctx
                .freshness_outcome
                .refresh_enqueued,
            refresh_job_id: ctx
                .freshness_outcome
                .refresh_job_id,
            freshness: build_crate_freshness_sources(
                ctx.crate_row.updated_at,
                &freshness_check_result,
            ),
            confidence: confidence_assessment
                .level
                .as_str()
                .to_string(),
            confidence_assessment,
            suggested_next_tools: vec![
                "crate_intel".to_string(),
                "crate_graph".to_string(),
                "index_refresh".to_string(),
            ],
            provenance: "local_postgres_index".to_string(),
        }))
    }
}
