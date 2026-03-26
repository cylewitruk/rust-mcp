//! Integration tests for `source.*` MCP tools.

use serde_json::{Value, json};

use super::common;

mod case_insensitive;
mod context;
mod error_hints;
mod search_read;
