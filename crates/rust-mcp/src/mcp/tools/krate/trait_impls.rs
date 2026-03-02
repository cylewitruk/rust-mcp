use std::collections::HashMap;

use rmcp::Json;
pub use rust_mcp_types::types::krate::{
    CrateTraitImplRelation, CrateTraitImplsRequest, CrateTraitImplsResponse,
};
use serde::{Deserialize, Serialize};
use sqlx::types::Json as SqlJson;

use crate::db::models::{
    CrateImplLookupRow, CrateTraitLookupRow, GenericParamEntry, ImplMethodEntry,
    TraitAssociatedTypeEntry,
};
use crate::db::tools;
use crate::mcp::models::{
    ConfidenceAssessment, ConfidenceLevel, CrateImplMethod, CrateTraitAssociatedType,
    CrateTraitDefinition,
};
use crate::mcp::server::McpServer;
use crate::mcp::utils::{
    CursorToken, build_crate_freshness_sources, decode_cursor, encode_cursor, normalize_optional,
    normalize_required, resolve_pagination, sync_page, trait_impls_limit,
};

#[derive(Debug, Serialize, Deserialize)]
struct CrateTraitImplsCursorToken {
    v: u8,
    offset: u32,
    limit: u32,
    crate_name: String,
    version: Option<String>,
    trait_name: Option<String>,
    type_name: Option<String>,
}

