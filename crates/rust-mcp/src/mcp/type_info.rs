use rmcp::Json;
use serde_json::Value;

use super::models::{
    ConfidenceAssessment, ConfidenceLevel, CrateCoreRow, CrateImplLookupRow, CrateImplMethod,
    CrateTraitImpl, CrateTypeConversion, CrateTypeDefinition, CrateTypeField, CrateTypeInfoRequest,
    CrateTypeInfoResponse, CrateTypeInfoRow, CrateTypeVariant, CrateVersionSelectionRow,
    ResponseFreshnessSource,
};
use super::server::McpServer;
use super::utils::{normalize_optional, normalize_required};

fn parse_type_fields(value: &Value) -> Vec<CrateTypeField> {
    match value {
        Value::Array(entries) => entries
            .iter()
            .filter_map(|entry| {
                let field_type = entry
                    .get("type")?
                    .as_str()?
                    .to_string();
                let name = entry
                    .get("name")
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                Some(CrateTypeField { name, field_type })
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn parse_type_variants(value: &Value) -> Vec<CrateTypeVariant> {
    match value {
        Value::Array(entries) => entries
            .iter()
            .filter_map(|entry| {
                let name = entry
                    .get("name")?
                    .as_str()?
                    .to_string();
                let fields = entry
                    .get("fields")
                    .map(parse_type_fields)
                    .unwrap_or_default();
                Some(CrateTypeVariant { name, fields })
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn parse_string_list(value: &Value) -> Vec<String> {
    match value {
        Value::Array(entries) => entries
            .iter()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

fn parse_impl_methods(value: &Value) -> Vec<CrateImplMethod> {
    match value {
        Value::Array(entries) => entries
            .iter()
            .filter_map(|entry| {
                let name = entry
                    .get("name")?
                    .as_str()?
                    .to_string();
                let signature = entry
                    .get("signature")
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                Some(CrateImplMethod { name, signature })
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn extract_generic_argument(value: &str) -> Option<String> {
    let start = value.find('<')?;
    let end = value.rfind('>')?;
    if end <= start + 1 {
        return None;
    }

    let inner = value[start + 1..end].trim();
    if inner.is_empty() { None } else { Some(inner.to_string()) }
}

impl McpServer {
    pub(super) async fn handle_crate_type_info(
        &self,
        request: CrateTypeInfoRequest,
    ) -> Result<Json<CrateTypeInfoResponse>, String> {
        let crate_name = normalize_required(request.crate_name, "crate_name")?;
        let type_name = normalize_required(request.type_name, "type_name")?;
        let requested_version = normalize_optional(request.version);
        let include_methods = request
            .include_methods
            .unwrap_or(true);
        let include_trait_impls = request
            .include_trait_impls
            .unwrap_or(true);

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

        let type_row = sqlx::query_as::<_, CrateTypeInfoRow>(
            "SELECT
                ct.type_name,
                ct.kind,
                ct.visibility,
                ct.generic_params,
                ct.fields,
                ct.variants,
                sf.path AS source_path,
                ct.start_line,
                ct.end_line,
                ct.index_source
             FROM crate_types ct
             JOIN source_files sf ON sf.id = ct.source_file_id
             WHERE ct.crate_version_id = $1
               AND LOWER(ct.type_name) = LOWER($2)
             ORDER BY
                CASE WHEN ct.visibility = 'public' THEN 0 ELSE 1 END,
                CASE WHEN ct.index_source = 'rustdoc_json' THEN 0 ELSE 1 END,
                ct.start_line ASC
             LIMIT 1",
        )
        .bind(selected_version.id)
        .bind(&type_name)
        .fetch_optional(&self.state.db)
        .await
        .map_err(|e| format!("crate.type_info type query failed: {e}"))?;

        let impl_rows = sqlx::query_as::<_, CrateImplLookupRow>(
            "SELECT
                ci.type_name,
                ci.type_name_display,
                ci.trait_name,
                ci.trait_name_display,
                ci.impl_kind,
                ci.methods,
                sf.path AS source_path,
                ci.start_line,
                ci.end_line,
                ci.index_source
             FROM crate_impls ci
             JOIN source_files sf ON sf.id = ci.source_file_id
             WHERE ci.crate_version_id = $1
               AND LOWER(ci.type_name) = LOWER($2)
             ORDER BY
                CASE ci.impl_kind
                    WHEN 'inherent' THEN 0
                    WHEN 'derive' THEN 1
                    ELSE 2
                END,
                CASE WHEN ci.index_source = 'rustdoc_json' THEN 0 ELSE 1 END,
                ci.start_line ASC",
        )
        .bind(selected_version.id)
        .bind(&type_name)
        .fetch_all(&self.state.db)
        .await
        .map_err(|e| format!("crate.type_info impl query failed: {e}"))?;

        let type_definition = type_row.map(|row| CrateTypeDefinition {
            type_name: row.type_name,
            kind: row.kind,
            visibility: row.visibility,
            generic_params: parse_string_list(&row.generic_params),
            fields: parse_type_fields(&row.fields),
            variants: parse_type_variants(&row.variants),
            source_path: row.source_path,
            start_line: row.start_line,
            end_line: row.end_line,
            index_source: row.index_source,
        });

        let inherent_methods = if include_methods {
            impl_rows
                .iter()
                .filter(|row| row.impl_kind == "inherent")
                .flat_map(|row| parse_impl_methods(&row.methods))
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };

        let trait_impls = if include_trait_impls {
            impl_rows
                .iter()
                .filter(|row| row.impl_kind != "inherent")
                .map(|row| CrateTraitImpl {
                    trait_name: row.trait_name.clone(),
                    trait_name_display: row.trait_name_display.clone(),
                    impl_kind: row.impl_kind.clone(),
                    methods: parse_impl_methods(&row.methods),
                    source_path: row.source_path.clone(),
                    start_line: row.start_line,
                    end_line: row.end_line,
                    index_source: row.index_source.clone(),
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };

        let conversions = impl_rows
            .iter()
            .filter_map(|row| {
                let trait_name = row.trait_name.as_deref()?;
                let normalized = trait_name.trim();
                if !matches!(normalized, "From" | "Into" | "TryFrom" | "TryInto") {
                    return None;
                }

                let generic_argument = row
                    .trait_name_display
                    .as_deref()
                    .and_then(extract_generic_argument);
                let type_display = row
                    .type_name_display
                    .clone()
                    .or_else(|| Some(row.type_name.clone()));

                let (source_type, target_type) = match normalized {
                    "From" | "TryFrom" => (generic_argument, type_display),
                    "Into" | "TryInto" => (type_display, generic_argument),
                    _ => (None, None),
                };

                Some(CrateTypeConversion {
                    trait_name: normalized.to_string(),
                    source_type,
                    target_type,
                })
            })
            .collect::<Vec<_>>();

        let confidence_assessment = if type_definition.is_none() {
            ConfidenceAssessment {
                level: ConfidenceLevel::Low,
                reason: "type definition was not found in indexed local cache data".to_string(),
            }
        } else if include_methods
            && include_trait_impls
            && inherent_methods.is_empty()
            && trait_impls.is_empty()
        {
            ConfidenceAssessment {
                level: ConfidenceLevel::Medium,
                reason: "type was found but no impl methods were extracted for this indexed \
                         version"
                    .to_string(),
            }
        } else {
            ConfidenceAssessment {
                level: ConfidenceLevel::High,
                reason: "type metadata and impl associations resolved from local index".to_string(),
            }
        };

        let freshness_check_result = freshness_outcome
            .freshness_check_result
            .clone();

        Ok(Json(CrateTypeInfoResponse {
            crate_name: crate_row.name,
            selected_version: selected_version.version,
            latest_version: latest_version.version,
            type_name,
            include_methods,
            include_trait_impls,
            type_definition,
            inherent_methods,
            trait_impls,
            conversions,
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
                "crate.trait_impls".to_string(),
                "crate.api".to_string(),
                "source.search".to_string(),
            ],
            provenance: "local_postgres_index(crate_types, crate_impls, source_files)".to_string(),
        }))
    }
}
