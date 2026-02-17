use std::collections::HashMap;

use rmcp::Json;
pub use rust_mcp_types::types::krate::{
    CrateTraitImplRelation, CrateTraitImplsRequest, CrateTraitImplsResponse,
};
use sqlx::types::Json as SqlJson;

use crate::db::models::{
    CrateCoreRow, CrateImplLookupRow, CrateTraitLookupRow, CrateVersionSelectionRow,
    GenericParamEntry, ImplMethodEntry, TraitAssociatedTypeEntry,
};
use crate::mcp::models::{
    ConfidenceAssessment, ConfidenceLevel, CrateImplMethod, CrateTraitAssociatedType,
    CrateTraitDefinition, ResponseFreshnessSource,
};
use crate::mcp::server::McpServer;
use crate::mcp::utils::{normalize_optional, normalize_required, trait_impls_limit};

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

impl McpServer {
    pub(crate) async fn handle_crate_trait_impls(
        &self,
        request: CrateTraitImplsRequest,
    ) -> Result<Json<CrateTraitImplsResponse>, String> {
        let crate_name = normalize_required(request.crate_name, "crate_name")?;
        let requested_version = normalize_optional(request.version);
        let trait_name = normalize_optional(request.trait_name);
        let type_name = normalize_optional(request.type_name);
        let limit = trait_impls_limit(request.limit);

        if trait_name.is_none() && type_name.is_none() {
            return Err("either trait_name or type_name must be provided".to_string());
        }

        let crate_row = sqlx::query_as::<_, CrateCoreRow>(
            "SELECT
                id,
                name,
                description,
                repository_url,
                docs_url,
                homepage_url,
                categories,
                keywords,
                updated_at::TEXT AS updated_at
             FROM crates
             WHERE name = $1",
        )
        .bind(&crate_name)
        .fetch_optional(&self.state.db)
        .await
        .map_err(|e| format!("crate lookup failed for {crate_name}: {e}"))?
        .ok_or_else(|| {
            format!("crate '{crate_name}' is not indexed locally; run index.sync_crates first")
        })?;

        let latest_version = sqlx::query_as::<_, CrateVersionSelectionRow>(
            "SELECT
                id,
                version,
                rust_version,
                published_at::TEXT AS published_at,
                readme
             FROM crate_versions
             WHERE crate_id = $1
             ORDER BY published_at DESC NULLS LAST, id DESC
             LIMIT 1",
        )
        .bind(crate_row.id)
        .fetch_optional(&self.state.db)
        .await
        .map_err(|e| format!("latest version lookup failed for {crate_name}: {e}"))?
        .ok_or_else(|| {
            format!(
                "crate '{}' has no indexed versions yet; run index.sync_crates first",
                crate_row.name
            )
        })?;

        let freshness_outcome = self
            .ensure_freshness_for_interaction(
                crate_row.id,
                &crate_row.name,
                &latest_version.version,
            )
            .await?;

        let latest_version = if freshness_outcome.freshness_check_result == "changed" {
            sqlx::query_as::<_, CrateVersionSelectionRow>(
                "SELECT
                    id,
                    version,
                    rust_version,
                    published_at::TEXT AS published_at,
                    readme
                 FROM crate_versions
                 WHERE crate_id = $1
                 ORDER BY published_at DESC NULLS LAST, id DESC
                 LIMIT 1",
            )
            .bind(crate_row.id)
            .fetch_optional(&self.state.db)
            .await
            .map_err(|e| format!("latest version relookup failed for {crate_name}: {e}"))?
            .ok_or_else(|| {
                format!(
                    "crate '{}' has no indexed versions yet; run index.sync_crates first",
                    crate_row.name
                )
            })?
        } else {
            latest_version
        };

        let mut refresh_enqueued = freshness_outcome.refresh_enqueued;
        let mut refresh_job_id = freshness_outcome
            .refresh_job_id
            .clone();

        let selected_version = if let Some(version) = requested_version {
            let selected = sqlx::query_as::<_, CrateVersionSelectionRow>(
                "SELECT
                    id,
                    version,
                    rust_version,
                    published_at::TEXT AS published_at,
                    readme
                 FROM crate_versions
                 WHERE crate_id = $1 AND version = $2
                 LIMIT 1",
            )
            .bind(crate_row.id)
            .bind(&version)
            .fetch_optional(&self.state.db)
            .await
            .map_err(|e| {
                format!("selected version lookup failed for {}@{}: {e}", crate_row.name, version)
            })?;

            if let Some(selected) = selected {
                selected
            } else {
                let queued_job_id = self
                    .backfill_missing_requested_version(&crate_row.name)
                    .await?;
                if let Some(job_id) = queued_job_id {
                    refresh_enqueued = true;
                    refresh_job_id = Some(job_id);
                }

                sqlx::query_as::<_, CrateVersionSelectionRow>(
                    "SELECT
                        id,
                        version,
                        rust_version,
                        published_at::TEXT AS published_at,
                        readme
                     FROM crate_versions
                     WHERE crate_id = $1 AND version = $2
                     LIMIT 1",
                )
                .bind(crate_row.id)
                .bind(&version)
                .fetch_optional(&self.state.db)
                .await
                .map_err(|e| {
                    format!(
                        "selected version lookup failed after backfill for {}@{}: {e}",
                        crate_row.name, version
                    )
                })?
                .ok_or_else(|| {
                    format!(
                        "version '{}' for crate '{}' is not indexed locally (refresh attempted)",
                        version, crate_row.name
                    )
                })?
            }
        } else {
            latest_version.clone()
        };

        let impl_rows = sqlx::query_as::<_, CrateImplLookupRow>(
            "SELECT
                ci.type_name,
                ci.type_name_display,
                ci.trait_name,
                ci.trait_name_display,
                ci.impl_kind,
                ci.methods,
                ci.is_blanket,
                ci.is_synthetic,
                ci.is_negative,
                ci.blanket_type,
                ci.generics,
                ci.where_clauses,
                sf.path AS source_path,
                ci.start_line,
                ci.end_line,
                ci.index_source
             FROM crate_impls ci
             JOIN source_files sf ON sf.id = ci.source_file_id
             WHERE ci.crate_version_id = $1
               AND ($2::TEXT IS NULL OR LOWER(ci.trait_name) = LOWER($2))
               AND ($3::TEXT IS NULL OR LOWER(ci.type_name) = LOWER($3))
             ORDER BY
                CASE ci.impl_kind
                    WHEN 'derive' THEN 0
                    WHEN 'trait' THEN 1
                    ELSE 2
                END,
                CASE WHEN ci.index_source = 'rustdoc_json' THEN 0 ELSE 1 END,
                ci.type_name ASC,
                ci.start_line ASC
             LIMIT $4",
        )
        .bind(selected_version.id)
        .bind(trait_name.as_deref())
        .bind(type_name.as_deref())
        .bind(i64::from(limit))
        .fetch_all(&self.state.db)
        .await
        .map_err(|e| format!("crate.trait_impls query failed: {e}"))?;

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
            let rows = sqlx::query_as::<_, CrateTraitLookupRow>(
                "SELECT
                    trait_name,
                    is_auto,
                    is_unsafe,
                    is_dyn_compatible,
                    supertraits,
                    required_methods,
                    provided_methods,
                    associated_types,
                    generics,
                    index_source
                 FROM crate_traits
                 WHERE crate_version_id = $1
                   AND LOWER(trait_name) = ANY($2::TEXT[])
                 ORDER BY
                    CASE WHEN index_source = 'rustdoc_json' THEN 0 ELSE 1 END,
                    trait_name ASC",
            )
            .bind(selected_version.id)
            .bind(&trait_names)
            .fetch_all(&self.state.db)
            .await
            .map_err(|e| format!("crate.trait_impls trait definition query failed: {e}"))?;

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

        let freshness_check_result = freshness_outcome
            .freshness_check_result
            .clone();

        Ok(Json(CrateTraitImplsResponse {
            crate_name: crate_row.name,
            selected_version: selected_version.version,
            latest_version: latest_version.version,
            trait_name,
            type_name,
            limit,
            count: impls.len(),
            impls,
            trait_definitions,
            freshness_check_performed: freshness_outcome.freshness_check_performed,
            freshness_check_result: freshness_check_result.clone(),
            refresh_enqueued,
            refresh_job_id,
            freshness: vec![
                ResponseFreshnessSource {
                    source: "local_postgres_index".to_string(),
                    status: "fresh".to_string(),
                    checked_at: crate_row.updated_at,
                },
                ResponseFreshnessSource {
                    source: "crates.io".to_string(),
                    status: freshness_check_result,
                    checked_at: None,
                },
            ],
            confidence: confidence_assessment
                .level
                .as_str()
                .to_string(),
            confidence_assessment,
            next_best_calls: vec![
                "crate.type_info".to_string(),
                "crate.api".to_string(),
                "symbol.search".to_string(),
            ],
            provenance: "local_postgres_index(crate_impls, source_files)".to_string(),
        }))
    }
}
