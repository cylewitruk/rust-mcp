use rmcp::Json;
pub use rust_mcp_types::types::krate::{
    CrateDeprecatedRequest, CrateDeprecatedResponse, DeprecatedItem,
};
use tracing::warn;

use crate::db::tools;
use crate::mcp::models::{ConfidenceAssessment, ConfidenceLevel};
use crate::mcp::server::McpServer;
use crate::mcp::utils::{
    build_crate_freshness_sources, normalize_optional, normalize_required, sync_page,
};

impl McpServer {
    /// Handles the `crate.deprecated` tool call.
    pub async fn handle_crate_deprecated(
        &self,
        request: CrateDeprecatedRequest,
    ) -> Result<Json<CrateDeprecatedResponse>, String> {
        let crate_name = normalize_required(request.crate_name, "crate_name")?;
        let requested_version = normalize_optional(request.version);
        let page = sync_page(request.page);
        let requested_limit = request
            .limit
            .unwrap_or(50)
            .clamp(1, 200);

        let ctx = self
            .fetch_crate_context(&crate_name)
            .await?;
        let resolution = self
            .resolve_version_or_latest(&ctx, requested_version.as_deref())
            .await?;

        // Deprecation data is richer when rustdoc JSON is available; trigger
        // on-demand enrichment but do not fail if it is unavailable.
        if let Err(error) = self
            .ensure_rustdoc_indexed(&crate_name, resolution.selected_version.id)
            .await
        {
            warn!(
                %crate_name,
                %error,
                "best-effort rustdoc enrichment failed for crate.deprecated"
            );
        }

        let limit = requested_limit;
        let offset = page.saturating_sub(1) * limit;

        let rows = tools::list_deprecated_items(
            &self.state.db,
            resolution.selected_version.id,
            i64::from(limit.saturating_add(1)),
            i64::from(offset),
        )
        .await
        .map_err(|e| format!("crate_deprecated query failed for {}: {e}", ctx.crate_row.name))?;

        let has_more = rows.len() > limit as usize;
        let mut items = rows
            .into_iter()
            .map(|row| DeprecatedItem {
                name: row.name,
                kind: row.kind,
                deprecated_since: row.deprecated_since,
                deprecated_note: row.deprecated_note,
                canonical_path: row.canonical_path,
                index_source: row.index_source,
            })
            .collect::<Vec<_>>();
        if has_more {
            items.truncate(limit as usize);
        }

        let confidence_assessment = if items.is_empty() {
            ConfidenceAssessment {
                level: ConfidenceLevel::Low,
                reason: "no deprecated items were found for the selected version; rustdoc JSON \
                         enrichment may not yet be available"
                    .to_string(),
            }
        } else {
            ConfidenceAssessment {
                level: ConfidenceLevel::High,
                reason: "deprecated items resolved from rustdoc JSON and/or source index"
                    .to_string(),
            }
        };

        let freshness_check_result = ctx
            .freshness_outcome
            .freshness_check_result
            .clone();

        let next_best_calls = if items.is_empty() {
            vec!["crate_api".to_string(), "crate_intel".to_string()]
        } else {
            vec![
                "crate_api".to_string(),
                "crate_type_info".to_string(),
                "crate_migration_path".to_string(),
            ]
        };

        Ok(Json(CrateDeprecatedResponse {
            crate_name: ctx.crate_row.name,
            selected_version: resolution
                .selected_version
                .version,
            latest_version: ctx.latest_version.version,
            count: items.len(),
            has_more,
            truncated: has_more,
            deprecated_items: items,
            freshness_check_performed: ctx
                .freshness_outcome
                .freshness_check_performed,
            freshness_check_result: freshness_check_result.clone(),
            refresh_enqueued: resolution.refresh_enqueued,
            refresh_job_id: resolution.refresh_job_id,
            freshness: build_crate_freshness_sources(
                ctx.crate_row.updated_at,
                &freshness_check_result,
            ),
            confidence: confidence_assessment
                .level
                .as_str()
                .to_string(),
            confidence_assessment,
            next_best_calls,
            provenance: "local_postgres_index(symbols, crate_types)".to_string(),
        }))
    }
}
