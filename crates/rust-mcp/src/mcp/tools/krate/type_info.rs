use std::collections::HashMap;

use rmcp::{Json, schemars};
use serde::{Deserialize, Serialize};
use sqlx::types::Json as SqlJson;

use crate::db::models::{
    CrateCoreRow, CrateImplLookupRow, CrateTraitLookupRow, CrateTypeInfoRow,
    CrateVersionSelectionRow, GenericParamEntry, ImplMethodEntry, TraitAssociatedTypeEntry,
    TypeFieldEntry, TypeVariantEntry,
};
use crate::mcp::models::{
    ConfidenceAssessment, ConfidenceLevel, CrateImplMethod, CrateTraitAssociatedType,
    CrateTraitDefinition, ResponseFreshnessSource,
};
use crate::mcp::server::McpServer;
use crate::mcp::utils::{normalize_optional, normalize_required};

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CrateTypeInfoRequest {
    pub crate_name: String,
    pub type_name: String,
    pub version: Option<String>,
    pub include_methods: Option<bool>,
    pub include_trait_impls: Option<bool>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateTypeInfoResponse {
    pub crate_name: String,
    pub selected_version: String,
    pub latest_version: String,
    pub type_name: String,
    pub include_methods: bool,
    pub include_trait_impls: bool,
    pub type_definition: Option<CrateTypeDefinition>,
    pub inherent_methods: Vec<CrateImplMethod>,
    pub trait_impls: Vec<CrateTraitImpl>,
    pub trait_definitions: Vec<CrateTraitDefinition>,
    pub conversions: Vec<CrateTypeConversion>,
    pub freshness_check_performed: bool,
    pub freshness_check_result: String,
    pub refresh_enqueued: bool,
    pub refresh_job_id: Option<String>,
    pub freshness: Vec<ResponseFreshnessSource>,
    pub confidence: String,
    pub confidence_assessment: ConfidenceAssessment,
    pub next_best_calls: Vec<String>,
    pub provenance: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateTypeDefinition {
    pub type_name: String,
    pub kind: String,
    pub visibility: Option<String>,
    pub canonical_path: Option<String>,
    pub definition_path: Option<String>,
    pub generic_params: Vec<String>,
    pub where_clauses: Vec<String>,
    pub fields: Vec<CrateTypeField>,
    pub variants: Vec<CrateTypeVariant>,
    pub deprecated_since: Option<String>,
    pub deprecated_note: Option<String>,
    pub is_non_exhaustive: bool,
    pub auto_traits: Vec<String>,
    pub source_path: String,
    pub start_line: i32,
    pub end_line: i32,
    pub index_source: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateTypeField {
    pub name: Option<String>,
    pub field_type: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateTypeVariant {
    pub name: String,
    pub fields: Vec<CrateTypeField>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateTraitImpl {
    pub trait_name: Option<String>,
    pub trait_name_display: Option<String>,
    pub impl_kind: String,
    pub blanket_impl: bool,
    pub synthetic_impl: bool,
    pub negative_impl: bool,
    pub blanket_type: Option<String>,
    pub generic_params: Vec<String>,
    pub where_clauses: Vec<String>,
    pub methods: Vec<CrateImplMethod>,
    pub source_path: String,
    pub start_line: i32,
    pub end_line: i32,
    pub index_source: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateTypeConversion {
    pub trait_name: String,
    pub source_type: Option<String>,
    pub target_type: Option<String>,
}

fn parse_type_fields(value: &SqlJson<Vec<TypeFieldEntry>>) -> Vec<CrateTypeField> {
    value
        .0
        .iter()
        .map(|field| CrateTypeField {
            name: field.name.clone(),
            field_type: field.field_type.clone(),
        })
        .collect()
}

fn parse_type_variants(value: &SqlJson<Vec<TypeVariantEntry>>) -> Vec<CrateTypeVariant> {
    value
        .0
        .iter()
        .map(|variant| CrateTypeVariant {
            name: variant.name.clone(),
            fields: variant
                .fields
                .iter()
                .map(|field| CrateTypeField {
                    name: field.name.clone(),
                    field_type: field.field_type.clone(),
                })
                .collect(),
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
    pub(crate) async fn handle_crate_type_info(
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
                ct.canonical_path,
                ct.definition_path,
                ct.generic_params,
                ct.where_clauses,
                ct.fields,
                ct.variants,
                ct.deprecated_since,
                ct.deprecated_note,
                ct.is_non_exhaustive,
                ct.auto_traits,
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
            canonical_path: row.canonical_path,
            definition_path: row.definition_path,
            generic_params: parse_generic_param_rendered(&row.generic_params),
            where_clauses: parse_string_list(&row.where_clauses),
            fields: parse_type_fields(&row.fields),
            variants: parse_type_variants(&row.variants),
            deprecated_since: row.deprecated_since,
            deprecated_note: row.deprecated_note,
            is_non_exhaustive: row.is_non_exhaustive,
            auto_traits: parse_string_list(&row.auto_traits),
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
                    blanket_impl: row.is_blanket,
                    synthetic_impl: row.is_synthetic,
                    negative_impl: row.is_negative,
                    blanket_type: row.blanket_type.clone(),
                    generic_params: parse_generic_param_rendered(&row.generics),
                    where_clauses: parse_string_list(&row.where_clauses),
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

        let trait_names = trait_impls
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
            .map_err(|e| format!("crate.type_info trait definition query failed: {e}"))?;

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
            trait_definitions,
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
