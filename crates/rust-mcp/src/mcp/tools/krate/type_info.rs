use std::collections::HashMap;

use rmcp::Json;
pub use rust_mcp_types::types::krate::{
    CrateTraitImpl, CrateTypeConversion, CrateTypeDefinition, CrateTypeField, CrateTypeInfoRequest,
    CrateTypeInfoResponse, CrateTypeVariant,
};
use sqlx::types::Json as SqlJson;

use crate::db::models::{
    CrateImplLookupRow, CrateTraitLookupRow, GenericParamEntry, ImplMethodEntry,
    TraitAssociatedTypeEntry, TypeFieldEntry, TypeVariantEntry,
};
use crate::db::tools;
use crate::mcp::models::{
    ConfidenceAssessment, ConfidenceLevel, CrateImplMethod, CrateTraitAssociatedType,
    CrateTraitDefinition,
};
use crate::mcp::server::McpServer;
use crate::mcp::utils::{build_crate_freshness_sources, normalize_optional, normalize_required};

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

fn impl_kind_rank(kind: &str) -> u8 {
    match kind {
        "inherent" => 0,
        "derive" => 1,
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

        let ctx = self
            .fetch_crate_context(&crate_name)
            .await?;
        let resolution = self
            .resolve_version_or_latest(&ctx, requested_version.as_deref())
            .await?;

        let type_row = tools::fetch_crate_type_info_row(
            &self.state.db,
            resolution.selected_version.id,
            &type_name,
        )
        .await
        .map_err(|e| format!("crate.type_info type query failed: {e}"))?;

        let impl_rows = tools::list_crate_impl_rows_for_filters(
            &self.state.db,
            resolution.selected_version.id,
            None,
            Some(&type_name),
            None,
            None,
        )
        .await
        .map_err(|e| format!("crate.type_info impl query failed: {e}"))?;

        let impl_rows = prioritize_impl_rows(impl_rows);

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
            let rows = tools::list_crate_trait_rows_for_names(
                &self.state.db,
                resolution.selected_version.id,
                &trait_names,
            )
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

        let freshness_check_result = ctx
            .freshness_outcome
            .freshness_check_result
            .clone();

        Ok(Json(CrateTypeInfoResponse {
            crate_name: ctx.crate_row.name,
            selected_version: resolution
                .selected_version
                .version,
            latest_version: ctx.latest_version.version,
            type_name,
            include_methods,
            include_trait_impls,
            type_definition,
            inherent_methods,
            trait_impls,
            trait_definitions,
            conversions,
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
                "crate.trait_impls".to_string(),
                "crate.api".to_string(),
                "source.search".to_string(),
            ],
            provenance: "local_postgres_index(crate_types, crate_impls, source_files)".to_string(),
        }))
    }
}
