use std::collections::BTreeSet;

use rmcp::Json;
pub use rust_mcp_types::types::krate::{CrateApiRequest, CrateApiResponse, CrateApiSymbol};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::mcp::models::{ConfidenceAssessment, ConfidenceLevel};
use crate::mcp::server::McpServer;
use crate::mcp::utils::{
    CursorToken, build_crate_freshness_sources, crate_api_limit, decode_cursor, encode_cursor,
    normalize_optional, normalize_required, path_glob_to_like, resolve_pagination, sync_page,
};

#[derive(Debug, Serialize, Deserialize)]
struct CrateApiCursorToken {
    v: u8,
    offset: u32,
    limit: u32,
    crate_name: String,
    version: Option<String>,
    path_glob: Option<String>,
    kinds: Vec<String>,
}

impl CursorToken for CrateApiCursorToken {
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
pub(crate) struct ApiSurfaceRow {
    pub(crate) name: String,
    pub(crate) kind: String,
    pub(crate) signature: Option<String>,
    pub(crate) visibility: Option<String>,
    pub(crate) source_path: String,
    pub(crate) start_line: i32,
    pub(crate) end_line: i32,
    pub(crate) index_source: String,
}

fn normalize_kind_filters(input: Option<Vec<String>>) -> Vec<String> {
    let mut out = input
        .unwrap_or_default()
        .into_iter()
        .map(|value| {
            value
                .trim()
                .to_ascii_lowercase()
        })
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    out.sort();
    out.dedup();
    out
}

fn allowed_kind_filters(values: Vec<String>) -> Vec<String> {
    let allowed =
        ["function", "struct", "enum", "trait", "type_alias", "const", "static", "module"]
            .into_iter()
            .map(ToString::to_string)
            .collect::<BTreeSet<_>>();

    values
        .into_iter()
        .filter(|value| allowed.contains(value))
        .collect()
}

impl McpServer {
    pub(crate) async fn handle_crate_api(
        &self,
        request: CrateApiRequest,
    ) -> Result<Json<CrateApiResponse>, String> {
        let crate_name = normalize_required(request.crate_name, "crate_name")?;
        let requested_version = normalize_optional(request.version);
        let path_glob = normalize_optional(request.path_glob);
        let kinds = allowed_kind_filters(normalize_kind_filters(request.kinds));
        let cursor = normalize_optional(request.cursor);
        let page = sync_page(request.page);
        let requested_limit = crate_api_limit(request.limit);

        let decoded = cursor
            .as_deref()
            .map(decode_cursor::<CrateApiCursorToken>)
            .transpose()?;

        if let Some(ref token) = decoded
            && (token.crate_name != crate_name
                || token.version != requested_version
                || token.path_glob != path_glob
                || token.kinds != kinds)
        {
            return Err("cursor does not match current crate.api filters".to_string());
        }

        let pag =
            resolve_pagination(decoded.as_ref(), request.limit.is_some(), requested_limit, page)?;

        let ctx = self
            .fetch_crate_context(&crate_name)
            .await?;
        let resolution = self
            .resolve_version_or_latest(&ctx, requested_version.as_deref())
            .await?;

        let has_rustdoc_symbols = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*)::BIGINT
             FROM symbols
             WHERE crate_version_id = $1
               AND visibility = 'public'
               AND index_source = 'rustdoc_json'",
        )
        .bind(resolution.selected_version.id)
        .fetch_one(&self.state.db)
        .await
        .map_err(|e| format!("crate.api rustdoc source check failed: {e}"))?
            > 0;
        let preferred_source = if has_rustdoc_symbols { Some("rustdoc_json") } else { None };

