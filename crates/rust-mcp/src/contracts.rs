use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, anyhow};
use rust_mcp_types::schemas;
use rust_mcp_types::types::schema::{ToolSchemaContract, ToolSchemasResponse};

/// Schema artifact export summary.
#[derive(Debug, Clone)]
pub struct SchemaArtifactExport {
    /// Aggregate snapshot file containing all tool contracts.
    pub aggregate_file: PathBuf,
    /// Per-tool contract files emitted under `tool-schemas/`.
    pub per_tool_files: Vec<PathBuf>,
}

fn to_contract_schema(schema: schemas::ToolSchema) -> ToolSchemaContract {
    ToolSchemaContract {
        tool_name: schema.tool_name.to_string(),
        request: schema.request,
        response: schema.response,
    }
}

/// Builds the `schema.get` response for one tool or all tools.
pub fn tool_schemas_response(tool_name: Option<String>) -> Result<ToolSchemasResponse, String> {
    let schemas = match tool_name.as_deref() {
        Some(name) => {
            vec![schemas::tool_schema(name).ok_or_else(|| format!("unknown MCP tool `{name}`"))?]
        }
        None => schemas::all_tool_schemas(),
    };

    let schemas = schemas
        .into_iter()
        .map(to_contract_schema)
        .collect::<Vec<_>>();

    Ok(ToolSchemasResponse {
        tool_name,
        total_tools: schemas.len(),
        schemas,
    })
}

/// Exports machine-readable tool contract artifacts to a directory.
pub fn export_tool_schema_artifacts(output_dir: &Path) -> Result<SchemaArtifactExport> {
    fs::create_dir_all(output_dir).with_context(|| {
        format!("failed to create schema artifact directory `{}`", output_dir.display())
    })?;

    let response = tool_schemas_response(None).map_err(|error| anyhow!(error))?;

    let aggregate_file = output_dir.join("tool-schemas.json");
    let aggregate_payload = serde_json::to_vec_pretty(&response)
        .context("failed to serialize aggregate tool schema snapshot")?;
    fs::write(&aggregate_file, aggregate_payload).with_context(|| {
        format!("failed to write aggregate tool schema snapshot `{}`", aggregate_file.display())
    })?;

    let per_tool_dir = output_dir.join("tool-schemas");
    fs::create_dir_all(&per_tool_dir).with_context(|| {
        format!("failed to create per-tool schema directory `{}`", per_tool_dir.display())
    })?;

    let mut per_tool_files = Vec::with_capacity(response.schemas.len());
    for schema in response.schemas {
        let output_file = per_tool_dir.join(format!("{}.json", schema.tool_name));
        let payload = serde_json::to_vec_pretty(&schema).with_context(|| {
            format!("failed to serialize schema contract for `{}`", schema.tool_name)
        })?;
        fs::write(&output_file, payload).with_context(|| {
            format!("failed to write schema contract `{}`", output_file.display())
        })?;
        per_tool_files.push(output_file);
    }

    Ok(SchemaArtifactExport { aggregate_file, per_tool_files })
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{export_tool_schema_artifacts, tool_schemas_response};

    #[test]
    fn tool_schemas_response_supports_filtering() {
        let filtered =
            tool_schemas_response(Some("ping".to_string())).expect("schema_get filtering failed");
        assert_eq!(filtered.total_tools, 1);
        assert_eq!(filtered.schemas[0].tool_name, "ping");
    }

    #[test]
    fn export_tool_schema_artifacts_writes_expected_files() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time moved backwards")
            .as_nanos();
        let output_dir = std::env::temp_dir()
            .join(format!("rust-mcp-schema-export-{unique}-{}", std::process::id()));

        let export = export_tool_schema_artifacts(&output_dir).expect("schema export failed");
        assert!(export.aggregate_file.exists(), "aggregate file should exist");
        assert!(
            !export
                .per_tool_files
                .is_empty(),
            "expected one or more per-tool schema artifacts"
        );
        assert!(
            export
                .per_tool_files
                .iter()
                .all(|path| path.exists()),
            "all per-tool schema files should exist"
        );

        let _ = std::fs::remove_dir_all(output_dir);
    }
}
