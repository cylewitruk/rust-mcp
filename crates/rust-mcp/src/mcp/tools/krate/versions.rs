use rmcp::Json;
pub use rust_mcp_types::types::krate::{
    CrateVersionTimelineItem, CrateVersionsRequest, CrateVersionsResponse,
};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::mcp::models::{ConfidenceAssessment, ConfidenceLevel};
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

#[derive(Debug, FromRow)]
pub(crate) struct CrateVersionTimelineRow {
    pub(crate) version: String,
    pub(crate) rust_version: Option<String>,
    pub(crate) published_at: Option<String>,
    pub(crate) yanked: bool,
    pub(crate) total_downloads: i64,
    pub(crate) advisory_count: i64,
    pub(crate) release_age_days: Option<i64>,
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
    pub(crate) async fn handle_crate_versions(
        &self,
        request: CrateVersionsRequest,
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
            .fetch_crate_context(&crate_name)
            .await?;

        let rows = sqlx::query_as::<_, CrateVersionTimelineRow>(
            "SELECT
                cv.version,
                cv.rust_version,
                cv.published_at::TEXT AS published_at,
                cv.yanked,
                COALESCE(cv.total_downloads, 0)::BIGINT AS total_downloads,
                COUNT(am.id)::BIGINT AS advisory_count,
                CASE
                    WHEN cv.published_at IS NULL THEN NULL
                    ELSE EXTRACT(EPOCH FROM (NOW() - cv.published_at))::BIGINT / 86400
                END AS release_age_days
             FROM crate_versions cv
             LEFT JOIN advisory_matches am ON am.version_id = cv.id
             WHERE cv.crate_id = $1
             GROUP BY cv.id, cv.version, cv.published_at, cv.yanked, cv.total_downloads
             ORDER BY cv.published_at DESC NULLS LAST, cv.id DESC
               LIMIT $2
               OFFSET $3",
        )
        .bind(ctx.crate_row.id)
        .bind(i64::from(pag.limit.saturating_add(1)))
        .bind(i64::from(pag.offset))
        .fetch_all(&self.state.db)
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
            next_best_calls: vec![
                "crate.intel".to_string(),
                "crate.graph".to_string(),
                "index.refresh".to_string(),
            ],
            provenance: "local_postgres_index".to_string(),
        }))
    }
}