        let mut rows = if let Some(path_filter) = path_glob.as_deref() {
            sqlx::query_as::<_, ApiSurfaceRow>(
                "SELECT
                    s.name,
                    s.kind,
                    s.signature,
                    s.visibility,
                    sf.path AS source_path,
                    s.start_line,
                    s.end_line,
                    s.index_source
                 FROM symbols s
                 JOIN source_files sf ON sf.id = s.source_file_id
                 WHERE s.crate_version_id = $1
                   AND s.visibility = 'public'
                   AND ($2::TEXT[] IS NULL OR s.kind = ANY($2))
                                     AND ($3::TEXT IS NULL OR s.index_source = $3)
                                     AND sf.path ILIKE $4 ESCAPE '\\'
                 ORDER BY
                    s.kind ASC,
                    s.name ASC,
                    sf.path ASC,
                    s.start_line ASC,
                    s.end_line ASC
                 LIMIT $5
                 OFFSET $6",
            )
            .bind(resolution.selected_version.id)
            .bind(if kinds.is_empty() { None } else { Some(kinds.as_slice()) })
            .bind(preferred_source)
            .bind(path_glob_to_like(path_filter))
            .bind(i64::from(pag.limit.saturating_add(1)))
            .bind(i64::from(pag.offset))
            .fetch_all(&self.state.db)
            .await
            .map_err(|e| format!("crate.api query failed: {e}"))?
        } else {
            sqlx::query_as::<_, ApiSurfaceRow>(
                "SELECT
                    s.name,
                    s.kind,
                    s.signature,
                    s.visibility,
                    sf.path AS source_path,
                    s.start_line,
                    s.end_line,
                    s.index_source
                 FROM symbols s
                 JOIN source_files sf ON sf.id = s.source_file_id
                 WHERE s.crate_version_id = $1
                   AND s.visibility = 'public'
                   AND ($2::TEXT[] IS NULL OR s.kind = ANY($2))
                                     AND ($3::TEXT IS NULL OR s.index_source = $3)
                 ORDER BY
                    s.kind ASC,
                    s.name ASC,
                    sf.path ASC,
                    s.start_line ASC,
                    s.end_line ASC
                 LIMIT $4
                 OFFSET $5",
            )
            .bind(resolution.selected_version.id)
            .bind(if kinds.is_empty() { None } else { Some(kinds.as_slice()) })
            .bind(preferred_source)
            .bind(i64::from(pag.limit.saturating_add(1)))
            .bind(i64::from(pag.offset))
            .fetch_all(&self.state.db)
            .await
            .map_err(|e| format!("crate.api query failed: {e}"))?
        };

        let has_more = rows.len() > pag.limit as usize;
        if has_more {
            rows.truncate(pag.limit as usize);
        }
        let next_cursor = if has_more {
            Some(encode_cursor(&CrateApiCursorToken {
                v: 1,
                offset: pag
                    .offset
                    .saturating_add(pag.limit),
                limit: pag.limit,
                crate_name: crate_name.clone(),
                version: requested_version.clone(),
                path_glob: path_glob.clone(),
                kinds: kinds.clone(),
            })?)
        } else {
            None
        };

        let symbols = rows
            .into_iter()
            .map(|row| CrateApiSymbol {
                name: row.name,
                kind: row.kind,
                signature: row.signature,
                visibility: row.visibility,
                source_path: row.source_path,
                start_line: row.start_line,
                end_line: row.end_line,
                index_source: row.index_source,
            })
            .collect::<Vec<_>>();

        let confidence_assessment = if symbols.is_empty() {
            ConfidenceAssessment {
                level: ConfidenceLevel::Low,
                reason: "no public symbols matched the selected version/filter constraints"
                    .to_string(),
            }
        } else if symbols
            .iter()
            .any(|symbol| symbol.signature.is_none())
        {
            ConfidenceAssessment {
                level: ConfidenceLevel::Medium,
                reason: "some public symbols are missing extracted signatures".to_string(),
            }
        } else {
            ConfidenceAssessment {
                level: ConfidenceLevel::High,
                reason: if has_rustdoc_symbols {
                    "public symbol surface resolved from rustdoc_json indexed symbols".to_string()
                } else {
                    "public symbol surface resolved from indexed symbols".to_string()
                },
            }
        };

        let freshness_check_result = ctx
            .freshness_outcome
            .freshness_check_result
            .clone();

        Ok(Json(CrateApiResponse {
            crate_name: ctx.crate_row.name,
            selected_version: resolution
                .selected_version
                .version,
            latest_version: ctx.latest_version.version,
            path_glob,
            kinds,
            cursor,
            next_cursor,
            page: pag.effective_page,
            limit: pag.limit,
            has_more,
            truncated: has_more,
            count: symbols.len(),
            symbols,
            freshness_check_performed: ctx
                .freshness_outcome
                .freshness_check_performed,
            freshness_check_result: freshness_check_result.clone(),
            refresh_enqueued: resolution.refresh_enqueued,
            refresh_job_id: resolution.refresh_job_id,
            freshness: build_crate_freshness_sources(
                ctx.crate_row
                    .updated_at
                    .clone(),
                &freshness_check_result,
            ),
            confidence: confidence_assessment
                .level
                .as_str()
                .to_string(),
            confidence_assessment,
            next_best_calls: vec![
                "crate.api_diff".to_string(),
                "symbol.search".to_string(),
                "source.read".to_string(),
            ],
            provenance: "local_postgres_index".to_string(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::{allowed_kind_filters, normalize_kind_filters};

    #[test]
    fn normalize_kind_filters_dedupes_and_lowers() {
        let values = normalize_kind_filters(Some(vec![
            " Function ".to_string(),
            "trait".to_string(),
            "function".to_string(),
        ]));
        assert_eq!(values, vec!["function".to_string(), "trait".to_string()]);
    }

    #[test]
    fn allowed_kind_filters_removes_unknown_values() {
        let values = allowed_kind_filters(vec![
            "function".to_string(),
            "macro".to_string(),
            "module".to_string(),
        ]);
        assert_eq!(values, vec!["function".to_string(), "module".to_string()]);
    }
}
