//! Integration tests for `dependency.*` MCP tools.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use super::common;

fn write_temp_manifest(contents: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("rust-mcp-dep-audit-{nanos}"));
    std::fs::create_dir_all(&dir).expect("failed to create temporary manifest directory");
    let manifest_path = dir.join("Cargo.toml");
    std::fs::write(&manifest_path, contents).expect("failed to write temporary Cargo.toml");
    manifest_path
}

mod audit;
mod feature_impact;
mod resolve;