impl CursorToken for CrateTraitImplsCursorToken {
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

fn parse_impl_methods(value: &SqlJson<Vec<ImplMethodEntry>>) -> Vec<CrateImplMethod> {
    value
        .0
        .iter()
        .map(|entry| CrateImplMethod {
            name: entry.name.clone(),
            signature: entry.signature.clone(),
        })
        .collect()
}

fn parse_string_list(value: &SqlJson<Vec<String>>) -> Vec<String> {
    value.0.clone()
}

fn parse_generic_param_rendered(value: &SqlJson<Vec<GenericParamEntry>>) -> Vec<String> {
    value
        .0
        .iter()
        .map(|entry| entry.rendered().to_string())
        .collect()
}

fn parse_assoc_types(
    value: &SqlJson<Vec<TraitAssociatedTypeEntry>>,
) -> Vec<CrateTraitAssociatedType> {
    value
        .0
        .iter()
        .map(|entry| CrateTraitAssociatedType {
            name: entry.name.clone(),
            bounds: entry.bounds.clone(),
            default: entry.default.clone(),
        })
        .collect()
}

fn trait_definition_from_row(row: CrateTraitLookupRow) -> CrateTraitDefinition {
    CrateTraitDefinition {
        trait_name: row.trait_name,
        is_auto: row.is_auto,
        is_unsafe: row.is_unsafe,
        is_dyn_compatible: row.is_dyn_compatible,
        supertraits: parse_string_list(&row.supertraits),
        required_methods: parse_impl_methods(&row.required_methods),
        provided_methods: parse_impl_methods(&row.provided_methods),
        associated_types: parse_assoc_types(&row.associated_types),
        generic_params: parse_generic_param_rendered(&row.generics),
        index_source: row.index_source,
    }
}

fn impl_kind_rank(kind: &str) -> u8 {
    match kind {
        "derive" => 0,
        "trait" => 1,
        _ => 2,
    }
}

fn impl_row_identity(row: &CrateImplLookupRow) -> String {
    let trait_key = row
        .trait_name
        .as_deref()
        .unwrap_or("")
        .to_ascii_lowercase();
    let trait_display_key = row
        .trait_name_display
        .as_deref()
        .unwrap_or("")
        .to_ascii_lowercase();
    let type_display_key = row
        .type_name_display
        .as_deref()
        .unwrap_or("")
        .to_ascii_lowercase();
    format!(
        "{}|{}|{}|{}|{}",
        row.type_name
            .to_ascii_lowercase(),
        type_display_key,
        trait_key,
        trait_display_key,
        row.impl_kind
    )
}

fn impl_row_priority(row: &CrateImplLookupRow) -> u8 {
    let mut score = 0u8;
    if row.index_source == "rustdoc_json" {
        score = score.saturating_add(4);
    }
    if !row.methods.0.is_empty() {
        score = score.saturating_add(2);
    }
    if row
        .trait_name_display
        .is_some()
        || row
            .type_name_display
            .is_some()
    {
        score = score.saturating_add(1);
    }
    if !row.generics.0.is_empty() || !row.where_clauses.0.is_empty() {
        score = score.saturating_add(1);
    }
    if row.is_blanket || row.is_synthetic || row.is_negative || row.blanket_type.is_some() {
        score = score.saturating_add(1);
    }
    score
}

fn prioritize_impl_rows(rows: Vec<CrateImplLookupRow>) -> Vec<CrateImplLookupRow> {
    let mut best_by_identity = HashMap::<String, (u8, CrateImplLookupRow)>::new();

    for row in rows {
        let key = impl_row_identity(&row);
        let priority = impl_row_priority(&row);
        match best_by_identity.get(&key) {
            Some((existing_priority, _)) if *existing_priority >= priority => {}
            _ => {
                best_by_identity.insert(key, (priority, row));
            }
        }
    }

    let mut prioritized = best_by_identity
        .into_values()
        .map(|(_, row)| row)
        .collect::<Vec<_>>();
    prioritized.sort_by(|left, right| {
        impl_kind_rank(&left.impl_kind)
            .cmp(&impl_kind_rank(&right.impl_kind))
            .then_with(|| {
                if left.index_source == right.index_source {
                    std::cmp::Ordering::Equal
                } else if left.index_source == "rustdoc_json" {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Greater
                }
            })
            .then_with(|| {
                left.type_name
                    .cmp(&right.type_name)
            })
            .then_with(|| {
                left.start_line
                    .cmp(&right.start_line)
            })
            .then_with(|| {
                left.end_line
                    .cmp(&right.end_line)
            })
    });
    prioritized
}

impl McpServer {
    /// Handles the `crate.trait_impls` tool call.
    pub async fn handle_crate_trait_impls(
        &self,
        request: CrateTraitImplsRequest,
    ) -> Result<Json<CrateTraitImplsResponse>, String> {
        let crate_name = normalize_required(request.crate_name, "crate_name")?;
        let requested_version = normalize_optional(request.version);
        let trait_name = normalize_optional(request.trait_name);
        let type_name = normalize_optional(request.type_name);
        let cursor = normalize_optional(request.cursor);
        let page = sync_page(request.page);
        let requested_limit = trait_impls_limit(request.limit);

        let decoded = cursor
            .as_deref()
            .map(decode_cursor::<CrateTraitImplsCursorToken>)
            .transpose()?;

        if let Some(ref token) = decoded
            && (token.crate_name != crate_name
                || token.version != requested_version
                || token.trait_name != trait_name
                || token.type_name != type_name)
        {
            return Err("cursor does not match current crate.trait_impls filters".to_string());
        }

        let pag =
            resolve_pagination(decoded.as_ref(), request.limit.is_some(), requested_limit, page)?;

        if trait_name.is_none() && type_name.is_none() {
            return Err("either trait_name or type_name must be provided".to_string());
        }

        let ctx = self
            .fetch_crate_context(&crate_name)
            .await?;
        let resolution = self
            .resolve_version_or_latest(&ctx, requested_version.as_deref())
            .await?;

        self.ensure_rustdoc_indexed(&crate_name, resolution.selected_version.id)
            .await?;

        let impl_rows = tools::list_crate_impl_rows_for_filters(
            &self.state.db,
            resolution.selected_version.id,
            trait_name.as_deref(),
            type_name.as_deref(),
            Some(i64::from(pag.limit.saturating_add(1))),
            Some(i64::from(pag.offset)),
        )
        .await
        .map_err(|e| format!("crate_trait_impls query failed: {e}"))?;

        let mut impl_rows = prioritize_impl_rows(impl_rows);
        let has_more = impl_rows.len() > pag.limit as usize;
        if has_more {
            impl_rows.truncate(pag.limit as usize);
        }
        let next_cursor = if has_more {
            Some(encode_cursor(&CrateTraitImplsCursorToken {
                v: 1,
                offset: pag
                    .offset
                    .saturating_add(pag.limit),
                limit: pag.limit,
                crate_name: crate_name.clone(),
                version: requested_version.clone(),
                trait_name: trait_name.clone(),
                type_name: type_name.clone(),
            })?)
        } else {
            None
        };

        let impls = impl_rows
            .into_iter()
            .map(|row| CrateTraitImplRelation {
                type_name: row.type_name,
                type_name_display: row.type_name_display.clone(),
                trait_name: row.trait_name,
                trait_name_display: row.trait_name_display,
                impl_kind: row.impl_kind,
                blanket_impl: row.is_blanket,
                synthetic_impl: row.is_synthetic,
                negative_impl: row.is_negative,
                blanket_type: row.blanket_type,
                generic_params: parse_generic_param_rendered(&row.generics),
                where_clauses: parse_string_list(&row.where_clauses),
                methods: parse_impl_methods(&row.methods),
                source_path: row.source_path,
                start_line: row.start_line,
                end_line: row.end_line,
                index_source: row.index_source,
            })
            .collect::<Vec<_>>();

        let trait_names = impls
            .iter()
            .filter_map(|relation| relation.trait_name.as_deref())
            .map(|trait_name| trait_name.to_ascii_lowercase())
            .collect::<Vec<_>>();

        let trait_definitions = if trait_names.is_empty() {
            Vec::new()
        } else {
            let mut definitions_by_name = HashMap::<String, CrateTraitDefinition>::new();
            let rows = tools::list_crate_trait_rows_for_names(
                &self.state.db,
                resolution.selected_version.id,
                &trait_names,
            )
            .await
            .map_err(|e| format!("crate_trait_impls trait definition query failed: {e}"))?;

            for row in rows {
                definitions_by_name
                    .entry(
                        row.trait_name
                            .to_ascii_lowercase(),
                    )
                    .or_insert_with(|| trait_definition_from_row(row));
            }

            let mut out = definitions_by_name
                .into_values()
                .collect::<Vec<_>>();
            out.sort_by(|left, right| {
                left.trait_name
                    .cmp(&right.trait_name)
            });
            out
        };

        let confidence_assessment = if impls.is_empty() {
            ConfidenceAssessment {
                level: ConfidenceLevel::Low,
                reason: "no matching impl relationships were found in indexed local cache data"
                    .to_string(),
            }
        } else if impls
            .iter()
            .any(|relation| relation.trait_name.is_none())
            && trait_name.is_none()
        {
            ConfidenceAssessment {
                level: ConfidenceLevel::Medium,
                reason: "results include inherent impls because lookup was not trait-constrained"
                    .to_string(),
            }
        } else {
            ConfidenceAssessment {
                level: ConfidenceLevel::High,
                reason: "trait/type impl relationships resolved from local index".to_string(),
            }
        };

        let freshness_check_result = ctx
            .freshness_outcome
            .freshness_check_result
            .clone();

        Ok(Json(CrateTraitImplsResponse {
            crate_name: ctx.crate_row.name,
            selected_version: resolution
                .selected_version
                .version,
            latest_version: ctx.latest_version.version,
            trait_name,
            type_name,
            cursor,
            next_cursor,
            page: pag.effective_page,
            limit: pag.limit,
            has_more,
            truncated: has_more,
            count: impls.len(),
            impls,
            trait_definitions,
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
            next_best_calls: vec![
                "crate_type_info".to_string(),
                "crate_api".to_string(),
                "symbol_search".to_string(),
            ],
            provenance: "local_postgres_index(crate_impls, source_files)".to_string(),
        }))
    }
}
