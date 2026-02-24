use std::collections::BTreeMap;

use quote::ToTokens as _;
use rmcp::Json;
pub use rust_mcp_types::types::krate::{
    CrateAttributeMacroEntry, CrateDeriveMacroEntry, CrateDeriveMacrosRequest,
    CrateDeriveMacrosResponse, CrateFunctionLikeMacroEntry,
};
use syn::punctuated::Punctuated;
use syn::{Attribute, Item};

use crate::db::tools;
use crate::mcp::models::{ConfidenceAssessment, ConfidenceLevel};
use crate::mcp::server::McpServer;
use crate::mcp::utils::{
    build_crate_freshness_sources, dedupe_strings, normalize_optional, normalize_required,
};

#[derive(Debug, Clone)]
struct DeriveMacroCandidate {
    name: String,
    accepted_attributes: Vec<String>,
    usage_pattern: String,
    source_path: String,
    source_line: i32,
}

#[derive(Debug, Clone)]
struct AttributeMacroCandidate {
    name: String,
    usage_pattern: String,
    signature_pattern: Option<String>,
    source_path: String,
    source_line: i32,
}

#[derive(Debug, Clone)]
struct FunctionLikeMacroCandidate {
    name: String,
    usage_pattern: String,
    signature_pattern: Option<String>,
    source_path: String,
    source_line: i32,
}

fn path_terminal(path: &syn::Path) -> Option<String> {
    path.segments
        .last()
        .map(|segment| segment.ident.to_string())
}

fn parse_proc_macro_derive(attr: &Attribute) -> Option<(String, Vec<String>)> {
    let (derive_path, accepted_paths) = attr
        .parse_args_with(|input: syn::parse::ParseStream<'_>| {
            let derive_path: syn::Path = input.parse()?;
            let mut accepted = Vec::<syn::Path>::new();

            if input.peek(syn::Token![,]) {
                input.parse::<syn::Token![,]>()?;
                let marker: syn::Ident = input.parse()?;
                if marker == "attributes" {
                    let content;
                    syn::parenthesized!(content in input);
                    accepted = Punctuated::<syn::Path, syn::Token![,]>::parse_terminated(&content)?
                        .into_iter()
                        .collect();
                }
            }

            Ok((derive_path, accepted))
        })
        .ok()?;

    let derive_name = path_terminal(&derive_path)?;
    let accepted = accepted_paths
        .iter()
        .filter_map(path_terminal)
        .collect::<Vec<_>>();

    Some((derive_name, accepted))
}

fn line_for_function(content: &str, fn_name: &str) -> i32 {
    let needle = format!("fn {fn_name}");
    for (line_index, line) in content.lines().enumerate() {
        if line.contains(&needle) {
            return (line_index + 1) as i32;
        }
    }
    1
}

fn signature_pattern(sig: &syn::Signature) -> Option<String> {
    let rendered = sig
        .to_token_stream()
        .to_string();
    if rendered.trim().is_empty() { None } else { Some(rendered) }
}

fn collect_macro_exports(
    source_path: &str,
    content: &str,
) -> (Vec<DeriveMacroCandidate>, Vec<AttributeMacroCandidate>, Vec<FunctionLikeMacroCandidate>) {
    let file = match syn::parse_file(content) {
        Ok(file) => file,
        Err(_) => return (Vec::new(), Vec::new(), Vec::new()),
    };

    let mut derive_macros = Vec::new();
    let mut attribute_macros = Vec::new();
    let mut function_like_macros = Vec::new();

    for item in file.items {
        let Item::Fn(function) = item else {
            continue;
        };

        let function_name = function.sig.ident.to_string();
        let source_line = line_for_function(content, &function_name);
        let signature = signature_pattern(&function.sig);

        for attr in &function.attrs {
            if attr
                .path()
                .is_ident("proc_macro_derive")
            {
                if let Some((derive_name, accepted_attributes)) = parse_proc_macro_derive(attr) {
                    derive_macros.push(DeriveMacroCandidate {
                        usage_pattern: format!("#[derive({derive_name})]"),
                        name: derive_name,
                        accepted_attributes,
                        source_path: source_path.to_string(),
                        source_line,
                    });
                }
                continue;
            }

            if attr
                .path()
                .is_ident("proc_macro_attribute")
            {
                attribute_macros.push(AttributeMacroCandidate {
                    name: function_name.clone(),
                    usage_pattern: format!("#[{}(...)]", function_name),
                    signature_pattern: signature.clone(),
                    source_path: source_path.to_string(),
                    source_line,
                });
                continue;
            }

            if attr
                .path()
                .is_ident("proc_macro")
            {
                function_like_macros.push(FunctionLikeMacroCandidate {
                    name: function_name.clone(),
                    usage_pattern: format!("{}!(...)", function_name),
                    signature_pattern: signature.clone(),
                    source_path: source_path.to_string(),
                    source_line,
                });
            }
        }
    }

    (derive_macros, attribute_macros, function_like_macros)
}

impl McpServer {
    pub(crate) async fn handle_crate_derive_macros(
        &self,
        request: CrateDeriveMacrosRequest,
    ) -> Result<Json<CrateDeriveMacrosResponse>, String> {
        let crate_name = normalize_required(request.crate_name, "crate_name")?;
        let requested_version = normalize_optional(request.version);

        let ctx = self
            .fetch_crate_context(&crate_name)
            .await?;
        let resolution = self
            .resolve_version_or_latest(&ctx, requested_version.as_deref())
            .await?;

        self.ensure_source_indexed(&crate_name, resolution.selected_version.id)
            .await?;

        let source_rows = tools::list_rust_source_files_for_version(
            &self.state.db,
            resolution.selected_version.id,
        )
        .await
        .map_err(|e| format!("crate.derive_macros source query failed: {e}"))?;

        let mut derive_macros = BTreeMap::<String, CrateDeriveMacroEntry>::new();
        let mut attribute_macros = BTreeMap::<String, CrateAttributeMacroEntry>::new();
        let mut function_like_macros = BTreeMap::<String, CrateFunctionLikeMacroEntry>::new();

        for source in source_rows {
            let (derive_candidates, attribute_candidates, function_like_candidates) =
                collect_macro_exports(&source.path, &source.content);

            for candidate in derive_candidates {
                derive_macros
                    .entry(candidate.name.clone())
                    .and_modify(|entry| {
                        entry
                            .accepted_attributes
                            .extend(
                                candidate
                                    .accepted_attributes
                                    .clone(),
                            );
                        entry.accepted_attributes = dedupe_strings(
                            entry
                                .accepted_attributes
                                .clone(),
                        );
                    })
                    .or_insert_with(|| CrateDeriveMacroEntry {
                        name: candidate.name,
                        accepted_attributes: dedupe_strings(candidate.accepted_attributes),
                        usage_pattern: candidate.usage_pattern,
                        source_path: candidate.source_path,
                        source_line: candidate.source_line,
                    });
            }

            for candidate in attribute_candidates {
                attribute_macros
                    .entry(candidate.name.clone())
                    .or_insert_with(|| CrateAttributeMacroEntry {
                        name: candidate.name,
                        usage_pattern: candidate.usage_pattern,
                        signature_pattern: candidate.signature_pattern,
                        source_path: candidate.source_path,
                        source_line: candidate.source_line,
                    });
            }

            for candidate in function_like_candidates {
                function_like_macros
                    .entry(candidate.name.clone())
                    .or_insert_with(|| CrateFunctionLikeMacroEntry {
                        name: candidate.name,
                        usage_pattern: candidate.usage_pattern,
                        signature_pattern: candidate.signature_pattern,
                        source_path: candidate.source_path,
                        source_line: candidate.source_line,
                    });
            }
        }

        let derive_macros = derive_macros
            .into_values()
            .collect::<Vec<_>>();
        let attribute_macros = attribute_macros
            .into_values()
            .collect::<Vec<_>>();
        let function_like_macros = function_like_macros
            .into_values()
            .collect::<Vec<_>>();

        let total_macros =
            derive_macros.len() + attribute_macros.len() + function_like_macros.len();
        let (confidence_level, confidence_reason) = if total_macros > 0 {
            (
                ConfidenceLevel::High,
                format!("parsed {total_macros} proc-macro exports from indexed Rust source files"),
            )
        } else {
            (
                ConfidenceLevel::Low,
                "no proc-macro exports detected in indexed source (crate may not be a proc-macro \
                 crate or source is incomplete)"
                    .to_string(),
            )
        };
        let freshness_check_result = ctx
            .freshness_outcome
            .freshness_check_result
            .clone();

        Ok(Json(CrateDeriveMacrosResponse {
            crate_name: ctx.crate_row.name,
            selected_version: resolution
                .selected_version
                .version
                .clone(),
            latest_version: ctx.latest_version.version,
            derive_macros,
            attribute_macros,
            function_like_macros,
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
            confidence: confidence_level
                .as_str()
                .to_string(),
            confidence_assessment: ConfidenceAssessment {
                level: confidence_level,
                reason: confidence_reason,
            },
            next_best_calls: vec![
                "source.search".to_string(),
                "source.read".to_string(),
                "crate.re_exports".to_string(),
            ],
            provenance: "source_files.language=rust parsed via syn for proc_macro attributes"
                .to_string(),
        }))
    }
}
